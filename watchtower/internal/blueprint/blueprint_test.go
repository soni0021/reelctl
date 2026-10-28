package blueprint

import (
	"math"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func near(a, b float64) bool { return math.Abs(a-b) < 1e-9 }

func cutsAt(times ...uint64) []Cut {
	cuts := make([]Cut, len(times))
	for i, t := range times {
		cuts[i] = Cut{TimeMs: t}
	}
	return cuts
}

func gaps(durations ...uint64) []SilenceGap {
	out := make([]SilenceGap, len(durations))
	for i, d := range durations {
		out[i] = SilenceGap{DurationMs: d}
	}
	return out
}

func TestMeasureShotLengthsFromCutsAndDuration(t *testing.T) {
	dna := DNA{DurationMs: 30000, Cuts: cutsAt(4000, 8000, 12000, 16000, 20000, 24000, 28000), WordsPerMinute: 150, IntegratedLUFS: -16}
	r, err := Measure(dna)
	if err != nil {
		t.Fatal(err)
	}
	if r.Shots != 8 {
		t.Errorf("shots = %d, want 8", r.Shots)
	}
	if !near(r.MeanShotMs, 3750) || !near(r.MedianShotMs, 4000) {
		t.Errorf("mean %v median %v, want 3750 and 4000", r.MeanShotMs, r.MedianShotMs)
	}
	if !near(r.CutsPerMinute, 14) {
		t.Errorf("cuts per minute = %v, want 14", r.CutsPerMinute)
	}
	if r.WordsPerMinute != 150 || r.LUFS != -16 {
		t.Errorf("words per minute %v, LUFS %v", r.WordsPerMinute, r.LUFS)
	}
}

func TestMeasureWithoutCutsIsOneShot(t *testing.T) {
	r, err := Measure(DNA{DurationMs: 20000})
	if err != nil {
		t.Fatal(err)
	}
	if r.Shots != 1 || !near(r.MeanShotMs, 20000) || r.CutsPerMinute != 0 {
		t.Fatalf("got %+v", r)
	}
}

func TestMeasureIgnoresCutsAtTheEdgesAndSortsTheRest(t *testing.T) {
	r, err := Measure(DNA{DurationMs: 10000, Cuts: cutsAt(6000, 0, 3000, 3000, 10000, 12000)})
	if err != nil {
		t.Fatal(err)
	}
	if r.Shots != 3 || !near(r.CutsPerMinute, 12) {
		t.Fatalf("got %d shots, %v cuts per minute; want 3 and 12", r.Shots, r.CutsPerMinute)
	}
}

func TestMeasureCountsPausesOfAtLeast180ms(t *testing.T) {
	r, err := Measure(DNA{DurationMs: 60000, SilenceGaps: gaps(179, 180, 600)})
	if err != nil {
		t.Fatal(err)
	}
	if !near(r.PausesPerMinute, 2) {
		t.Errorf("pauses per minute = %v, want 2", r.PausesPerMinute)
	}
	if want := 959.0 / 60000; !near(r.SilentShare, want) {
		t.Errorf("silent share = %v, want %v (every gap counts)", r.SilentShare, want)
	}
}

func TestMeasureRejectsAZeroDuration(t *testing.T) {
	if _, err := Measure(DNA{}); err == nil {
		t.Fatal("expected an error")
	}
}

func TestMeasureVisualEventsPerMinuteByKind(t *testing.T) {
	events := []VisualEvent{{Kind: "punch_in"}, {Kind: "punch_in"}, {Kind: "overlay_change"}}
	r, err := Measure(DNA{DurationMs: 30000, VisualEvents: events})
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]float64{"punch_in": 4, "overlay_change": 2}
	if !reflect.DeepEqual(r.VisualPerMinute, want) {
		t.Fatalf("got %v, want %v", r.VisualPerMinute, want)
	}
}

func TestMeasureTellsNoEventsFromNotMeasured(t *testing.T) {
	measured, _ := Measure(DNA{DurationMs: 30000, VisualEvents: []VisualEvent{}})
	if measured.VisualPerMinute == nil {
		t.Error("an empty visual_events list is measured: want a non-nil map")
	}
	unmeasured, _ := Measure(DNA{DurationMs: 30000})
	if unmeasured.VisualPerMinute != nil {
		t.Error("an absent visual_events list is not measured: want a nil map")
	}
}

