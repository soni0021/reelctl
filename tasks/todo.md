# Tasks: quiet audio

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/quiet-audio.md](../docs/specs/quiet-audio.md). Tick each box in the commit that completes it.

## Task 1: Baselines, `quiet12`, and the failing tests

**Description:** Save every existing fixture's `edit_dna.json` and the real iPhone recording's silence figures from the current release build, outside the repo. Add the `quiet12` fixture. Write the unit and end-to-end tests that the spec lists, and watch them fail.

**Acceptance criteria:**
- [x] Baselines saved for all fixtures and for `IMG_1435.MOV`; the Reels file's current result noted (one gap, threshold −40.0)
- [x] `quiet12` in `fixtures/generate.sh` (`quiet12` kind and in `all`): a 440 Hz tone with `volume=-28dB` (−46 dB peak, about −49 dB RMS) and a 0.5 s mute; byte-reproducible; its window level checked with ffmpeg `astats`
- [x] Unit tests in `silence.rs`, failing for the right reason: the Reels-shaped levels, the quiet tone with a pause, the two mostly-silent shapes (must stay −40; they pass today and must keep passing), the under-1% shape (995 at −120 plus 5 at −14: −70, with the silence still a gap), a recording of only −120 (−70, one gap), and a silent recording never giving `−inf` or NaN
- [x] End-to-end test in `p0_fixture.rs`: `quiet12`'s gaps of 180 ms or more are the planted pause (±20 ms) and `diff` flags it; failing today
- [x] The existing tests still pass

**Verification:**
- [x] `cargo test` shows exactly the new quiet-recording tests failing (4 unit tests and the end-to-end test; the mostly-silent test passes today, as it must)

**Dependencies:** None

**Files likely touched:** `crates/decompile/src/silence.rs` (tests), `fixtures/generate.sh`, `crates/reelctl/tests/p0_fixture.rs`, README fixture row

**Estimated scope:** Small

## Task 2: The new threshold

**Description:** Change `threshold_db` to `max(max(min(−40, p99 − 12), −70), min(p10 + 6, p90 − 12))` with named `LOUD_PERCENTILE` and `FLOOR_LIMIT_DB`, and update its comments.

**Acceptance criteria:**
- [x] All Task 1 tests pass; the existing silence tests pass unchanged
- [x] Every fixture and the real iPhone recording match their baselines apart from timings (`silence_gaps`, `silence_threshold_db`, `noise_floor_db`, `words`)
- [x] The Reels file, run with the real binary: no whole-clip gap, threshold below −40; the numbers recorded
- [x] The 60 s benchmark's median `signal_ms` stays within 242–262 ms (one extra rank lookup)
- [x] The ignored whisper test passes with the Apex model

**Verification:**
- [x] `cargo test` once; baseline comparison with a release build; `cargo fmt` clean; no warnings

**Dependencies:** Task 1

**Files likely touched:** `crates/decompile/src/silence.rs`

**Estimated scope:** Small

### Checkpoint A
- [x] `cargo test` passes; baselines match; Reels file fixed

## Task 3: Docs, ADR-013, and the real-file numbers

**Description:** Record the decision and the measurements; mark the superseded part of ADR-008.

**Acceptance criteria:**
- [x] ADR-013 (quiet recordings: the floor follows the loud part); ADR-008 marked partly superseded
- [x] `docs/edit-dna.md` (`silence_threshold_db` and the silence section), `docs/status.md` (measurements with date and machine; the quiet-audio limitation removed or restated), `docs/cli.md` only if its text changes, CHANGELOG, README, this spec's status, the capability map if it lists it
- [x] The Reels file is never committed; only its numbers are

**Verification:**
- [x] Links checked; `cargo test` unchanged
- [ ] The `audit` subagent run on the final commit

**Dependencies:** Task 2

**Files likely touched:** the docs above

**Estimated scope:** Small

### Checkpoint B
- [ ] All five spec success criteria verified; audit run; reviewed with the user, merged into `main`, and this plan moved to `tasks/archive/quiet-audio/`
