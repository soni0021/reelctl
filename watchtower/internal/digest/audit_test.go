package digest

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/soni0021/reelctl-watchtower/internal/niche"
	"github.com/soni0021/reelctl-watchtower/internal/reelctl"
)

// snapshotsOn builds snapshots on the given days of October 2026.
func snapshotsOn(days []int, followers []int64, totalViews []int64) []niche.Snapshot {
	var out []niche.Snapshot
	for i, d := range days {
		out = append(out, niche.Snapshot{Date: time.Date(2026, 10, d, 0, 0, 0, 0, time.UTC), Followers: followers[i], TotalViews: totalViews[i]})
	}
	return out
}

var flat = spec{views: repeat(100, 6)}

// Finding 5: a zero first total_views must not drop the follower velocity.
func TestAZeroFirstTotalViewsKeepsTheFollowerVelocity(t *testing.T) {
	c := creator("c", flat)
	c.Snapshots = snapshotsOn([]int{15, 16, 17}, []int64{1000, 1030, 1060}, []int64{0, 500, 1000})
	got := find(t, build(t, opts(t), &fakeDecompiler{}, c), "c")
	if got.Velocity == nil || got.Velocity.FollowersPerDay != 30 || got.Velocity.FollowersShare != 0.03 {
		t.Fatalf("velocity %+v", got.Velocity)
	}
	if got.Velocity.ViewsPerDay != nil || got.Velocity.ViewsShare != nil {
		t.Fatalf("views growth from a zero start must not be computed: %+v", got.Velocity)
	}
	if !got.Surging || !reflect.DeepEqual(got.SurgingBy, []string{"velocity"}) {
		t.Fatalf("3%% a day must surge: %+v", got)
	}
	want := Skip{Reason: "velocity_not_computed", Detail: "total views: the first snapshot in the window is zero"}
	if !containsSkip(got.Skipped, want) {
		t.Fatalf("skipped %+v", got.Skipped)
	}
}

func TestAZeroFirstFollowerCountDropsOnlyThatVelocity(t *testing.T) {
	c := creator("c", flat)
	c.Snapshots = snapshotsOn([]int{15, 16, 17}, []int64{0, 30, 60}, []int64{1000, 2000, 3000})
	got := find(t, build(t, opts(t), &fakeDecompiler{}, c), "c")
	if got.Velocity != nil || got.Surging {
		t.Fatalf("velocity %+v surging %v", got.Velocity, got.Surging)
	}
	want := Skip{Reason: "velocity_not_computed", Detail: "followers: the first snapshot in the window is zero"}
	if !containsSkip(got.Skipped, want) {
		t.Fatalf("skipped %+v", got.Skipped)
	}
}

// Finding 11.
func TestSnapshotsThatShareADateSayThatNotTooFew(t *testing.T) {
	c := creator("c", flat)
	c.Snapshots = snapshotsOn([]int{15, 15, 15}, []int64{1000, 1010, 1020}, []int64{1, 2, 3})
	got := find(t, build(t, opts(t), &fakeDecompiler{}, c), "c")
	want := Skip{Reason: "velocity_not_computed", Detail: "all 3 snapshots in the 7 days ending 2026-10-15 share one date, so there is no time span to fit a rate over"}
	if got.Velocity != nil || !containsSkip(got.Skipped, want) {
		t.Fatalf("velocity %+v skipped %+v", got.Velocity, got.Skipped)
	}
}

// Finding 8: snapshots older than the 7-day window must not count.
func TestSnapshotsOlderThanSevenDaysAreLeftOutOfTheVelocity(t *testing.T) {
	c := creator("c", flat)
	c.Snapshots = snapshotsOn([]int{1, 2, 3, 15, 16, 17}, []int64{100, 100, 100, 1000, 1020, 1040}, []int64{1, 1, 1, 10000, 10200, 10400})
	got := find(t, build(t, opts(t), &fakeDecompiler{}, c), "c")
	if got.Velocity == nil || got.Velocity.Snapshots != 3 || got.Velocity.FollowersPerDay != 20 || got.Velocity.FollowersShare != 0.02 {
		t.Fatalf("velocity %+v; only the last 3 snapshots are in the window", got.Velocity)
	}
	if got.Surging {
		t.Fatalf("2%% a day is not above the 2%% bar: %+v", got)
	}
}

func TestASnapshotExactlySevenDaysBeforeTheLatestIsOut(t *testing.T) {
	c := creator("c", flat)
	c.Snapshots = snapshotsOn([]int{10, 15, 16, 17}, []int64{1, 1000, 1020, 1040}, []int64{1, 10000, 10200, 10400})
	got := find(t, build(t, opts(t), &fakeDecompiler{}, c), "c")
	if got.Velocity == nil || got.Velocity.Snapshots != 3 {
		t.Fatalf("velocity %+v", got.Velocity)
	}
}

