// Package stats holds the watchtower's calculations: median, outlier
// multiplier, growth rate and the surging rule. Nothing here reads files or
// the clock.
package stats

import (
	"errors"
	"math"
	"sort"
	"time"
)

const (
	// MinHistory is the fewest earlier posts a multiplier needs.
	MinHistory = 5
	// HistoryWindow is how many earlier posts the median covers.
	HistoryWindow = 30
	// MinSnapshots is the fewest snapshots a growth rate needs.
	MinSnapshots = 3
	// WindowDays is the length of the growth window.
	WindowDays = 7

	// ReasonMultiplier and ReasonVelocity name why a creator is surging.
	ReasonMultiplier = "multiplier"
	ReasonVelocity   = "velocity"
)

var (
	ErrNotEnoughHistory = errors.New("not enough earlier posts")
	ErrZeroMedian       = errors.New("the median of earlier views is zero")
	ErrTooFewSnapshots  = errors.New("too few snapshots")
	ErrZeroStart        = errors.New("the first snapshot value is zero")
	ErrSameDay          = errors.New("every snapshot has the same date")
)

// Sample is one dated value, such as a follower count.
type Sample struct {
	Day   time.Time
	Value float64
}

// Rate is a growth rate: the least-squares slope per day, and that slope as a
// share of the first value in the window.
type Rate struct {
	PerDay float64
	Share  float64
}

// Median returns the median of values, or false when there are none.
func Median(values []float64) (float64, bool) {
	if len(values) == 0 {
		return 0, false
	}
	sorted := append([]float64(nil), values...)
	sort.Float64s(sorted)
	mid := len(sorted) / 2
	if len(sorted)%2 == 1 {
		return sorted[mid], true
	}
	return (sorted[mid-1] + sorted[mid]) / 2, true
}

// Multiplier is views divided by the median of the last HistoryWindow earlier
// posts. earlier is in date order, oldest first.
func Multiplier(earlier []float64, views float64) (float64, error) {
	if len(earlier) < MinHistory {
		return 0, ErrNotEnoughHistory
	}
	if len(earlier) > HistoryWindow {
		earlier = earlier[len(earlier)-HistoryWindow:]
	}
	median, _ := Median(earlier)
	if median == 0 {
		return 0, ErrZeroMedian
	}
	return views / median, nil
}

// Slope is the least-squares slope of ys against xs. It reports false for
// fewer than two points or when every x is the same.
func Slope(xs, ys []float64) (float64, bool) {
	n := len(xs)
	if n < 2 || n != len(ys) {
		return 0, false
	}
	meanX, meanY := Mean(xs), Mean(ys)
	var sxy, sxx float64
	for i := range xs {
		sxy += (xs[i] - meanX) * (ys[i] - meanY)
		sxx += (xs[i] - meanX) * (xs[i] - meanX)
	}
	if sxx == 0 {
		return 0, false
	}
	return sxy / sxx, true
}

// Round keeps four decimals, so emitted numbers do not depend on the last
// bits of a floating-point sum.
func Round(v float64) float64 {
	return math.Round(v*1e4) / 1e4
}

// Mean is the average of values. Callers pass at least one value.
func Mean(values []float64) float64 {
	var sum float64
	for _, v := range values {
		sum += v
	}
	return sum / float64(len(values))
}

// Window returns the samples from the WindowDays days ending on the latest
// sample, oldest first. A sample exactly WindowDays before the latest is out.
func Window(samples []Sample) []Sample {
	if len(samples) == 0 {
		return nil
	}
	sorted := append([]Sample(nil), samples...)
	sort.SliceStable(sorted, func(i, j int) bool { return sorted[i].Day.Before(sorted[j].Day) })
	cutoff := sorted[len(sorted)-1].Day.AddDate(0, 0, -WindowDays)
	first := sort.Search(len(sorted), func(i int) bool { return sorted[i].Day.After(cutoff) })
	return sorted[first:]
}

// GrowthRate fits a line through a window of samples, oldest first.
func GrowthRate(window []Sample) (Rate, error) {
	if len(window) < MinSnapshots {
		return Rate{}, ErrTooFewSnapshots
	}
	start := window[0]
	if start.Value == 0 {
		return Rate{}, ErrZeroStart
	}
	xs := make([]float64, len(window))
	ys := make([]float64, len(window))
	for i, s := range window {
		xs[i] = s.Day.Sub(start.Day).Hours() / 24
		ys[i] = s.Value
	}
	slope, ok := Slope(xs, ys)
	if !ok {
		return Rate{}, ErrSameDay
	}
	return Rate{PerDay: slope, Share: slope / start.Value}, nil
}

// SurgeReasons lists why a creator is surging. A multiplier at or above
// minScore counts; a follower share per day strictly above minVelocity counts.
// A nil value was not computed and counts for nothing.
func SurgeReasons(multiplier, followerShare *float64, minScore, minVelocity float64) []string {
	var reasons []string
	if multiplier != nil && *multiplier >= minScore {
		reasons = append(reasons, ReasonMultiplier)
	}
	if followerShare != nil && *followerShare > minVelocity {
		reasons = append(reasons, ReasonVelocity)
	}
	return reasons
}
