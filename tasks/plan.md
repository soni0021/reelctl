# Implementation Plan: comparative diff

Spec: [docs/specs/comparative-diff.md](../docs/specs/comparative-diff.md), approved 2026-10-26 with the five recommended answers. Branch: `feature/comparative-diff`. Checklist: [tasks/todo.md](todo.md). Earlier plans are in [tasks/archive/](archive/quiet-audio/plan.md).

## Overview

Let `reelctl diff` compare a draft with one to three outliers: per-reel metrics, cut rhythm in 5 s windows, two relative findings, and a text report. Three tasks: the comparison logic in `crates/diff` first (pure functions over `EditDna`, easiest to test), then the CLI and its output, then the docs and the real reels. `decompile` and `edit_dna.json` are untouched.

## Architecture Decisions

- **`flags` stays byte-identical.** `compare(&draft, &outlier) -> DiffReport` keeps its signature, behavior, and JSON, and its existing tests do not change. A new function `compare_many(&draft, &[outlier]) -> Report` returns `{ flags, comparison }`; its `flags` come from the existing three checks, with the payoff check generalized (a cue in more than half of the outliers; with one outlier this is `compare`'s rule, and a test says so).
- **Relative findings live in `comparison.flags`**, as a second enum (`RelativeFlag`: `RhythmGap`, `PauseDensity`), so nothing that reads `flags` (the existing tests, `cli.md`, `Reels/compare.sh`) can meet an unknown kind.
- **Metrics come from `EditDna` alone.** `cuts`, `silence_gaps`, `words_per_minute`, `asr_backend`, `integrated_lufs`, and `duration_ms` are required fields in every schema version; version 1 lacks only `silence_threshold_db` and `noise_floor_db` (serde defaults), which the metrics do not use. Checked in `crates/edit-dna/src/lib.rs` and by the existing version 1 outlier test, so older files need no fallback.
- **Names come from the CLI.** `EditDna` carries no source name, so `compare_many` takes named reels (`NamedReel { name, dna }`) and each metrics entry carries its `name`; the JSON and the text report both use it. The CLI passes the draft's file stem and each outlier's file name without its directory and without a trailing `.edit_dna.json`, so `render_text` stays a pure function of the report.
- **Windows and votes.** 5,000 ms windows over the draft's duration. An outlier's count in a window is `None` once the outlier has ended before the window starts, and `None` outliers do not vote. "More than half" means `2 * with_cut > counted`, the same test the payoff rule uses, so one helper serves both.
- **Constants named for what they limit:** `RHYTHM_WINDOW_MS`, `PAUSE_DENSITY_RATIO` (1.5), `PAUSE_DENSITY_MIN_EXTRA_PER_MINUTE` (3.0). No flag, no environment variable.
- **The text report is a pure function of the report** (`render_text(&Report) -> String`), so it is tested without spawning anything. The CLI chooses between it and `serde_json` by `--format`.
- **Arguments:** `--outlier` becomes a repeatable `Vec<PathBuf>`. Zero is an error from clap as today. Clap 4 cannot cap how many times a flag repeats, and `main` turns every other error into exit code 1, so the CLI checks the count itself and ends with `Cli::command().error(ErrorKind::TooManyValues, "…").exit()` (from `clap::CommandFactory`), which prints clap's usage error and exits with code 2. The test asserts the exit code is 2, not just that it failed.
- **The red commit compiles.** Task 1's failing tests need `compare_many`, `Report`, `Comparison`, `RelativeFlag`, and the metrics types, and a commit with `todo!()` is not allowed. So the red commit adds those public types and signatures with honest stub bodies (`compare_many` returns today's `flags` and an empty `Comparison`), and the new tests fail on their assertions, not on compilation. The same holds for Task 2's `render_text` and `--format`.
- **Baseline first.** Before any change, save `reelctl diff` output for every existing fixture pair and for the two real reels in both directions (outside the repo). After Task 2, `flags` must be identical (criterion 2).
- **Fixtures:** no new ones. `outlier12` (cuts at 3 and 9 s), `textured_cuts12` (3, 6, 9 s), and `draft12` (cut at 6 s) already give the rhythm cases the spec lists.

## Task List

### Phase 1: Comparison
- [x] Task 1: Metrics, rhythm windows, relative findings, and multi-outlier payoff in `crates/diff`

### Checkpoint A
- [x] `cargo test -p diff` passes; `compare` and its existing tests are untouched

### Phase 2: Command line
- [x] Task 2: `--outlier` up to three times, `comparison` in the JSON, and `--format text`

### Checkpoint B
- [x] `cargo test` passes; `flags` identical to the baselines for every fixture pair and both real reels

### Phase 3: Close-out
- [ ] Task 3: Docs, ADR-014, the helper script, and the real reels

### Checkpoint C
- [ ] All six spec success criteria verified; the `audit` subagent has run on the final commit; reviewed with the user, then `feature/comparative-diff` merged into `main`, and this plan moved to `tasks/archive/comparative-diff/`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| A change to `flags` breaks a script, the docs example, or an existing test | High | `compare` is not edited; the baselines of every fixture pair and both real reels are compared after Task 2; the existing tests run unchanged |
| Division by zero or panics on odd inputs (zero duration, no cuts, an outlier shorter than the draft) | Medium | Unit tests for each; no `unwrap` in library code; `duration_ms == 0` gives zero rates |
| The thresholds are tuned on two slow reels, so they may be wrong on fast-cut footage | Medium | Named constants; `status.md` says they are untested on a fast-cut outlier, and the spec says to retune on the first one |
| `pause_density` fires on the NEET-as-draft pair because the milk clip's pauses come from near-silent audio | Low | Expected and written into the spec's Evidence; the manual run must show it and `status.md` must call it a known limit |
| The text report drifts from the JSON | Low | Both are built from one `Report`; the end-to-end test checks the same numbers in both |
| `--outlier` repeated four times is silently accepted | Low | A test that expects exit code 2 |