// Finding 8: a tie for the best post goes to the earlier post.
func TestATieForBestPostGoesToTheEarlierPost(t *testing.T) {
	c := creator("c", spec{views: join(repeat(100, 5), []int64{500, 500})})
	got := find(t, build(t, opts(t), &fakeDecompiler{}, c), "c")
	if got.BestPostID == nil || *got.BestPostID != "c6" || *got.BestMultiplier != 5 {
		t.Fatalf("best %v %v", got.BestPostID, got.BestMultiplier)
	}
}

// Finding 8: equal creators are ordered by handle, whatever order they came in.
func TestCreatorsThatTieAreOrderedByHandle(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{}, creator("b", flat), creator("c", flat), creator("a", flat))
	var order []string
	for _, c := range d.Creators {
		order = append(order, c.Handle)
	}
	if want := []string{"a", "b", "c"}; !reflect.DeepEqual(order, want) {
		t.Fatalf("got %v, want %v", order, want)
	}
}

func TestCreatorsThatTieOnMultiplierAreOrderedByFollowerGrowth(t *testing.T) {
	slow := withSnapshots(creator("a", flat), 1000, 1010, 1020)
	fast := withSnapshots(creator("b", flat), 1000, 1015, 1030)
	d := build(t, opts(t), &fakeDecompiler{}, slow, fast)
	if d.Creators[0].Handle != "b" {
		t.Fatalf("faster growth must come first: %v, %v", d.Creators[0].Handle, d.Creators[1].Handle)
	}
}

// Finding 8: a non-default --min-score changes who is a breakout and who surges.
func TestAnotherMinScoreMovesTheBreakoutLine(t *testing.T) {
	views := join(repeat(100, 5), []int64{250, 400})
	low := opts(t)
	low.MinScore = 3
	d := build(t, low, &fakeDecompiler{}, creator("c", spec{views: views}))
	got := find(t, d, "c")
	if !got.Surging || len(got.Breakouts) != 1 || got.Breakouts[0].ID != "c7" || got.Breakouts[0].Multiplier != 4 {
		t.Fatalf("min-score 3: %+v", got)
	}
	if got.BaselinePosts != 0 {
		t.Fatalf("a 2.5x post is neither a breakout nor a baseline post: baseline %d", got.BaselinePosts)
	}
	if d.MinScore != 3 {
		t.Fatalf("digest min_score %v", d.MinScore)
	}
	high := opts(t)
	high.MinScore = 20
	if got := find(t, build(t, high, &fakeDecompiler{}, creator("c", spec{views: views})), "c"); got.Surging || len(got.Breakouts) != 0 {
		t.Fatalf("min-score 20: %+v", got)
	}
}

// Finding 7: how a run ended is kept apart.
func TestFailuresKeepHowTheRunEnded(t *testing.T) {
	cases := map[string]struct {
		err  error
		want Failure
	}{
		"exited":      {&reelctl.RunError{Cause: reelctl.CauseExited, ExitCode: 7, Stderr: "boom"}, Failure{Cause: "exited", ExitCode: intp(7), Detail: "boom"}},
		"not started": {&reelctl.RunError{Cause: reelctl.CauseNotStarted, Stderr: "no such file"}, Failure{Cause: "not_started", Detail: "no such file"}},
		"signaled":    {&reelctl.RunError{Cause: reelctl.CauseSignaled, Signal: "killed", Stderr: "partial"}, Failure{Cause: "signaled", Signal: "killed", Detail: "partial"}},
		"timed out":   {&reelctl.RunError{Cause: reelctl.CauseTimedOut, Stderr: "slow"}, Failure{Cause: "timed_out", Detail: "slow"}},
		"other error": {os.ErrPermission, Failure{Cause: "not_started", Detail: os.ErrPermission.Error()}},
	}
	for name, c := range cases {
		d := build(t, opts(t), &fakeDecompiler{fail: map[string]error{"k8": c.err}}, surgerSpec(3))
		c.want.Creator, c.want.Post, c.want.Stage = "k", "k8", "decompile"
		if len(d.Failures) != 1 || !reflect.DeepEqual(d.Failures[0], c.want) {
			t.Errorf("%s: got %+v, want %+v", name, d.Failures, c.want)
		}
	}
}

func TestFailureJSONHasNoExitCodeUnlessTheProcessExited(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{fail: map[string]error{"k8": &reelctl.RunError{Cause: reelctl.CauseSignaled, Signal: "killed"}}}, surgerSpec(3))
	data, err := d.JSON()
	if err != nil {
		t.Fatal(err)
	}
	var parsed struct {
		Failures []map[string]any `json:"failures"`
	}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatal(err)
	}
	f := parsed.Failures[0]
	if _, has := f["exit_code"]; has || f["cause"] != "signaled" || f["signal"] != "killed" {
		t.Fatalf("failure %v", f)
	}
}

