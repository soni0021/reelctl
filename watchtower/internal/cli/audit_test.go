package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

func script(t *testing.T, body string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "reelctl")
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"+body), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}

func writeData(t *testing.T, body string) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "niche.json"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

// nicheWithPostIDs is one creator whose seventh post is a 50x breakout; ids
// names the seven posts in date order.
func nicheWithPostIDs(ids ...string) string {
	var posts []string
	for i, id := range ids {
		views := 100
		if i == len(ids)-1 {
			views = 5000
		}
		posts = append(posts, fmt.Sprintf(`{"id": %q, "posted_at": "2026-10-%02dT10:00:00Z", "views": %d, "media_path": "media/%s.mp4"}`, id, i+1, views, id))
	}
	return `{"niche": "n", "creators": [{"handle": "a", "posts": [` + strings.Join(posts, ",") + `]}]}`
}

func surgingHandles(t *testing.T, out string) []string {
	t.Helper()
	var handles []string
	for _, c := range readDigest(t, out).Creators {
		if c.Surging {
			handles = append(handles, c.Handle)
		}
	}
	return handles
}

// The audit's case: a baseline post renamed to the breakout's id used to
// overwrite the breakout's dna file and report the wrong numbers.
func TestADuplicatePostIDExitsTwoWithAClearMessageAndWritesNothing(t *testing.T) {
	data := writeData(t, nicheWithPostIDs("p1", "p2", "p7", "p4", "p5", "p6", "p7"))
	out := t.TempDir()
	code, _, stderr := runWatchtower(t, "run", "--data", data, "--out", out, "--reelctl", stub(t), "--now", now)
	if code != 2 {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	for _, want := range []string{`creator "a"`, `"p7"`, "duplicate"} {
		if !strings.Contains(stderr, want) {
			t.Errorf("stderr %q does not mention %q", stderr, want)
		}
	}
	if _, err := os.Stat(filepath.Join(out, "digest.json")); err == nil {
		t.Error("a digest was written for invalid input")
	}
}

func TestAMediaPathThatEscapesTheDataDirectoryExitsTwoAndRunsNothing(t *testing.T) {
	marker := filepath.Join(t.TempDir(), "ran")
	reelctl := script(t, fmt.Sprintf("echo ran > %q\nexit 0\n", marker))
	body := strings.Replace(nicheWithPostIDs("p1", "p2", "p3", "p4", "p5", "p6", "p7"), `media/p7.mp4`, `../../etc/passwd`, 1)
	code, _, stderr := runWatchtower(t, "run", "--data", writeData(t, body), "--out", t.TempDir(), "--reelctl", reelctl)
	if code != 2 || !strings.Contains(stderr, "media_path") || !strings.Contains(stderr, `"p7"`) {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("reelctl was run")
	}
}

func TestReelctlGetsAnAbsolutePathInsideTheDataDirectory(t *testing.T) {
	log := filepath.Join(t.TempDir(), "inputs")
	reelctl := script(t, fmt.Sprintf(`while [ $# -gt 0 ]; do case "$1" in --input) echo "$2" >> %q; shift 2;; *) shift;; esac; done
exit 1
`, log))
	data := writeData(t, nicheWithPostIDs("p1", "p2", "p3", "p4", "p5", "p6", "p7"))
	t.Chdir(filepath.Dir(data))
	code, _, stderr := runWatchtower(t, "run", "--data", filepath.Base(data), "--out", t.TempDir(), "--reelctl", reelctl)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	inputs, err := os.ReadFile(log)
	if err != nil {
		t.Fatal(err)
	}
	for _, input := range strings.Fields(string(inputs)) {
		if !filepath.IsAbs(input) || !strings.HasSuffix(input, ".mp4") {
			t.Errorf("reelctl got %q", input)
		}
	}
}

// Finding 8: the flags reach the calculation.
func TestMinScoreFromTheCommandLineDecidesWhoSurges(t *testing.T) {
	out := t.TempDir()
	code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", out, "--reelctl", stub(t), "--now", now, "--min-score", "30")
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	if got, want := surgingHandles(t, out), []string{"rocket"}; !reflect.DeepEqual(got, want) {
		t.Fatalf("with --min-score 30 surging = %v, want %v (the best multiplier of surger is 26.09)", got, want)
	}
	if d := readDigest(t, out); d.MinScore != 30 {
		t.Fatalf("min_score = %v", d.MinScore)
	}
	low := t.TempDir()
	if code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", low, "--reelctl", stub(t), "--now", now, "--min-score", "1.1"); code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	if got := surgingHandles(t, low); !reflect.DeepEqual(got, []string{"surger", "rocket"}) {
		t.Fatalf("with --min-score 1.1 surging = %v (rocket's best post is 1.12x, steady's 1.06x)", got)
	}
}

