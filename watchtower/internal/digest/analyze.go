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
	velocity, velocitySkips := growth(c.Snapshots)
	skips = append(skips, velocitySkips...)
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

// growth fits followers and total views on their own, so a total_views series
// that cannot be fitted (a zero first value) does not take the follower
// velocity with it. A follower series that cannot be fitted gives no velocity,
// because the surging rule rests on it.
func growth(snapshots []niche.Snapshot) (*Velocity, []Skip) {
	if len(snapshots) == 0 {
		return nil, []Skip{{Reason: ReasonVelocity, Detail: "no snapshots"}}
	}
	followers := window(snapshots, func(s niche.Snapshot) float64 { return float64(s.Followers) })
	views := window(snapshots, func(s niche.Snapshot) float64 { return float64(s.TotalViews) })
	fr, ferr := stats.GrowthRate(followers)
	vr, verr := stats.GrowthRate(views)
	if isWindowError(ferr) {
		// The dates are the same for both series, so one skip says it.
		return nil, []Skip{{Reason: ReasonVelocity, Detail: windowDetail(followers, ferr)}}
	}
	var skips []Skip
	if verr != nil {
		skips = append(skips, Skip{Reason: ReasonVelocity, Detail: "total views: " + zeroStartDetail})
	}
	if ferr != nil {
		return nil, append([]Skip{{Reason: ReasonVelocity, Detail: "followers: " + zeroStartDetail}}, skips...)
	}
	v := &Velocity{
		Snapshots:       len(followers),
		FollowersPerDay: stats.Round(fr.PerDay),
		FollowersShare:  stats.Round(fr.Share),
	}
	if verr == nil {
		perDay, share := stats.Round(vr.PerDay), stats.Round(vr.Share)
		v.ViewsPerDay, v.ViewsShare = &perDay, &share
	}
	return v, skips
}

const zeroStartDetail = "the first snapshot in the window is zero"

func isWindowError(err error) bool {
	return errors.Is(err, stats.ErrTooFewSnapshots) || errors.Is(err, stats.ErrSameDay)
}

func window(snapshots []niche.Snapshot, pick func(niche.Snapshot) float64) []stats.Sample {
	samples := make([]stats.Sample, len(snapshots))
	for i, s := range snapshots {
		samples[i] = stats.Sample{Day: s.Date, Value: pick(s)}
	}
	return stats.Window(samples)
}

func windowDetail(window []stats.Sample, err error) string {
	latest := window[len(window)-1].Day.Format(dateLayout)
	if errors.Is(err, stats.ErrSameDay) {
		return fmt.Sprintf("all %d snapshots in the 7 days ending %s share one date, so there is no time span to fit a rate over", len(window), latest)
	}
	return fmt.Sprintf("%d snapshots in the 7 days ending %s, need %d", len(window), latest, stats.MinSnapshots)
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