func TestLoadDNAReadsOnlyWhatTheBlueprintUses(t *testing.T) {
	body := `{"schema_version": 3, "duration_ms": 30000, "fps": 30.0, "cuts": [{"frame_index": 120, "time_ms": 4000, "interval_ms": 4000, "scene_score": 0.9}],
	  "silence_gaps": [{"start_ms": 5000, "end_ms": 5400, "duration_ms": 400}], "words_per_minute": 150.5, "integrated_lufs": -16.2,
	  "visual_events": [{"time_ms": 8000, "frame_index": 240, "kind": "overlay_change", "scale": null, "area": 0.3}], "unknown": 1}`
	got := loadFrom(t, body)
	if got.DurationMs != 30000 || len(got.Cuts) != 1 || got.Cuts[0].TimeMs != 4000 {
		t.Fatalf("got %+v", got)
	}
	if got.SilenceGaps[0].DurationMs != 400 || got.WordsPerMinute != 150.5 || got.IntegratedLUFS != -16.2 {
		t.Fatalf("got %+v", got)
	}
	if len(got.VisualEvents) != 1 || got.VisualEvents[0].Kind != "overlay_change" {
		t.Fatalf("visual events: %+v", got.VisualEvents)
	}
}

// minimal is a valid edit_dna.json with only the fields the blueprint requires.
const minimal = `"schema_version": 2, "duration_ms": 1000, "cuts": [], "silence_gaps": [], "words_per_minute": 0, "integrated_lufs": -70`

func TestLoadDNAWithoutVisualEvents(t *testing.T) {
	if got := loadFrom(t, `{`+minimal+`}`); got.VisualEvents != nil {
		t.Errorf("absent: got %v", got.VisualEvents)
	}
	if got := loadFrom(t, `{`+minimal+`, "visual_events": null}`); got.VisualEvents != nil {
		t.Errorf("null: got %v", got.VisualEvents)
	}
	if got := loadFrom(t, `{`+minimal+`, "visual_events": []}`); got.VisualEvents == nil {
		t.Error("empty list: got nil")
	}
}