func TestMinVelocityFromTheCommandLineDecidesWhoSurges(t *testing.T) {
	out := t.TempDir()
	if code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", out, "--reelctl", stub(t), "--now", now, "--min-velocity", "0.05"); code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	if got, want := surgingHandles(t, out), []string{"surger"}; !reflect.DeepEqual(got, want) {
		t.Fatalf("with --min-velocity 0.05 surging = %v, want %v (rocket grows 3.2%% a day)", got, want)
	}
}

// Finding 7.
func TestAKilledReelctlIsReportedAsSignaledNotAsNotStarted(t *testing.T) {
	out := t.TempDir()
	code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", out, "--reelctl", script(t, "kill -9 $$\n"), "--now", now)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	d := readDigest(t, out)
	if len(d.Failures) != 6 {
		t.Fatalf("failures %+v", d.Failures)
	}
	for _, f := range d.Failures {
		if f.Cause != "signaled" || f.Signal != "killed" || f.ExitCode != nil {
			t.Errorf("failure %+v", f)
		}
	}
	md, err := os.ReadFile(filepath.Join(out, "digest.md"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(md), "reelctl was ended by signal killed") || strings.Contains(string(md), "did not start") {
		t.Fatalf("digest.md:\n%s", md)
	}
}

// Finding 10.
func TestASlowReelctlIsCutOffAtTheTimeoutFlag(t *testing.T) {
	out := t.TempDir()
	begin := time.Now()
	code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", out, "--reelctl", script(t, "exec sleep 30\n"), "--now", now, "--reelctl-timeout", "300ms")
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	if elapsed := time.Since(begin); elapsed > 20*time.Second {
		t.Fatalf("took %v for 6 runs with a 300ms timeout", elapsed)
	}
	d := readDigest(t, out)
	if len(d.Failures) != 6 {
		t.Fatalf("failures %+v", d.Failures)
	}
	for _, f := range d.Failures {
		if f.Cause != "timed_out" || f.ExitCode != nil {
			t.Errorf("failure %+v", f)
		}
	}
}

// Finding 14.
func TestARerunThatWritesNothingDoesNotReuseTheEarlierOutput(t *testing.T) {
	out := t.TempDir()
	standardRun(t, stub(t), out)
	if _, err := os.Stat(filepath.Join(out, "dna", "s9.edit_dna.json")); err != nil {
		t.Fatal(err)
	}
	code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", out, "--reelctl", script(t, "exit 0\n"), "--now", now)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	d := readDigest(t, out)
	if len(d.Failures) != 6 {
		t.Fatalf("failures %+v", d.Failures)
	}
	for _, f := range d.Failures {
		if f.Stage != "read_edit_dna" || f.Detail != "reelctl exited successfully but wrote no edit_dna.json" {
			t.Errorf("failure %+v", f)
		}
	}
	for _, c := range d.Creators {
		if c.Blueprint != nil {
			t.Errorf("%s got a blueprint from stale files", c.Handle)
		}
	}
}
