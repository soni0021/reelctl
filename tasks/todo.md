# Tasks: comparative diff

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/comparative-diff.md](../docs/specs/comparative-diff.md). Tick each box in the commit that completes it.

## Task 1: Metrics, rhythm windows, relative findings, and multi-outlier payoff

**Description:** Save the baselines of `reelctl diff` first (every existing fixture pair, and the two real reels in both directions). Then, test-first in `crates/diff`: per-reel metrics, the 5 s rhythm windows with their votes, `rhythm_gap` and `pause_density`, the payoff rule for several outliers, and `compare_many`.

**Acceptance criteria:**
- [ ] Baselines saved; the failing tests committed before the implementation, in a commit that compiles (public types and signatures with honest stub bodies, so the tests fail on assertions)
- [ ] Metrics: mean and longest shot with and without cuts, dead-air count and per minute, silent share, `words_source`, and no division by zero for a zero-length reel
- [ ] Rhythm: counts per window for the draft and each outlier; an ended outlier is `None` and does not vote; the last window is short
- [ ] `rhythm_gap`: 2 of 3 yes, 1 of 2 no, 1 of 1 yes; adjacent windows merge; no flag when the draft cuts as often; none after every outlier has ended
- [ ] `pause_density`: nothing below 1.5× or under 3 extra per minute; one flag above both; the median of two outliers is their mean
- [ ] Payoff with one, two (both needed), and three (two needed) outliers; with one it equals `compare`
- [ ] `compare` and its existing tests are untouched and pass

**Verification:**
- [ ] `cargo test -p diff` (new tests red first); `cargo fmt`; no warnings

**Dependencies:** None

**Files likely touched:** `crates/diff/src/lib.rs` (or a new `crates/diff/src/compare.rs` and a re-export)

**Estimated scope:** Medium

### Checkpoint A
- [ ] `cargo test -p diff` passes

## Task 2: `--outlier` up to three times, `comparison` in the JSON, and `--format text`

**Description:** Make `--outlier` repeatable (one to three) and add `--format <json|text>`. JSON gains `comparison` after `flags`. The text report is a pure function tested on its own. Test-first, with fixtures.

**Acceptance criteria:**
- [ ] `draft12` against `outlier12`: `flags` exactly as today, plus `comparison` with both reels' metrics and a `rhythm_gap` for 0–5 s
- [ ] `draft12` against `outlier12` and `textured_cuts12`: per-outlier metrics and window rows, and `rhythm_gap` only where both outliers cut and the draft does not
- [ ] Four `--outlier` arguments exit with code 2 (through clap's error path, not `main`'s exit 1) and a message naming the limit of three; zero still exits with code 2 from clap
- [ ] Each reel carries a `name` in the metrics (file name without directory or `.edit_dna.json`), and the text report's column headings use it
- [ ] `--format text` has the table header, the window rows, and the flag lines; `--format json` parses and is the default
- [ ] The existing diff end-to-end tests pass unchanged, including the version 1 outlier test
- [ ] `flags` equals the Task 1 baselines for every fixture pair

**Verification:**
- [ ] `cargo test` once (new tests red first); `cargo fmt`; no warnings; baseline comparison with a release build

**Dependencies:** Task 1

**Files likely touched:** `crates/reelctl/src/main.rs`, `crates/reelctl/tests/p0_fixture.rs`, `crates/diff/src/lib.rs` (the text renderer)

**Estimated scope:** Medium

### Checkpoint B
- [ ] `cargo test` passes; baselines match

## Task 3: Docs, ADR-014, the helper script, and the real reels

**Description:** Run the two real reels in both directions and record the output; write the decision and the docs; switch the helper script to the text report.

**Acceptance criteria:**
- [ ] Both real-reel directions match the expected output in the spec's Evidence (including the known-limit `pause_density`), recorded in `docs/status.md` with date and machine; the reels themselves are not committed
- [ ] ADR-014; `docs/cli.md` (`--outlier` repeated, `--format`, the `comparison` object, an updated example); README; CHANGELOG; the capability map row; this spec's status
- [ ] `Reels/compare.sh` uses `--format text` (it is git-ignored, so it is updated in place and not committed)
- [ ] The thresholds are listed in `status.md` as untested on a fast-cut outlier

**Verification:**
- [ ] Links checked; `cargo test` unchanged; the `audit` subagent run on the final commit

**Dependencies:** Task 2

**Files likely touched:** the docs above

**Estimated scope:** Small

### Checkpoint C
- [ ] All six spec success criteria verified; audit run; reviewed with the user, merged into `main`, and this plan moved to `tasks/archive/comparative-diff/`
