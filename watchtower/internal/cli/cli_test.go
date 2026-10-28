package cli

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/soni0021/reelctl-watchtower/internal/digest"
)

const now = "2026-10-28T06:00:00Z"

func dataDir(t *testing.T) string {
	t.Helper()
	dir, err := filepath.Abs(filepath.Join("..", "..", "testdata"))
	if err != nil {
		t.Fatal(err)
	}
	return dir
}

// stub writes a reelctl stand-in that copies testdata/dna/<media name>.json to
// --out, and exits 4 for the media names in fail.
func stub(t *testing.T, fail ...string) string {
	t.Helper()
	script := fmt.Sprintf(`#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    --input) input="$2"; shift 2;;
    --out) out="$2"; shift 2;;
    *) shift;;
  esac
done
name=$(basename "$input" .mp4)
case " %s " in *" $name "*) echo "error: decode failed" >&2; exit 4;; esac
cp "%s/dna/$name.json" "$out" || exit 3
`, strings.Join(fail, " "), dataDir(t))
	path := filepath.Join(t.TempDir(), "reelctl")
	if err := os.WriteFile(path, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}

func runWatchtower(t *testing.T, args ...string) (code int, stdout, stderr string) {
	t.Helper()
	var out, errOut bytes.Buffer
	code = Run(args, &out, &errOut)
	return code, out.String(), errOut.String()
}

func standardRun(t *testing.T, reelctl, out string) {
	t.Helper()
	code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", out, "--reelctl", reelctl, "--now", now)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
}

func readDigest(t *testing.T, out string) digest.Digest {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(out, "digest.json"))
	if err != nil {
		t.Fatal(err)
	}
	var d digest.Digest
	if err := json.Unmarshal(data, &d); err != nil {
		t.Fatal(err)
	}
	return d
}

func creatorNamed(t *testing.T, d digest.Digest, handle string) digest.Creator {
	t.Helper()
	for _, c := range d.Creators {
		if c.Handle == handle {
			return c
		}
	}
	t.Fatalf("no creator %q", handle)
	return digest.Creator{}
}

func TestDigestMatchesTheGoldenFiles(t *testing.T) {
	out := t.TempDir()
	standardRun(t, stub(t), out)
	for _, name := range []string{"digest.json", "digest.md"} {
		got, err := os.ReadFile(filepath.Join(out, name))
		if err != nil {
			t.Fatal(err)
		}
		golden := filepath.Join(dataDir(t), "golden", name)
		if os.Getenv("WATCHTOWER_UPDATE_GOLDEN") == "1" {
			if err := os.MkdirAll(filepath.Dir(golden), 0o755); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(golden, got, 0o644); err != nil {
				t.Fatal(err)
			}
		}
		want, err := os.ReadFile(golden)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(got, want) {
			t.Errorf("%s differs from %s", name, golden)
		}
	}
}

