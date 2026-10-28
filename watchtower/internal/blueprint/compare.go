package blueprint

import (
	"fmt"
	"math"
	"sort"

	"github.com/soni0021/reelctl-watchtower/internal/stats"
)

// ChangeThreshold is the smallest relative change that earns a sentence.
const ChangeThreshold = 0.25

const visualKeyPrefix = "visual_events_per_minute."

const (
	unitMs        = "ms"
	unitPerMinute = "per_minute"
	unitShare     = "share"
	unitLUFS      = "lufs"
)

// Metric is one measured number before and after. Before is the baseline
// posts and After is the breakouts, each the mean of the per-reel values.
// Change is (after - before) / |before|, nil when before is zero.
type Metric struct {
	Key    string   `json:"key"`
	Label  string   `json:"label"`
	Unit   string   `json:"unit"`
	Before float64  `json:"before"`
	After  float64  `json:"after"`
	Change *float64 `json:"change"`
}

// Blueprint is the comparison of a creator's breakouts with their baseline posts.
type Blueprint struct {
	Breakouts       int      `json:"breakouts"`
	Baseline        int      `json:"baseline_posts"`
	ChangeThreshold float64  `json:"change_threshold"`
	Metrics         []Metric `json:"metrics"`
	Sentences       []string `json:"sentences"`
	Notes           []string `json:"notes"`
}

type spec struct {
	key, label, unit string
	pick             func(Reel) float64
}

var fixedSpecs = []spec{
	{"mean_shot_length_ms", "Mean shot length", unitMs, func(r Reel) float64 { return r.MeanShotMs }},
	{"median_shot_length_ms", "Median shot length", unitMs, func(r Reel) float64 { return r.MedianShotMs }},
	{"cuts_per_minute", "Cuts per minute", unitPerMinute, func(r Reel) float64 { return r.CutsPerMinute }},
	{"pauses_per_minute", "Pauses per minute", unitPerMinute, func(r Reel) float64 { return r.PausesPerMinute }},
	{"silent_share", "Silent share", unitShare, func(r Reel) float64 { return r.SilentShare }},
	{"words_per_minute", "Words per minute", unitPerMinute, func(r Reel) float64 { return r.WordsPerMinute }},
	{"integrated_lufs", "Integrated loudness", unitLUFS, func(r Reel) float64 { return r.LUFS }},
}

// Compare sets the baseline reels against the breakout reels. Both must be
// non-empty. Every metric is listed; a sentence is written only for a change
// of at least ChangeThreshold.
func Compare(baseline, breakouts []Reel) Blueprint {
	specs, notes := specsFor(baseline, breakouts)
	bp := Blueprint{
		Breakouts:       len(breakouts),
		Baseline:        len(baseline),
		ChangeThreshold: ChangeThreshold,
		Metrics:         []Metric{},
		Sentences:       []string{},
		Notes:           notes,
	}
	for _, s := range specs {
		before, after := groupMean(baseline, s.pick), groupMean(breakouts, s.pick)
		bp.Metrics = append(bp.Metrics, newMetric(s, before, after))
		if notable(before, after) {
			bp.Sentences = append(bp.Sentences, sentence(s, before, after, len(breakouts), len(baseline)))
		}
	}
	return bp
}

func specsFor(baseline, breakouts []Reel) ([]spec, []string) {
	all := append(append([]Reel(nil), baseline...), breakouts...)
	missing := countUnmeasured(all)
	if missing > 0 {
		note := fmt.Sprintf("visual_events was missing from %d of %d files, so visual events are not compared.", missing, len(all))
		return fixedSpecs, []string{note}
	}
	specs := append([]spec(nil), fixedSpecs...)
	for _, kind := range visualKinds(all) {
		specs = append(specs, visualSpec(kind))
	}
	return specs, []string{}
}

func countUnmeasured(reels []Reel) int {
	n := 0
	for _, r := range reels {
		if r.VisualPerMinute == nil {
			n++
		}
	}
	return n
}

func visualKinds(reels []Reel) []string {
	seen := make(map[string]bool)
	for _, r := range reels {
		for kind := range r.VisualPerMinute {
			seen[kind] = true
		}
	}
	kinds := make([]string, 0, len(seen))
	for kind := range seen {
		kinds = append(kinds, kind)
	}
	sort.Strings(kinds)
	return kinds
}

func visualSpec(kind string) spec {
	return spec{
		key:   visualKeyPrefix + kind,
		label: fmt.Sprintf("Visual events per minute (%s)", kind),
		unit:  unitPerMinute,
		pick:  func(r Reel) float64 { return r.VisualPerMinute[kind] },
	}
}

func groupMean(reels []Reel, pick func(Reel) float64) float64 {
	values := make([]float64, len(reels))
	for i, r := range reels {
		values[i] = pick(r)
	}
	return stats.Mean(values)
}

func newMetric(s spec, before, after float64) Metric {
	m := Metric{Key: s.key, Label: s.label, Unit: s.unit, Before: stats.Round(before), After: stats.Round(after)}
	if before != 0 {
		change := stats.Round((after - before) / math.Abs(before))
		m.Change = &change
	}
	return m
}

func notable(before, after float64) bool {
	if before == 0 {
		return after != 0
	}
	return math.Abs(after-before) >= ChangeThreshold*math.Abs(before)
}

func sentence(s spec, before, after float64, breakouts, baseline int) string {
	verb := "rose"
	if after < before {
		verb = "dropped"
	}
	change := ""
	if before != 0 {
		change = " (" + SignedPercent((after-before)/math.Abs(before)) + ")"
	}
	return fmt.Sprintf("%s %s from %s to %s%s across %d breakouts against %d baseline posts.",
		s.label, verb, Display(s.unit, before), Display(s.unit, after), change, breakouts, baseline)
}

// SignedPercent writes a relative change as "+25%" or "−64%", and as "0%"
// when it rounds to zero.
func SignedPercent(change float64) string {
	text := fmt.Sprintf("%.0f%%", math.Abs(change)*100)
	if text == "0%" {
		return text
	}
	if change < 0 {
		return "−" + text
	}
	return "+" + text
}

// Display writes a metric value in the unit people read it in.
func Display(unit string, v float64) string {
	switch unit {
	case unitMs:
		return fmt.Sprintf("%.1f s", v/1000)
	case unitShare:
		return fmt.Sprintf("%.1f%%", v*100)
	case unitLUFS:
		return fmt.Sprintf("%.1f LUFS", v)
	default:
		return fmt.Sprintf("%.1f", v)
	}
}
