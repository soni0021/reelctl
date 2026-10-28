package digest

import (
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/soni0021/reelctl-watchtower/internal/niche"
	"github.com/soni0021/reelctl-watchtower/internal/reelctl"
)

var firstDay = time.Date(2026, 1, 1, 10, 0, 0, 0, time.UTC)

// post is named prefix+position (c1, c2, ...), one a day. A post with media
// has the path media/<id>.mp4.
type spec struct {
	views []int64
	media bool
}

func creator(handle string, s spec) niche.Creator {
	c := niche.Creator{Handle: handle}
	for i, v := range s.views {
		p := niche.Post{ID: fmt.Sprintf("%s%d", handle, i+1), PostedAt: firstDay.AddDate(0, 0, i), Views: v}
		if s.media {
			p.Media = "media/" + p.ID + ".mp4"
		}
		c.Posts = append(c.Posts, p)
	}
	return c
}

func snapshots(followers ...int64) []niche.Snapshot {
	var out []niche.Snapshot
	for i, f := range followers {
		day := time.Date(2026, 10, 15+i, 0, 0, 0, 0, time.UTC)
		out = append(out, niche.Snapshot{Date: day, Followers: f, TotalViews: f * 10})
	}
	return out
}

func repeat(v int64, n int) []int64 {
	out := make([]int64, n)
	for i := range out {
		out[i] = v
	}
	return out
}

func join(parts ...[]int64) []int64 {
	var out []int64
	for _, p := range parts {
		out = append(out, p...)
	}
	return out
}

// fakeDecompiler writes a small edit_dna.json for each media file, named by
// the post id, with the words per minute in wpm (150 when absent).
type fakeDecompiler struct {
	wpm   map[string]float64
	fail  map[string]error
	bad   map[string]bool
	calls []string
}

func (f *fakeDecompiler) Decompile(input, output string) error {
	id := strings.TrimSuffix(filepath.Base(input), ".mp4")
	f.calls = append(f.calls, id)
	if err := f.fail[id]; err != nil {
		return err
	}
	if f.bad[id] {
		return os.WriteFile(output, []byte("{"), 0o644)
	}
	wpm, ok := f.wpm[id]
	if !ok {
		wpm = 150
	}
	body := fmt.Sprintf(`{"duration_ms": 30000, "cuts": [{"time_ms": 10000}, {"time_ms": 20000}], "silence_gaps": [], "words_per_minute": %v, "integrated_lufs": -14}`, wpm)
	return os.WriteFile(output, []byte(body), 0o644)
}

func opts(t *testing.T) Options {
	return Options{MinScore: 10, MinVelocity: 0.02, GeneratedAt: "2026-10-28T06:00:00Z", OutDir: t.TempDir()}
}

func build(t *testing.T, o Options, d Decompiler, creators ...niche.Creator) Digest {
	t.Helper()
	got, err := Build(niche.Niche{Name: "n", Creators: creators}, o, d)
	if err != nil {
		t.Fatal(err)
	}
	return got
}

func find(t *testing.T, d Digest, handle string) Creator {
	t.Helper()
	for _, c := range d.Creators {
		if c.Handle == handle {
			return c
		}
	}
	t.Fatalf("no creator %q", handle)
	return Creator{}
}

func TestPostsWithTooLittleHistoryAreListedWithTheirCount(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{}, creator("c", spec{views: repeat(100, 6)}))
	c := find(t, d, "c")
	want := []Skip{
		{Post: "c1", Reason: "not_enough_history", Detail: "0 of 5 earlier posts needed"},
		{Post: "c2", Reason: "not_enough_history", Detail: "1 of 5 earlier posts needed"},
		{Post: "c3", Reason: "not_enough_history", Detail: "2 of 5 earlier posts needed"},
		{Post: "c4", Reason: "not_enough_history", Detail: "3 of 5 earlier posts needed"},
		{Post: "c5", Reason: "not_enough_history", Detail: "4 of 5 earlier posts needed"},
		{Reason: "velocity_not_computed", Detail: "no snapshots"},
	}
	if !reflect.DeepEqual(c.Skipped, want) {
		t.Fatalf("got %+v", c.Skipped)
	}
	if c.BestMultiplier == nil || *c.BestMultiplier != 1 || c.BestPostID != "c6" {
		t.Fatalf("best: %v %q", c.BestMultiplier, c.BestPostID)
	}
}

func TestAZeroMedianIsListedNotDivided(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{}, creator("c", spec{views: join(repeat(0, 5), []int64{50})}))
	c := find(t, d, "c")
	want := Skip{Post: "c6", Reason: "zero_median", Detail: "the median views of 5 earlier posts is zero"}
	if !containsSkip(c.Skipped, want) || c.BestMultiplier != nil {
		t.Fatalf("got %+v best %v", c.Skipped, c.BestMultiplier)
	}
}