// The values below were computed outside the program, from niche.json and the
// recorded edit_dna files, with a median and a least-squares fit written
// separately.
func TestDigestHoldsTheHandComputedValues(t *testing.T) {
	out := t.TempDir()
	standardRun(t, stub(t), out)
	d := readDigest(t, out)

	var order, surging []string
	for _, c := range d.Creators {
		order = append(order, c.Handle)
		if c.Surging {
			surging = append(surging, c.Handle)
		}
	}
	if want := []string{"surger", "rocket", "steady", "newbie"}; !reflect.DeepEqual(order, want) {
		t.Errorf("order %v, want %v", order, want)
	}
	if want := []string{"surger", "rocket"}; !reflect.DeepEqual(surging, want) {
		t.Errorf("surging %v, want %v", surging, want)
	}

	surger := creatorNamed(t, d, "surger")
	multipliers := map[string]float64{}
	for _, b := range surger.Breakouts {
		multipliers[b.ID] = b.Multiplier
	}
	if want := map[string]float64{"s8": 12, "s9": 23.8095, "s11": 26.087}; !reflect.DeepEqual(multipliers, want) {
		t.Errorf("breakout multipliers %v, want %v", multipliers, want)
	}
	if *surger.BestMultiplier != 26.087 || surger.BestPostID != "s11" || surger.BaselinePosts != 3 {
		t.Errorf("best %v %s, baseline %d", *surger.BestMultiplier, surger.BestPostID, surger.BaselinePosts)
	}
	if !reflect.DeepEqual(surger.SurgingBy, []string{"multiplier"}) || surger.Velocity.FollowersPerDay != 100 || surger.Velocity.FollowersShare != 0.001 {
		t.Errorf("surger: %v %+v", surger.SurgingBy, surger.Velocity)
	}

	rocket := creatorNamed(t, d, "rocket")
	if !reflect.DeepEqual(rocket.SurgingBy, []string{"velocity"}) || rocket.Velocity.FollowersPerDay != 319.6429 || rocket.Velocity.FollowersShare != 0.032 {
		t.Errorf("rocket: %v %+v", rocket.SurgingBy, rocket.Velocity)
	}
	if *rocket.BestMultiplier != 1.122 || rocket.Blueprint != nil {
		t.Errorf("rocket best %v blueprint %v", *rocket.BestMultiplier, rocket.Blueprint)
	}

	steady := creatorNamed(t, d, "steady")
	if steady.Surging || *steady.BestMultiplier != 1.06 || steady.Velocity.FollowersShare != 0.0004 {
		t.Errorf("steady: %+v", steady)
	}

	newbie := creatorNamed(t, d, "newbie")
	if newbie.BestMultiplier != nil || newbie.Velocity != nil || len(newbie.Skipped) != 4 {
		t.Errorf("newbie: %+v", newbie)
	}
	if want := (digest.Skip{Post: "n3", Reason: "not_enough_history", Detail: "2 of 5 earlier posts needed"}); newbie.Skipped[2] != want {
		t.Errorf("newbie skip %+v, want %+v", newbie.Skipped[2], want)
	}
	if want := (digest.Skip{Reason: "velocity_not_computed", Detail: "2 snapshots in the 7 days ending 2026-10-21, need 3"}); newbie.Skipped[3] != want {
		t.Errorf("newbie velocity skip %+v, want %+v", newbie.Skipped[3], want)
	}
}

func TestBlueprintSentencesMatchTheHandComputedChanges(t *testing.T) {
	out := t.TempDir()
	standardRun(t, stub(t), out)
	bp := creatorNamed(t, readDigest(t, out), "surger").Blueprint
	if bp == nil {
		t.Fatal("no blueprint")
	}
	tail := "across 3 breakouts against 3 baseline posts."
	want := []string{
		"Mean shot length dropped from 4.5 s to 1.6 s (−65%) " + tail,
		"Median shot length dropped from 4.7 s to 1.6 s (−66%) " + tail,
		"Cuts per minute rose from 12.3 to 37.7 (+205%) " + tail,
		"Pauses per minute dropped from 2.7 to 0.7 (−75%) " + tail,
		"Silent share dropped from 1.9% to 0.2% (−89%) " + tail,
		"Integrated loudness rose from -16.0 LUFS to -11.8 LUFS (+26%) " + tail,
	}
	if !reflect.DeepEqual(bp.Sentences, want) {
		t.Errorf("sentences:\n%s\nwant:\n%s", strings.Join(bp.Sentences, "\n"), strings.Join(want, "\n"))
	}
	wantNotes := []string{"visual_events was missing from 3 of 6 files, so visual events are not compared."}
	if !reflect.DeepEqual(bp.Notes, wantNotes) {
		t.Errorf("notes %v", bp.Notes)
	}
	wpm := bp.Metrics[5]
	if wpm.Key != "words_per_minute" || wpm.Before != 140 || wpm.After != 150 || *wpm.Change != 0.0714 {
		t.Errorf("words per minute: %+v", wpm)
	}
}

func TestADecompileFailureIsListedAndTheRestProceed(t *testing.T) {
	out := t.TempDir()
	standardRun(t, stub(t, "s9"), out)
	d := readDigest(t, out)
	want := []digest.Failure{{Creator: "surger", Post: "s9", Stage: "decompile", ExitCode: 4, Detail: "error: decode failed"}}
	if !reflect.DeepEqual(d.Failures, want) {
		t.Fatalf("failures %+v", d.Failures)
	}
	surger := creatorNamed(t, d, "surger")
	if surger.Blueprint == nil || surger.Blueprint.Breakouts != 2 || surger.Blueprint.Baseline != 3 {
		t.Fatalf("blueprint %+v", surger.Blueprint)
	}
	if _, err := os.Stat(filepath.Join(out, "dna", "s11.edit_dna.json")); err != nil {
		t.Errorf("the run stopped early: %v", err)
	}
	md, err := os.ReadFile(filepath.Join(out, "digest.md"))
	if err != nil || !strings.Contains(string(md), "- surger, post s9: reelctl exited with code 4: error: decode failed") {
		t.Errorf("digest.md does not list the failure: %v\n%s", err, md)
	}
}