// Finding 14: a stale dna file must not hide a run that wrote nothing.
func TestAStaleEditDNAFromAnEarlierRunIsNotMistakenForOutput(t *testing.T) {
	o := opts(t)
	if err := os.MkdirAll(filepath.Join(o.OutDir, "dna"), 0o755); err != nil {
		t.Fatal(err)
	}
	stale := filepath.Join(o.OutDir, "dna", "k8.edit_dna.json")
	good := `{"schema_version": 2, "duration_ms": 30000, "cuts": [], "silence_gaps": [], "words_per_minute": 1, "integrated_lufs": -14}`
	if err := os.WriteFile(stale, []byte(good), 0o644); err != nil {
		t.Fatal(err)
	}
	d := build(t, o, &fakeDecompiler{silent: map[string]bool{"k8": true}}, surgerSpec(3))
	if len(d.Failures) != 1 || d.Failures[0].Stage != "read_edit_dna" || d.Failures[0].Post != "k8" {
		t.Fatalf("failures %+v", d.Failures)
	}
	if want := "reelctl exited successfully but wrote no edit_dna.json"; d.Failures[0].Detail != want {
		t.Fatalf("detail %q, want %q", d.Failures[0].Detail, want)
	}
	if c := find(t, d, "k"); c.Breakouts[0].EditDNA != "" {
		t.Fatalf("the stale file was used: %+v", c.Breakouts[0])
	}
}

// Finding 4: a file the blueprint cannot trust is a failure, not a zero.
func TestAnEditDNAWithoutADurationOrOfAnUnknownVersionIsAFailure(t *testing.T) {
	cases := map[string]string{
		"null duration":   `{"schema_version": 2, "duration_ms": null, "cuts": [], "silence_gaps": [], "words_per_minute": 1, "integrated_lufs": -14}`,
		"missing cuts":    `{"schema_version": 2, "duration_ms": 1000, "silence_gaps": [], "words_per_minute": 1, "integrated_lufs": -14}`,
		"future version":  `{"schema_version": 9, "duration_ms": 1000, "cuts": [], "silence_gaps": [], "words_per_minute": 1, "integrated_lufs": -14}`,
		"no version":      `{"duration_ms": 1000, "cuts": [], "silence_gaps": [], "words_per_minute": 1, "integrated_lufs": -14}`,
		"negative length": `{"schema_version": 2, "duration_ms": -4, "cuts": [], "silence_gaps": [], "words_per_minute": 1, "integrated_lufs": -14}`,
	}
	for name, body := range cases {
		d := build(t, opts(t), &fakeDecompiler{body: map[string]string{"k8": body}}, surgerSpec(3))
		if len(d.Failures) != 1 || d.Failures[0].Stage != "read_edit_dna" || d.Failures[0].Post != "k8" {
			t.Errorf("%s: failures %+v", name, d.Failures)
		}
	}
}

// Finding 3 at the digest level: a burst rate is not called a word rate.
func TestBurstRatesReachTheDigestAsVoicedBursts(t *testing.T) {
	asr := map[string]string{}
	for _, id := range []string{"k6", "k7", "k8", "k9", "k10"} {
		asr[id] = "silence_bursts"
	}
	d := build(t, opts(t), &fakeDecompiler{asr: asr}, surgerSpec(3))
	bp := find(t, d, "k").Blueprint
	if bp == nil {
		t.Fatal("no blueprint")
	}
	keys := []string{}
	for _, m := range bp.Metrics {
		keys = append(keys, m.Key)
	}
	if !strings.Contains(strings.Join(keys, " "), "voiced_bursts_per_minute") || strings.Contains(strings.Join(keys, " "), "words_per_minute") {
		t.Fatalf("metrics %v", keys)
	}
}

// Finding 2 behind the CLI: Build refuses ids that would share a file.
func TestBuildRefusesPostIDsThatWouldShareAFile(t *testing.T) {
	a, b := creator("a", flat), creator("b", flat)
	b.Posts[0].ID = "A1"
	if _, err := Build(niche.Niche{Name: "n", Creators: []niche.Creator{a, b}}, opts(t), &fakeDecompiler{}); err == nil {
		t.Fatal("expected an error")
	}
}

// Finding 12: best_post_id is null, not absent, when no multiplier was computed.
func TestBestPostIDIsNullWhenNoneWasComputed(t *testing.T) {
	d := build(t, opts(t), &fakeDecompiler{}, creator("new", spec{views: repeat(100, 2)}))
	data, err := d.JSON()
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), `"best_post_id": null`) || !strings.Contains(string(data), `"best_multiplier": null`) {
		t.Fatalf("json:\n%s", data)
	}
}
