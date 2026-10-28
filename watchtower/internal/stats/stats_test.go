package stats

import (
	"errors"
	"math"
	"reflect"
	"testing"
	"time"
)

func near(a, b float64) bool { return math.Abs(a-b) < 1e-9 }

func TestMedian(t *testing.T) {
	cases := []struct {
		name string
		in   []float64
		want float64
	}{
		{"odd count", []float64{3, 1, 2}, 2},
		{"even count averages the two middle values", []float64{4, 1, 3, 2}, 2.5},
		{"ties", []float64{5, 5, 1, 5}, 5},
		{"one value", []float64{7}, 7},
		{"outlier does not move it", []float64{1, 1, 1, 1, 100}, 1},
	}
	for _, c := range cases {
		got, ok := Median(c.in)
		if !ok || !near(got, c.want) {
			t.Errorf("%s: got %v, %v; want %v", c.name, got, ok, c.want)
		}
	}
}

func TestMedianOfNothing(t *testing.T) {
	if _, ok := Median(nil); ok {
		t.Fatal("median of no values must report false")
	}
}

func TestMedianLeavesInputOrder(t *testing.T) {
	in := []float64{3, 1, 2}
	Median(in)
	if !reflect.DeepEqual(in, []float64{3, 1, 2}) {
		t.Fatalf("input was reordered: %v", in)
	}
}

func TestMultiplierNeedsFiveEarlierPosts(t *testing.T) {
	if _, err := Multiplier([]float64{10, 20, 30, 40}, 90); !errors.Is(err, ErrNotEnoughHistory) {
		t.Fatalf("4 earlier posts: got %v, want ErrNotEnoughHistory", err)
	}
	got, err := Multiplier([]float64{10, 20, 30, 40, 50}, 90)
	if err != nil || !near(got, 3) {
		t.Fatalf("5 earlier posts: got %v, %v; want 3", got, err)
	}
}

func TestMultiplierUsesTheLastThirtyPosts(t *testing.T) {
	earlier := []float64{1}
	for i := 0; i < 15; i++ {
		earlier = append(earlier, 100)
	}
	for i := 0; i < 15; i++ {
		earlier = append(earlier, 200)
	}
	got, err := Multiplier(earlier, 300)
	if err != nil || !near(got, 2) {
		t.Fatalf("got %v, %v; want 2 (median 150 of the last 30)", got, err)
	}
}

func TestMultiplierWithAZeroMedian(t *testing.T) {
	if _, err := Multiplier([]float64{0, 0, 0, 0, 0}, 50); !errors.Is(err, ErrZeroMedian) {
		t.Fatalf("got %v, want ErrZeroMedian", err)
	}
}

func TestSlopeOfALine(t *testing.T) {
	got, ok := Slope([]float64{0, 1, 2, 3}, []float64{1, 3, 5, 7})
	if !ok || !near(got, 2) {
		t.Fatalf("got %v, %v; want 2", got, ok)
	}
}

func TestSlopeWithNoise(t *testing.T) {
	got, ok := Slope([]float64{0, 1, 2, 3}, []float64{0, 1, 1, 3})
	if !ok || !near(got, 0.9) {
		t.Fatalf("got %v, %v; want 0.9", got, ok)
	}
}

func TestSlopeNeedsTwoDistinctPoints(t *testing.T) {
	if _, ok := Slope([]float64{1}, []float64{1}); ok {
		t.Error("one point must report false")
	}
	if _, ok := Slope([]float64{2, 2}, []float64{1, 3}); ok {
		t.Error("equal x values must report false")
	}
}

func day(s string) time.Time {
	d, err := time.Parse("2006-01-02", s)
	if err != nil {
		panic(err)
	}
	return d
}