func TestAMissingReelctlBinaryIsFailuresNotACrash(t *testing.T) {
	out := t.TempDir()
	code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", out, "--reelctl", filepath.Join(t.TempDir(), "absent"), "--now", now)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr)
	}
	d := readDigest(t, out)
	if len(d.Failures) != 6 || d.Failures[0].ExitCode != -1 {
		t.Fatalf("failures %+v", d.Failures)
	}
}

func TestARerunIsByteIdentical(t *testing.T) {
	first, second := t.TempDir(), t.TempDir()
	reelctl := stub(t)
	standardRun(t, reelctl, first)
	standardRun(t, reelctl, second)
	for _, name := range []string{"digest.json", "digest.md"} {
		a, errA := os.ReadFile(filepath.Join(first, name))
		b, errB := os.ReadFile(filepath.Join(second, name))
		if errA != nil || errB != nil || !bytes.Equal(a, b) {
			t.Errorf("%s differs between runs", name)
		}
		if strings.Contains(string(a), first) {
			t.Errorf("%s names the output directory", name)
		}
	}
}

func TestWithoutNowTheClockIsUsed(t *testing.T) {
	out := t.TempDir()
	clock := func() time.Time { return time.Date(2026, 11, 2, 9, 30, 0, 0, time.FixedZone("x", 3600)) }
	var stdout, stderr bytes.Buffer
	code := run([]string{"run", "--data", dataDir(t), "--out", out, "--reelctl", stub(t)}, &stdout, &stderr, clock)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, stderr.String())
	}
	if got := readDigest(t, out).GeneratedAt; got != "2026-11-02T08:30:00Z" {
		t.Fatalf("generated_at = %q, want the clock in UTC", got)
	}
}

func TestSummaryLineOnStdout(t *testing.T) {
	out := t.TempDir()
	code, stdout, _ := runWatchtower(t, "run", "--data", dataDir(t), "--out", out, "--reelctl", stub(t), "--now", now)
	want := "watchtower: 4 creators, 2 surging, 0 failures; wrote digest.json and digest.md\n"
	if code != 0 || stdout != want {
		t.Fatalf("exit %d, stdout %q, want %q", code, stdout, want)
	}
}

func TestBadArgumentsExitTwo(t *testing.T) {
	data, out := dataDir(t), t.TempDir()
	cases := map[string][]string{
		"no command":          {},
		"unknown command":     {"watch"},
		"missing --data":      {"run", "--out", out},
		"missing --out":       {"run", "--data", data},
		"unknown flag":        {"run", "--data", data, "--out", out, "--bogus"},
		"bad --now":           {"run", "--data", data, "--out", out, "--now", "yesterday"},
		"zero --min-score":    {"run", "--data", data, "--out", out, "--min-score", "0"},
		"negative velocity":   {"run", "--data", data, "--out", out, "--min-velocity", "-1"},
		"missing data dir":    {"run", "--data", filepath.Join(out, "absent"), "--out", out},
		"extra positional":    {"run", "--data", data, "--out", out, "extra"},
		"min-score not a num": {"run", "--data", data, "--out", out, "--min-score", "ten"},
	}
	for name, args := range cases {
		if code, _, stderr := runWatchtower(t, args...); code != 2 || stderr == "" {
			t.Errorf("%s: exit %d, stderr %q; want 2 and a message", name, code, stderr)
		}
	}
}

func TestAMalformedNicheFileExitsTwoNamingTheField(t *testing.T) {
	dir := t.TempDir()
	body := `{"niche": "n", "creators": [{"handle": "a", "posts": [{"id": "p1", "posted_at": "2026-10-20T10:00:00Z"}]}]}`
	if err := os.WriteFile(filepath.Join(dir, "niche.json"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	code, _, stderr := runWatchtower(t, "run", "--data", dir, "--out", t.TempDir())
	if code != 2 || !strings.Contains(stderr, `"a"`) || !strings.Contains(stderr, "views") {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
}

func TestAnOutputDirectoryThatCannotBeMadeExitsOne(t *testing.T) {
	blocker := filepath.Join(t.TempDir(), "file")
	if err := os.WriteFile(blocker, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, _, stderr := runWatchtower(t, "run", "--data", dataDir(t), "--out", filepath.Join(blocker, "out"), "--reelctl", stub(t))
	if code != 1 || stderr == "" {
		t.Fatalf("exit %d, stderr %q", code, stderr)
	}
}
