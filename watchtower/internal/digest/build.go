package digest

import (
	"errors"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"sort"
	"time"

	"github.com/soni0021/reelctl-watchtower/internal/blueprint"
	"github.com/soni0021/reelctl-watchtower/internal/niche"
	"github.com/soni0021/reelctl-watchtower/internal/reelctl"
	"github.com/soni0021/reelctl-watchtower/internal/stats"
)

const (
	// RecentBreakouts is how many of the latest decompiled breakouts a blueprint covers.
	RecentBreakouts = 5
	// MinCompared is the fewest decompiled breakouts, and the fewest decompiled
	// baseline posts, a blueprint needs.
	MinCompared = 2

	dnaDir = "dna"
)

var unsafeName = regexp.MustCompile(`[^A-Za-z0-9._-]`)

// measured is a decompiled post.
type measured struct {
	scoredPost
	editDNA string
	reel    blueprint.Reel
}

type run struct {
	dec      Decompiler
	outDir   string
	failures []Failure
}

// Build scores every creator, decompiles the posts of the surging ones and
// compares their breakouts with their baseline posts. A failed decompile is
// recorded in the digest and does not stop the run. The error is for a problem
// that stops it, such as an output directory that cannot be made.
func Build(n niche.Niche, o Options, d Decompiler) (Digest, error) {
	if err := os.MkdirAll(filepath.Join(o.OutDir, dnaDir), 0o755); err != nil {
		return Digest{}, err
	}
	r := &run{dec: d, outDir: o.OutDir, failures: []Failure{}}
	out := Digest{
		GeneratedAt: o.GeneratedAt,
		Niche:       n.Name,
		MinScore:    o.MinScore,
		MinVelocity: o.MinVelocity,
		Creators:    []Creator{},
	}
	for _, c := range n.Creators {
		out.Creators = append(out.Creators, r.creator(c, o))
	}
	rank(out.Creators)
	out.Failures = r.failures
	return out, nil
}

func (r *run) creator(c niche.Creator, o Options) Creator {
	a := analyze(c, o)
	breakouts, baseline := split(a.scored, o.MinScore)
	out := Creator{
		Handle:        c.Handle,
		Surging:       len(a.surgingBy) > 0,
		SurgingBy:     orEmpty(a.surgingBy),
		Velocity:      a.velocity,
		BaselinePosts: len(baseline),
		Skipped:       a.skipped,
	}
	if best, ok := bestPost(a.scored); ok {
		m := stats.Round(best.multiplier)
		out.BestMultiplier, out.BestPostID = &m, best.post.ID
	}
	if !out.Surging {
		out.Breakouts = []Breakout{}
		return out
	}
	done, noMedia := r.decompileAll(c.Handle, a.scored, breakouts, baseline)
	out.Skipped = append(out.Skipped, noMedia...)
	out.Breakouts = breakoutRows(breakouts, done)
	out.Blueprint, out.Skipped = blueprintFor(done, breakouts, out.Skipped)
	return out
}

func orEmpty(s []string) []string {
	if s == nil {
		return []string{}
	}
	return s
}

// decompileAll decompiles the breakouts and baseline posts, in date order.
func (r *run) decompileAll(handle string, scored []scoredPost, breakouts, baseline []scoredPost) (map[string]measured, []Skip) {
	wanted := make(map[string]bool)
	for _, s := range append(append([]scoredPost(nil), breakouts...), baseline...) {
		wanted[s.post.ID] = true
	}
	done := make(map[string]measured)
	noMedia := []Skip{}
	for _, s := range scored {
		if !wanted[s.post.ID] {
			continue
		}
		if s.post.Media == "" {
			noMedia = append(noMedia, Skip{Post: s.post.ID, Reason: ReasonNoMedia, Detail: "the post has no media_path, so it cannot be decompiled"})
			continue
		}
		if m, ok := r.decompile(handle, s); ok {
			done[s.post.ID] = m
		}
	}
	return done, noMedia
}

func (r *run) decompile(handle string, s scoredPost) (measured, bool) {
	rel := path.Join(dnaDir, unsafeName.ReplaceAllString(s.post.ID, "_")+".edit_dna.json")
	if err := r.dec.Decompile(s.post.Media, filepath.Join(r.outDir, filepath.FromSlash(rel))); err != nil {
		r.failures = append(r.failures, decompileFailure(handle, s.post.ID, err))
		return measured{}, false
	}
	reel, err := readReel(filepath.Join(r.outDir, filepath.FromSlash(rel)))
	if err != nil {
		r.failures = append(r.failures, Failure{Creator: handle, Post: s.post.ID, Stage: StageReadDNA, Detail: err.Error()})
		return measured{}, false
	}
	return measured{scoredPost: s, editDNA: rel, reel: reel}, true
}