func TestMultiplierExactlyAtMinScoreIsSurging(t *testing.T) {
	at := build(t, opts(t), &fakeDecompiler{}, creator("c", spec{views: join(repeat(100, 5), []int64{1000})}))
	if c := find(t, at, "c"); !c.Surging || !reflect.DeepEqual(c.SurgingBy, []string{"multiplier"}) {
		t.Errorf("a multiplier of exactly 10: %+v", c)
	}
	under := build(t, opts(t), &fakeDecompiler{}, creator("c", spec{views: join(repeat(100, 5), []int64{999})}))
	if c := find(t, under, "c"); c.Surging || len(c.SurgingBy) != 0 {
		t.Errorf("a multiplier of 9.99: %+v", c)
	}
}

func withSnapshots(c niche.Creator, followers ...int64) niche.Creator {
	c.Snapshots = snapshots(followers...)
	return c
}

func TestVelocityMustBeStrictlyAboveMinVelocity(t *testing.T) {
	flat := spec{views: repeat(100, 6)}
	at := build(t, opts(t), &fakeDecompiler{}, withSnapshots(creator("c", flat), 1000, 1020, 1040))
	if c := find(t, at, "c"); c.Surging {
		t.Errorf("2.0%% a day against a 2%% bar must not surge: %+v", c.Velocity)
	}
	over := build(t, opts(t), &fakeDecompiler{}, withSnapshots(creator("c", flat), 1000, 1021, 1042))
	c := find(t, over, "c")
	if !c.Surging || !reflect.DeepEqual(c.SurgingBy, []string{"velocity"}) {
		t.Errorf("2.1%% a day: %+v", c)
	}
	if c.Velocity == nil || c.Velocity.Snapshots != 3 || c.Velocity.FollowersPerDay != 21 || c.Velocity.FollowersShare != 0.021 {
		t.Errorf("velocity: %+v", c.Velocity)
	}
	if c.Velocity.ViewsPerDay != 210 || c.Velocity.ViewsShare != 0.021 {
		t.Errorf("views velocity: %+v", c.Velocity)
	}
}

func TestVelocityWithTwoSnapshotsIsNotComputed(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{}, withSnapshots(creator("c", spec{views: repeat(100, 6)}), 1000, 2000))
	c := find(t, d, "c")
	want := Skip{Reason: "velocity_not_computed", Detail: "2 snapshots in the 7 days ending 2026-10-16, need 3"}
	if c.Velocity != nil || c.Surging || !containsSkip(c.Skipped, want) {
		t.Fatalf("got %+v %+v", c.Velocity, c.Skipped)
	}
}

func TestVelocityWithNoSnapshotsIsNotComputed(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{}, creator("c", spec{views: repeat(100, 6)}))
	want := Skip{Reason: "velocity_not_computed", Detail: "no snapshots"}
	if !containsSkip(find(t, d, "c").Skipped, want) {
		t.Fatalf("got %+v", find(t, d, "c").Skipped)
	}
}

func containsSkip(skips []Skip, want Skip) bool {
	for _, s := range skips {
		if s == want {
			return true
		}
	}
	return false
}

// surgerSpec: five old posts at 100, two baseline posts at 100, then
// breakouts at 5000. Every post after the fifth has media.
func surgerSpec(breakouts int) niche.Creator {
	c := creator("k", spec{views: join(repeat(100, 7), repeat(5000, breakouts)), media: true})
	for i := 0; i < 5; i++ {
		c.Posts[i].Media = ""
	}
	return c
}

func TestADecompileFailureIsListedAndTheRunGoesOn(t *testing.T) {
	dec := &fakeDecompiler{fail: map[string]error{"k9": &reelctl.RunError{ExitCode: 7, Stderr: "boom"}}}
	d := build(t, opts(t), dec, surgerSpec(3))
	wantFailure := Failure{Creator: "k", Post: "k9", Stage: "decompile", ExitCode: 7, Detail: "boom"}
	if !reflect.DeepEqual(d.Failures, []Failure{wantFailure}) {
		t.Fatalf("failures: %+v", d.Failures)
	}
	if !reflect.DeepEqual(dec.calls, []string{"k6", "k7", "k8", "k9", "k10"}) {
		t.Fatalf("calls: %v", dec.calls)
	}
	c := find(t, d, "k")
	if c.Blueprint == nil || c.Blueprint.Breakouts != 2 || c.Blueprint.Baseline != 2 {
		t.Fatalf("blueprint: %+v", c.Blueprint)
	}
	if len(c.Breakouts) != 3 || c.Breakouts[1].EditDNA != "" || c.Breakouts[0].EditDNA != "dna/k8.edit_dna.json" {
		t.Fatalf("breakouts: %+v", c.Breakouts)
	}
}

