package digest

import (
	"strings"
	"testing"

	"github.com/soni0021/reelctl-watchtower/internal/blueprint"
)

const emptyMarkdown = `# Why They Grew + Exact Edit Blueprint

Niche: n. Generated 2026-10-28T06:00:00Z.

A creator is surging when a post has an outlier multiplier of at least 10, or followers grow by more than 2.0% a day. A sentence appears only for a change of at least 25%.

## Surging creators

None.

## Other creators

None.

## Failures

None.
`

func TestMarkdownOfAnEmptyRun(t *testing.T) {
	d := Digest{GeneratedAt: "2026-10-28T06:00:00Z", Niche: "n", MinScore: 10, MinVelocity: 0.02}
	if got := d.Markdown(); got != emptyMarkdown {
		t.Fatalf("got:\n%s", got)
	}
}

func TestMarkdownListsFailuresByStage(t *testing.T) {
	d := Digest{Failures: []Failure{
		{Creator: "k", Post: "k9", Stage: StageDecompile, Cause: "exited", ExitCode: intp(7), Detail: "boom"},
		{Creator: "k", Post: "k8", Stage: StageDecompile, Cause: "not_started", Detail: "no such file"},
		{Creator: "k", Post: "k7", Stage: StageReadDNA, Detail: "not a valid edit_dna.json"},
		{Creator: "k", Post: "k6", Stage: StageDecompile, Cause: "signaled", Signal: "killed", Detail: "partial"},
		{Creator: "k", Post: "k5", Stage: StageDecompile, Cause: "timed_out", Detail: "slow"},
	}}
	got := d.Markdown()
	for _, want := range []string{
		"- k, post k9: reelctl exited with code 7: boom",
		"- k, post k8: reelctl did not start: no such file",
		"- k, post k7: the edit_dna.json could not be used: not a valid edit_dna.json",
		"- k, post k6: reelctl was ended by signal killed: partial",
		"- k, post k5: reelctl timed out: slow",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %q in:\n%s", want, got)
		}
	}
}

func TestMarkdownShowsACreatorsNumbersAndBlueprint(t *testing.T) {
	m := 12.5
	change := -0.5
	d := Digest{Creators: []Creator{{
		Handle: "a", Surging: true, SurgingBy: []string{"multiplier", "velocity"},
		BestPostID: strp("p2"), BestMultiplier: &m,
		Velocity:  &Velocity{Snapshots: 7, FollowersPerDay: 150, FollowersShare: 0.03, ViewsPerDay: floatp(1000), ViewsShare: floatp(0.01)},
		Breakouts: []Breakout{{ID: "p2", Multiplier: 12.5}}, BaselinePosts: 4,
		Blueprint: &blueprint.Blueprint{
			Breakouts: 3, Baseline: 4, ChangeThreshold: 0.25,
			Metrics:   []blueprint.Metric{{Label: "Cuts per minute", Unit: "per_minute", Before: 10, After: 5, Change: &change}, {Label: "Silent share", Unit: "share", Before: 0, After: 0.1}},
			Sentences: []string{"Cuts per minute dropped from 10.0 to 5.0 (−50%) across 3 breakouts against 4 baseline posts."},
			Notes:     []string{"a note"},
		},
		Skipped: []Skip{{Post: "p1", Reason: "not_enough_history", Detail: "0 earlier posts, need 5"}, {Reason: "velocity_not_computed", Detail: "no snapshots"}},
	}}}
	got := d.Markdown()
	for _, want := range []string{
		"### a\n",
		"- Surging by: multiplier, velocity.\n",
		"- Best post: p2 at 12.50x the median of its earlier posts.\n",
		"- Followers: +150.0 a day (3.00% a day) over 7 snapshots. Total views: +1000.0 a day (1.00% a day).\n",
		"- Breakouts: 1 (p2 at 12.50x). Baseline posts: 4.\n",
		"**Edit blueprint**: 3 breakouts against 4 baseline posts.\n",
		"- Cuts per minute dropped from 10.0 to 5.0 (−50%) across 3 breakouts against 4 baseline posts.\n",
		"| Metric | Baseline posts → breakouts | Change |\n|---|---|---|\n",
		"| Cuts per minute | 10.0 → 5.0 | −50% |\n",
		"| Silent share | 0.0% → 10.0% | from zero |\n",
		"- a note\n",
		"- Post p1: not_enough_history, 0 earlier posts, need 5.\n",
		"- velocity_not_computed, no snapshots.\n",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %q in:\n%s", want, got)
		}
	}
}

func intp(v int) *int           { return &v }
func strp(v string) *string     { return &v }
func floatp(v float64) *float64 { return &v }

func TestMarkdownSaysWhenTotalViewsGrowthWasNotComputed(t *testing.T) {
	d := Digest{Creators: []Creator{{Handle: "a", SurgingBy: []string{}, Velocity: &Velocity{Snapshots: 3, FollowersPerDay: 10, FollowersShare: 0.01}}}}
	got := d.Markdown()
	if !strings.Contains(got, "- Followers: +10.0 a day (1.00% a day) over 3 snapshots. Total views: not computed.\n") {
		t.Fatalf("got:\n%s", got)
	}
}

func TestMarkdownPutsANonSurgingCreatorUnderOtherCreators(t *testing.T) {
	d := Digest{Creators: []Creator{{Handle: "calm", SurgingBy: []string{}}}}
	got := d.Markdown()
	if strings.Index(got, "### calm") < strings.Index(got, "## Other creators") {
		t.Fatalf("calm is not under Other creators:\n%s", got)
	}
	if !strings.Contains(got, "- Not surging.\n") {
		t.Fatalf("missing the not-surging line:\n%s", got)
	}
}