func TestWindowKeepsTheSevenDaysEndingAtTheLatestSnapshot(t *testing.T) {
	var samples []Sample
	for _, d := range []string{"2026-10-21", "2026-10-14", "2026-10-17", "2026-10-15", "2026-10-20", "2026-10-16", "2026-10-19", "2026-10-18"} {
		samples = append(samples, Sample{Day: day(d), Value: 1})
	}
	got := Window(samples)
	if len(got) != 7 {
		t.Fatalf("got %d samples, want 7", len(got))
	}
	if !got[0].Day.Equal(day("2026-10-15")) || !got[6].Day.Equal(day("2026-10-21")) {
		t.Fatalf("window runs %v to %v, want 10-15 to 10-21", got[0].Day, got[6].Day)
	}
}

func TestWindowOfNothing(t *testing.T) {
	if got := Window(nil); len(got) != 0 {
		t.Fatalf("got %v", got)
	}
}

func TestGrowthRateOfALine(t *testing.T) {
	window := []Sample{{day("2026-10-15"), 100}, {day("2026-10-16"), 110}, {day("2026-10-17"), 120}, {day("2026-10-18"), 130}}
	got, err := GrowthRate(window)
	if err != nil || !near(got.PerDay, 10) || !near(got.Share, 0.1) {
		t.Fatalf("got %+v, %v; want 10 a day, share 0.1", got, err)
	}
}

func TestGrowthRateUsesRealDayGaps(t *testing.T) {
	window := []Sample{{day("2026-10-15"), 100}, {day("2026-10-17"), 120}, {day("2026-10-19"), 140}}
	got, err := GrowthRate(window)
	if err != nil || !near(got.PerDay, 10) {
		t.Fatalf("got %+v, %v; want 10 a day", got, err)
	}
}

func TestGrowthRateNeedsThreeSnapshots(t *testing.T) {
	window := []Sample{{day("2026-10-15"), 100}, {day("2026-10-16"), 110}}
	if _, err := GrowthRate(window); !errors.Is(err, ErrTooFewSnapshots) {
		t.Fatalf("got %v, want ErrTooFewSnapshots", err)
	}
}

func TestGrowthRateFromAZeroStart(t *testing.T) {
	window := []Sample{{day("2026-10-15"), 0}, {day("2026-10-16"), 10}, {day("2026-10-17"), 20}}
	if _, err := GrowthRate(window); !errors.Is(err, ErrZeroStart) {
		t.Fatalf("got %v, want ErrZeroStart", err)
	}
}

func TestRoundKeepsFourDecimals(t *testing.T) {
	cases := []struct{ in, want float64 }{
		{26.087012345, 26.087},
		{23.80951, 23.8095},
		{-0.64814814, -0.6481},
		{3, 3},
	}
	for _, c := range cases {
		if got := Round(c.in); got != c.want {
			t.Errorf("Round(%v) = %v, want %v", c.in, got, c.want)
		}
	}
}

func ptr(v float64) *float64 { return &v }

func TestSurgeReasons(t *testing.T) {
	cases := []struct {
		name  string
		mult  *float64
		share *float64
		want  []string
	}{
		{"multiplier exactly at the bar counts", ptr(10), nil, []string{"multiplier"}},
		{"multiplier just under the bar", ptr(9.99), nil, nil},
		{"velocity exactly at the bar does not count", nil, ptr(0.02), nil},
		{"velocity just over the bar", nil, ptr(0.0201), []string{"velocity"}},
		{"multiplier alone is enough", ptr(12), ptr(0.001), []string{"multiplier"}},
		{"velocity alone is enough", ptr(1.1), ptr(0.05), []string{"velocity"}},
		{"both", ptr(12), ptr(0.05), []string{"multiplier", "velocity"}},
		{"neither computed", nil, nil, nil},
	}
	for _, c := range cases {
		got := SurgeReasons(c.mult, c.share, 10, 0.02)
		if !reflect.DeepEqual(got, c.want) {
			t.Errorf("%s: got %v, want %v", c.name, got, c.want)
		}
	}
}
