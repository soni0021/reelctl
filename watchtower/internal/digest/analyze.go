package digest

import (
	"errors"
	"fmt"
	"sort"

	"github.com/soni0021/reelctl-watchtower/internal/niche"
	"github.com/soni0021/reelctl-watchtower/internal/stats"
)

// BaselineBelow is the multiplier under which a post counts as a baseline post.
const BaselineBelow = 2.0

const dateLayout = "2006-01-02"

type scoredPost struct {
	post       niche.Post
	multiplier float64
}

// analysis is what the numbers say about one creator, before any decompiling.
type analysis struct {
	scored    []scoredPost
	skipped   []Skip
	velocity  *Velocity
	surgingBy []string
}

func analyze(c niche.Creator, o Options) analysis {
	scored, skips := scorePosts(c.Posts)
	velocity, velocitySkip := growth(c.Snapshots)
	if velocitySkip != nil {
		skips = append(skips, *velocitySkip)
	}
	a := analysis{scored: scored, skipped: skips, velocity: velocity}
	a.surgingBy = stats.SurgeReasons(bestMultiplier(scored), followerShare(velocity), o.MinScore, o.MinVelocity)
	return a
}

func scorePosts(posts []niche.Post) ([]scoredPost, []Skip) {
	ordered := append([]niche.Post(nil), posts...)
	sort.SliceStable(ordered, func(i, j int) bool {
		if !ordered[i].PostedAt.Equal(ordered[j].PostedAt) {
			return ordered[i].PostedAt.Before(ordered[j].PostedAt)
		}
		return ordered[i].ID < ordered[j].ID
	})
	var scored []scoredPost
	skips := []Skip{}
	views := make([]float64, len(ordered))
	for i, p := range ordered {
		views[i] = float64(p.Views)
	}
	for i, p := range ordered {
		m, err := stats.Multiplier(views[:i], views[i])
		if err != nil {
			skips = append(skips, postSkip(p.ID, err, i))
			continue
		}
		scored = append(scored, scoredPost{post: p, multiplier: m})
	}
	return scored, skips
}

func postSkip(id string, err error, earlier int) Skip {
	if errors.Is(err, stats.ErrZeroMedian) {
		window := min(earlier, stats.HistoryWindow)
		return Skip{Post: id, Reason: ReasonZeroMedian, Detail: fmt.Sprintf("the median views of %d earlier posts is zero", window)}
	}
	return Skip{Post: id, Reason: ReasonNotEnoughHistory, Detail: fmt.Sprintf("%d of %d earlier posts needed", earlier, stats.MinHistory)}
}

func bestMultiplier(scored []scoredPost) *float64 {
	var best *float64
	for _, s := range scored {
		if best == nil || s.multiplier > *best {
			m := s.multiplier
			best = &m
		}
	}
	return best
}

func bestPost(scored []scoredPost) (scoredPost, bool) {
	var best scoredPost
	found := false
	for _, s := range scored {
		if !found || s.multiplier > best.multiplier {
			best, found = s, true
		}
	}
	return best, found
}

func followerShare(v *Velocity) *float64 {
	if v == nil {
		return nil
	}
	share := v.FollowersShare
	return &share
}

func growth(snapshots []niche.Snapshot) (*Velocity, *Skip) {
	if len(snapshots) == 0 {
		return nil, &Skip{Reason: ReasonVelocity, Detail: "no snapshots"}
	}
	followers := window(snapshots, func(s niche.Snapshot) float64 { return float64(s.Followers) })
	views := window(snapshots, func(s niche.Snapshot) float64 { return float64(s.TotalViews) })
	fr, ferr := stats.GrowthRate(followers)
	vr, verr := stats.GrowthRate(views)
	if err := errors.Join(ferr, verr); err != nil {
		return nil, &Skip{Reason: ReasonVelocity, Detail: velocityDetail(followers, ferr, verr)}
	}
	return &Velocity{
		Snapshots:       len(followers),
		FollowersPerDay: stats.Round(fr.PerDay),
		FollowersShare:  stats.Round(fr.Share),
		ViewsPerDay:     stats.Round(vr.PerDay),
		ViewsShare:      stats.Round(vr.Share),
	}, nil
}

func window(snapshots []niche.Snapshot, pick func(niche.Snapshot) float64) []stats.Sample {
	samples := make([]stats.Sample, len(snapshots))
	for i, s := range snapshots {
		samples[i] = stats.Sample{Day: s.Date, Value: pick(s)}
	}
	return stats.Window(samples)
}

func velocityDetail(followers []stats.Sample, errs ...error) string {
	if errors.Is(errors.Join(errs...), stats.ErrTooFewSnapshots) {
		latest := followers[len(followers)-1].Day.Format(dateLayout)
		return fmt.Sprintf("%d snapshots in the 7 days ending %s, need %d", len(followers), latest, stats.MinSnapshots)
	}
	return "the first snapshot in the window has zero followers or zero views"
}

// split sorts scored posts into breakouts (at or above minScore) and baseline
// posts (the rest under BaselineBelow), each in date order.
func split(scored []scoredPost, minScore float64) (breakouts, baseline []scoredPost) {
	for _, s := range scored {
		switch {
		case s.multiplier >= minScore:
			breakouts = append(breakouts, s)
		case s.multiplier < BaselineBelow:
			baseline = append(baseline, s)
		}
	}
	return breakouts, baseline
}
