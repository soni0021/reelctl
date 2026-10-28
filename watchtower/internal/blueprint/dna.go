// Package blueprint measures reels from edit_dna.json and compares a creator's
// recent breakouts with their baseline posts. It reads only measured fields
// and never claims anything edit_dna.json does not record.
package blueprint

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"sort"

	"github.com/soni0021/reelctl-watchtower/internal/stats"
)

// PauseMinMs is the shortest silence gap that counts as a pause.
const PauseMinMs = 180

const msPerMinute = 60000.0

// Cut is the part of a cut the blueprint reads.
type Cut struct {
	TimeMs uint64 `json:"time_ms"`
}

// SilenceGap is the part of a silence gap the blueprint reads.
type SilenceGap struct {
	DurationMs uint64 `json:"duration_ms"`
}

// VisualEvent is the part of a visual event the blueprint reads.
type VisualEvent struct {
	Kind string `json:"kind"`
}

// DNA is the subset of edit_dna.json the blueprint reads. VisualEvents is nil
// when the file has no visual_events, which an older file does not: that means
// not measured, which is different from an empty list.
type DNA struct {
	DurationMs     uint64        `json:"duration_ms"`
	Cuts           []Cut         `json:"cuts"`
	SilenceGaps    []SilenceGap  `json:"silence_gaps"`
	WordsPerMinute float64       `json:"words_per_minute"`
	IntegratedLUFS float64       `json:"integrated_lufs"`
	VisualEvents   []VisualEvent `json:"visual_events"`
}

// Reel is what one edit_dna.json says about a reel's editing.
// VisualPerMinute is nil when visual events were not measured.
type Reel struct {
	Shots           int
	MeanShotMs      float64
	MedianShotMs    float64
	CutsPerMinute   float64
	PausesPerMinute float64
	SilentShare     float64
	WordsPerMinute  float64
	LUFS            float64
	VisualPerMinute map[string]float64
}

// LoadDNA reads an edit_dna.json file. Its errors do not name the path, so a
// digest that records one does not carry the machine's directories.
func LoadDNA(path string) (DNA, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		var pathErr *fs.PathError
		if errors.As(err, &pathErr) {
			return DNA{}, pathErr.Err
		}
		return DNA{}, err
	}
	var dna DNA
	if err := json.Unmarshal(data, &dna); err != nil {
		return DNA{}, fmt.Errorf("not a valid edit_dna.json: %w", err)
	}
	return dna, nil
}

// Measure turns one file into per-reel numbers. The first shot starts at 0 and
// the last ends at duration_ms; a cut at 0 or at or after the end is ignored.
func Measure(d DNA) (Reel, error) {
	if d.DurationMs == 0 {
		return Reel{}, errors.New("duration_ms is zero")
	}
	minutes := float64(d.DurationMs) / msPerMinute
	lengths := shotLengths(d.DurationMs, d.Cuts)
	median, _ := stats.Median(lengths)
	return Reel{
		Shots:           len(lengths),
		MeanShotMs:      stats.Mean(lengths),
		MedianShotMs:    median,
		CutsPerMinute:   float64(len(lengths)-1) / minutes,
		PausesPerMinute: float64(countPauses(d.SilenceGaps)) / minutes,
		SilentShare:     float64(totalSilence(d.SilenceGaps)) / float64(d.DurationMs),
		WordsPerMinute:  d.WordsPerMinute,
		LUFS:            d.IntegratedLUFS,
		VisualPerMinute: visualPerMinute(d.VisualEvents, minutes),
	}, nil
}

func shotLengths(duration uint64, cuts []Cut) []float64 {
	times := make([]uint64, 0, len(cuts))
	for _, c := range cuts {
		if c.TimeMs > 0 && c.TimeMs < duration {
			times = append(times, c.TimeMs)
		}
	}
	sort.Slice(times, func(i, j int) bool { return times[i] < times[j] })
	var lengths []float64
	start := uint64(0)
	for _, t := range times {
		if t == start {
			continue
		}
		lengths = append(lengths, float64(t-start))
		start = t
	}
	return append(lengths, float64(duration-start))
}

func countPauses(gaps []SilenceGap) int {
	n := 0
	for _, g := range gaps {
		if g.DurationMs >= PauseMinMs {
			n++
		}
	}
	return n
}

func totalSilence(gaps []SilenceGap) uint64 {
	var total uint64
	for _, g := range gaps {
		total += g.DurationMs
	}
	return total
}

func visualPerMinute(events []VisualEvent, minutes float64) map[string]float64 {
	if events == nil {
		return nil
	}
	counts := make(map[string]int)
	for _, e := range events {
		counts[e.Kind]++
	}
	perMinute := make(map[string]float64, len(counts))
	for kind, n := range counts {
		perMinute[kind] = float64(n) / minutes
	}
	return perMinute
}
