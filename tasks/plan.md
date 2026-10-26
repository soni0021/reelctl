# Implementation Plan: quiet audio

Spec: [docs/specs/quiet-audio.md](../docs/specs/quiet-audio.md), approved 2026-10-26 with the recommended answers (no new JSON field; keep the 12 dB distance). Branch: `feature/quiet-audio`, started from the unmerged `chore/remove-unreachable-arms`. Checklist: [tasks/todo.md](todo.md). Earlier plans are in [tasks/archive/](archive/fast-decode/plan.md).

## Overview

Let the silence threshold's −40 dB floor come down to `p99 − 12` when a recording's loudest windows are under −28 dB, so a quiet recording is not one giant silence gap. Two code tasks and a close-out, because the change is one function and its test fixture; the weight is in proving that nothing else moves.

## Architecture Decisions

- **One function changes:** `threshold_db` in `crates/decompile/src/silence.rs` becomes `max(max(min(−40, p99 − 12), −70), min(p10 + 6, p90 − 12))`, with the new constants `LOUD_PERCENTILE = 99` and `FLOOR_LIMIT_DB = −70`. The bound was added during planning (advisor review): without it a recording with under 1% sound gets a threshold below its own silence and loses its gaps. `analyze`, `detect_gaps`, the burst estimator in `asr.rs`, and `diff` all read the threshold from there, so nothing else needs to learn about it. The result is stored in `silence_threshold_db`, so the JSON already says what was used.
- **New constants, not a new knob:** `LOUD_PERCENTILE` and `FLOOR_LIMIT_DB` next to the existing `FLOOR_DB`, `NOISE_MARGIN_DB`, and `HEADROOM_DB`. No flag, no environment variable, no schema change (spec Resolved Question 1).
- **Percentiles keep the existing nearest-rank `rank`.** The Python model in the spec used the same rule and reproduced the binary's p10 to 0.03 dB.
- **Baseline first.** Before any change, save every fixture's `edit_dna.json` and the real iPhone recording's `silence_gaps` and `silence_threshold_db` from the current build. After Task 2 they must match apart from timings (criterion 3). The Reels file's before-value is already known: one gap, 0–15,092 ms, threshold −40.0.
- **`quiet12` is a 440 Hz tone at −46 dB peak, which `window_db` reads as −49 dB RMS** (checked with ffmpeg `astats`: lavfi `sine` has a fixed peak of −18 dBFS, so the fixture applies `volume=-28dB`). It has a planted 0.5 s mute in 12 s, about 4% of the windows, so p10 is the tone (−49), not the mute. The threshold is then `max(max(min(−40, −49 − 12), −70), min(−49 + 6, −49 − 12)) = max(−61, −61) = −61`: the mute (−120) is a gap and the tone (−49) is not. Task 1 checks the window level of the file with ffmpeg, not only that it exists.
- **Unit tests pin the cases the first drafts of the rule would have broken:** 95 windows at −120 plus 5 at −14, and 92 at −60 plus 8 at −20, both staying at −40; and 995 at −120 plus 5 at −14, which the bound holds at −70 instead of −132.

## Task List

### Phase 1: The rule
- [ ] Task 1: Baselines, `quiet12`, and the failing tests
- [ ] Task 2: The new threshold

### Checkpoint A
- [ ] `cargo test` passes; every fixture and the real iPhone recording unchanged apart from timings; the Reels file has no whole-clip gap

### Phase 2: Close-out
- [ ] Task 3: Docs, ADR-013, and the real-file numbers

### Checkpoint B
- [ ] All five spec success criteria verified; the `audit` subagent has run on the final commit; reviewed with the user, then `feature/quiet-audio` merged into `main`, and this plan moved to `tasks/archive/quiet-audio/`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| The change moves the threshold of a recording that should not change | High | Baselines of all fixtures and the real recording, compared after Task 2; unit tests for the three mostly-silent shapes; the formula is the old one whenever `p99 − 12 ≥ −40` |
| A quiet recording's threshold lands inside speech, so speech becomes silence | Medium | The floor is `p99 − 12`, 12 dB under the loud part, the same headroom ADR-008 uses; `quiet12` and the Reels file check it; the margin is a named constant |
| The Python model's numbers for the Reels file differ from the binary | Low | Criterion 1 is judged on the real binary, and the model's figures are replaced in the docs with the binary's |
| The real iPhone recording sits near the p99 boundary | Low | Its p90 is −22.6 dB, so its p99 is higher; the baseline comparison would show a change |
| Whisper word ends use gaps, so a changed gap could move words | Low | Only recordings with p99 under −28 dB change, and none of the whisper test inputs are that quiet; the ignored whisper test is run once after the change |
