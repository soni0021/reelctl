package stats

import (
	"errors"
	"testing"
	"time"
)

func TestGrowthRateOfSnapshotsOnOneDayHasNoTimeSpan(t *testing.T) {
	day := time.Date(2026, 10, 21, 0, 0, 0, 0, time.UTC)
	window := []Sample{{day, 100}, {day, 110}, {day, 120}}
	_, err := GrowthRate(window)
	if !errors.Is(err, ErrSameDay) {
		t.Fatalf("got %v, want ErrSameDay", err)
	}
	if errors.Is(err, ErrTooFewSnapshots) {
		t.Fatal("three snapshots are not too few; the dates are the problem")
	}
}