func TestLoadDNAErrors(t *testing.T) {
	dir := t.TempDir()
	if _, err := LoadDNA(filepath.Join(dir, "absent.json")); err == nil {
		t.Error("a missing file must fail")
	}
	bad := filepath.Join(dir, "bad.json")
	if err := os.WriteFile(bad, []byte("{"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadDNA(bad); err == nil {
		t.Error("invalid JSON must fail")
	}
}

func loadFrom(t *testing.T, body string) DNA {
	t.Helper()
	path := filepath.Join(t.TempDir(), "dna.json")
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	dna, err := LoadDNA(path)
	if err != nil {
		t.Fatal(err)
	}
	return dna
}

func reelWithWPM(wpm float64) Reel {
	return Reel{Shots: 5, MeanShotMs: 2000, MedianShotMs: 2000, CutsPerMinute: 10, WordsPerMinute: wpm, LUFS: -14, ASRBackend: BackendWhisper}
}

func compareWPM(before, after float64) Blueprint {
	return Compare([]Reel{reelWithWPM(before), reelWithWPM(before)}, []Reel{reelWithWPM(after), reelWithWPM(after)})
}

func TestSentenceThresholdIsTwentyFivePercentInBothDirections(t *testing.T) {
	cases := []struct {
		before, after float64
		want          string
	}{
		{100, 125, "Words per minute rose from 100.0 to 125.0 (+25%) across 2 breakouts against 2 baseline posts."},
		{100, 124, ""},
		{100, 75, "Words per minute dropped from 100.0 to 75.0 (−25%) across 2 breakouts against 2 baseline posts."},
		{100, 76, ""},
	}
	for _, c := range cases {
		bp := compareWPM(c.before, c.after)
		if c.want == "" {
			if len(bp.Sentences) != 0 {
				t.Errorf("%v to %v: unexpected sentences %v", c.before, c.after, bp.Sentences)
			}
			continue
		}
		if !reflect.DeepEqual(bp.Sentences, []string{c.want}) {
			t.Errorf("%v to %v: got %q, want %q", c.before, c.after, bp.Sentences, c.want)
		}
	}
}

func TestSignedPercent(t *testing.T) {
	cases := map[float64]string{0.25: "+25%", -0.25: "−25%", 2.054: "+205%", -0.0046: "0%", 0.0046: "0%", 0: "0%"}
	for change, want := range cases {
		if got := SignedPercent(change); got != want {
			t.Errorf("SignedPercent(%v) = %q, want %q", change, got, want)
		}
	}
}

func TestEveryMetricIsListedWhetherOrNotItChanged(t *testing.T) {
	bp := compareWPM(100, 101)
	var keys []string
	for _, m := range bp.Metrics {
		keys = append(keys, m.Key)
	}
	want := []string{"mean_shot_length_ms", "median_shot_length_ms", "cuts_per_minute", "pauses_per_minute", "silent_share", "words_per_minute", "integrated_lufs"}
	if !reflect.DeepEqual(keys, want) {
		t.Fatalf("got %v, want %v", keys, want)
	}
	if bp.ChangeThreshold != 0.25 || bp.Breakouts != 2 || bp.Baseline != 2 {
		t.Fatalf("got %+v", bp)
	}
}

func TestGroupValueIsTheMeanOfThePerReelValues(t *testing.T) {
	bp := Compare([]Reel{reelWithWPM(100), reelWithWPM(200)}, []Reel{reelWithWPM(300), reelWithWPM(300), reelWithWPM(300)})
	m := find(t, bp, "words_per_minute")
	if !near(m.Before, 150) || !near(m.After, 300) || m.Change == nil || !near(*m.Change, 1) {
		t.Fatalf("got %+v", m)
	}
	if bp.Baseline != 2 || bp.Breakouts != 3 {
		t.Fatalf("counts: %+v", bp)
	}
}

func find(t *testing.T, bp Blueprint, key string) Metric {
	t.Helper()
	for _, m := range bp.Metrics {
		if m.Key == key {
			return m
		}
	}
	t.Fatalf("no metric %q in %+v", key, bp.Metrics)
	return Metric{}
}

func TestShotLengthSentenceReadsLikeTheSpecExample(t *testing.T) {
	before := Reel{MeanShotMs: 3900, MedianShotMs: 3900}
	after := Reel{MeanShotMs: 1400, MedianShotMs: 3900}
	bp := Compare([]Reel{before, before, before, before}, []Reel{after, after, after, after, after})
	want := []string{"Mean shot length dropped from 3.9 s to 1.4 s (−64%) across 5 breakouts against 4 baseline posts."}
	if !reflect.DeepEqual(bp.Sentences, want) {
		t.Fatalf("got %q, want %q", bp.Sentences, want)
	}
}

func TestSilentShareIsShownAsAPercentAndLoudnessInLUFS(t *testing.T) {
	before := Reel{SilentShare: 0.12, LUFS: -16}
	after := Reel{SilentShare: 0.04, LUFS: -10}
	bp := Compare([]Reel{before, before}, []Reel{after, after})
	want := []string{
		"Silent share dropped from 12.0% to 4.0% (−67%) across 2 breakouts against 2 baseline posts.",
		"Integrated loudness rose from -16.0 LUFS to -10.0 LUFS (+38%) across 2 breakouts against 2 baseline posts.",
	}
	if !reflect.DeepEqual(bp.Sentences, want) {
		t.Fatalf("got %q, want %q", bp.Sentences, want)
	}
}

func TestVisualEventsAreComparedPerKindInSortedOrder(t *testing.T) {
	before := Reel{VisualPerMinute: map[string]float64{"punch_in": 2}}
	after := Reel{VisualPerMinute: map[string]float64{"punch_in": 6, "overlay_change": 3}}
	bp := Compare([]Reel{before, before}, []Reel{after, after})
	var keys []string
	for _, m := range bp.Metrics {
		if strings.HasPrefix(m.Key, "visual_events_per_minute.") {
			keys = append(keys, m.Key)
		}
	}
	if want := []string{"visual_events_per_minute.overlay_change", "visual_events_per_minute.punch_in"}; !reflect.DeepEqual(keys, want) {
		t.Fatalf("got %v, want %v", keys, want)
	}
	overlay := find(t, bp, "visual_events_per_minute.overlay_change")
	if overlay.Change != nil || overlay.Before != 0 || overlay.After != 3 {
		t.Fatalf("a kind with no baseline events has no percentage: %+v", overlay)
	}
	want := []string{
		"Visual events per minute (overlay_change) rose from 0.0 to 3.0 across 2 breakouts against 2 baseline posts.",
		"Visual events per minute (punch_in) rose from 2.0 to 6.0 (+200%) across 2 breakouts against 2 baseline posts.",
	}
	if !reflect.DeepEqual(bp.Sentences, want) {
		t.Fatalf("got %q, want %q", bp.Sentences, want)
	}
}

func TestVisualEventsAreLeftOutWhenAnyReelDidNotMeasureThem(t *testing.T) {
	measured := Reel{ASRBackend: BackendWhisper, VisualPerMinute: map[string]float64{"punch_in": 2}}
	old := Reel{ASRBackend: BackendWhisper}
	bp := Compare([]Reel{old, measured}, []Reel{measured, measured, measured})
	for _, m := range bp.Metrics {
		if strings.HasPrefix(m.Key, "visual_events_per_minute") {
			t.Fatalf("metric %q compared though 1 of 5 files has no visual_events", m.Key)
		}
	}
	want := []string{"visual_events was missing from 1 of 5 files, so visual events are not compared."}
	if !reflect.DeepEqual(bp.Notes, want) {
		t.Fatalf("got %q, want %q", bp.Notes, want)
	}
}

func TestAnOldDataSetStillSaysWhyVisualEventsAreAbsent(t *testing.T) {
	r := Reel{ASRBackend: BackendWhisper}
	bp := Compare([]Reel{r, r}, []Reel{r, r})
	want := []string{"visual_events was missing from 4 of 4 files, so visual events are not compared."}
	if !reflect.DeepEqual(bp.Notes, want) {
		t.Fatalf("got %q, want %q", bp.Notes, want)
	}
}