func readReel(file string) (blueprint.Reel, error) {
	dna, err := blueprint.LoadDNA(file)
	if err != nil {
		return blueprint.Reel{}, err
	}
	return blueprint.Measure(dna)
}

func decompileFailure(handle, id string, err error) Failure {
	f := Failure{Creator: handle, Post: id, Stage: StageDecompile, ExitCode: -1, Detail: err.Error()}
	var runErr *reelctl.RunError
	if errors.As(err, &runErr) {
		f.ExitCode, f.Detail = runErr.ExitCode, runErr.Stderr
	}
	return f
}

func breakoutRows(breakouts []scoredPost, done map[string]measured) []Breakout {
	rows := []Breakout{}
	for _, s := range breakouts {
		rows = append(rows, Breakout{
			ID:         s.post.ID,
			URL:        s.post.URL,
			PostedAt:   s.post.PostedAt.UTC().Format(time.RFC3339),
			Views:      s.post.Views,
			Multiplier: stats.Round(s.multiplier),
			EditDNA:    done[s.post.ID].editDNA,
		})
	}
	return rows
}

// blueprintFor compares the last RecentBreakouts decompiled breakouts with
// the decompiled baseline posts, or says why it cannot.
func blueprintFor(done map[string]measured, breakouts []scoredPost, skipped []Skip) (*blueprint.Blueprint, []Skip) {
	recent := decompiled(done, breakouts)
	if len(recent) > RecentBreakouts {
		recent = recent[len(recent)-RecentBreakouts:]
	}
	baseline := baselineReels(done, breakouts)
	if len(recent) < MinCompared || len(baseline) < MinCompared {
		detail := fmt.Sprintf("%d decompiled breakouts and %d decompiled baseline posts, need %d of each", len(recent), len(baseline), MinCompared)
		return nil, append(skipped, Skip{Reason: ReasonBlueprint, Detail: detail})
	}
	bp := blueprint.Compare(baseline, reels(recent))
	return &bp, skipped
}

func decompiled(done map[string]measured, posts []scoredPost) []measured {
	var out []measured
	for _, s := range posts {
		if m, ok := done[s.post.ID]; ok {
			out = append(out, m)
		}
	}
	return out
}

// baselineReels returns the decompiled posts that are not breakouts, oldest first.
func baselineReels(done map[string]measured, breakouts []scoredPost) []blueprint.Reel {
	isBreakout := make(map[string]bool)
	for _, s := range breakouts {
		isBreakout[s.post.ID] = true
	}
	var rest []measured
	for id, m := range done {
		if !isBreakout[id] {
			rest = append(rest, m)
		}
	}
	sort.Slice(rest, func(i, j int) bool { return before(rest[i].post, rest[j].post) })
	return reels(rest)
}

func before(a, b niche.Post) bool {
	if !a.PostedAt.Equal(b.PostedAt) {
		return a.PostedAt.Before(b.PostedAt)
	}
	return a.ID < b.ID
}

func reels(ms []measured) []blueprint.Reel {
	out := make([]blueprint.Reel, len(ms))
	for i, m := range ms {
		out[i] = m.reel
	}
	return out
}

// rank puts surging creators first, then orders by best multiplier, then by
// follower growth, then by handle. A creator without a number comes last.
func rank(creators []Creator) {
	sort.SliceStable(creators, func(i, j int) bool {
		a, b := creators[i], creators[j]
		if a.Surging != b.Surging {
			return a.Surging
		}
		if x, y := multiplierOf(a), multiplierOf(b); x != y {
			return x > y
		}
		if x, y := shareOf(a), shareOf(b); x != y {
			return x > y
		}
		return a.Handle < b.Handle
	})
}

func multiplierOf(c Creator) float64 {
	if c.BestMultiplier == nil {
		return -1
	}
	return *c.BestMultiplier
}

func shareOf(c Creator) float64 {
	if c.Velocity == nil {
		return -1
	}
	return c.Velocity.FollowersShare
}