func TestAnUnreadableEditDNAIsAFailure(t *testing.T) {
	dec := &fakeDecompiler{bad: map[string]bool{"k8": true}}
	d := build(t, opts(t), dec, surgerSpec(3))
	if len(d.Failures) != 1 || d.Failures[0].Stage != "read_edit_dna" || d.Failures[0].Post != "k8" || d.Failures[0].Detail == "" {
		t.Fatalf("failures: %+v", d.Failures)
	}
}

func TestOnlyTheLastFiveBreakoutsAreCompared(t *testing.T) {
	dec := &fakeDecompiler{wpm: map[string]float64{"k8": 1000, "k9": 1000}}
	for _, id := range []string{"k10", "k11", "k12", "k13", "k14"} {
		dec.wpm[id] = 200
	}
	dec.wpm["k6"], dec.wpm["k7"] = 100, 100
	d := build(t, opts(t), dec, surgerSpec(7))
	c := find(t, d, "k")
	if len(c.Breakouts) != 7 || len(dec.calls) != 9 {
		t.Fatalf("%d breakouts, %d decompiles; want 7 and 9", len(c.Breakouts), len(dec.calls))
	}
	if c.Blueprint.Breakouts != 5 {
		t.Fatalf("compared %d breakouts, want 5", c.Blueprint.Breakouts)
	}
	for _, m := range c.Blueprint.Metrics {
		if m.Key == "words_per_minute" && (m.Before != 100 || m.After != 200) {
			t.Fatalf("words per minute %v to %v; want 100 to 200", m.Before, m.After)
		}
	}
}

func TestTooFewDecompiledPostsMeansNoBlueprint(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{}, surgerSpec(1))
	c := find(t, d, "k")
	want := Skip{Reason: "blueprint_not_computed", Detail: "1 decompiled breakouts and 2 decompiled baseline posts, need 2 of each"}
	if c.Blueprint != nil || !containsSkip(c.Skipped, want) {
		t.Fatalf("blueprint %+v skipped %+v", c.Blueprint, c.Skipped)
	}
}

func TestPostsWithoutMediaAreListedWhenTheyCouldHaveBeenDecompiled(t *testing.T) {
	c := surgerSpec(2)
	c.Posts[6].Media = ""
	d := build(t, opts(t), &fakeDecompiler{}, c)
	want := Skip{Post: "k7", Reason: "no_media", Detail: "the post has no media_path, so it cannot be decompiled"}
	if !containsSkip(find(t, d, "k").Skipped, want) {
		t.Fatalf("got %+v", find(t, d, "k").Skipped)
	}
}

func TestACreatorWhoIsNotSurgingIsNotDecompiled(t *testing.T) {
	dec := &fakeDecompiler{}
	build(t, opts(t), dec, creator("c", spec{views: repeat(100, 8), media: true}))
	if len(dec.calls) != 0 {
		t.Fatalf("decompiled %v for a creator who is not surging", dec.calls)
	}
}

func TestBaselineIsEveryPostUnderTwiceTheMedian(t *testing.T) {
	views := join(repeat(100, 5), []int64{100, 199, 200, 5000})
	d := build(t, opts(t), &fakeDecompiler{}, creator("c", spec{views: views}))
	if got := find(t, d, "c").BaselinePosts; got != 2 {
		t.Fatalf("baseline posts = %d, want 2 (100 and 199 are under 2x; 200 is not)", got)
	}
}

func TestCreatorsAreRankedSurgingFirstThenByMultiplierThenVelocity(t *testing.T) {
	rising := withSnapshots(creator("rising", spec{views: repeat(100, 6)}), 1000, 1100, 1200)
	creators := []niche.Creator{
		creator("flat", spec{views: repeat(100, 6)}),
		creator("big", spec{views: join(repeat(100, 5), []int64{3000})}),
		creator("new", spec{views: repeat(100, 2)}),
		rising,
		creator("huge", spec{views: join(repeat(100, 5), []int64{9000})}),
	}
	d := build(t, opts(t), &fakeDecompiler{}, creators...)
	var order []string
	for _, c := range d.Creators {
		order = append(order, c.Handle)
	}
	if want := []string{"huge", "big", "rising", "flat", "new"}; !reflect.DeepEqual(order, want) {
		t.Fatalf("got %v, want %v", order, want)
	}
}

func TestBuildCopiesTheRunParameters(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{})
	if d.GeneratedAt != "2026-10-28T06:00:00Z" || d.Niche != "n" || d.MinScore != 10 || d.MinVelocity != 0.02 {
		t.Fatalf("got %+v", d)
	}
	if d.Creators == nil || d.Failures == nil {
		t.Fatal("empty lists must be [] in JSON, not null")
	}
}
