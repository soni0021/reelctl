# Implementation Plan: Silence relative to the noise floor

Spec: [docs/specs/relative-silence.md](../docs/specs/relative-silence.md), approved 2026-10-25. Branch: `feature/relative-silence`. Checklist: [tasks/todo.md](todo.md). The previous plan, for whisper word timing, is in [tasks/archive/whisper-asr/](archive/whisper-asr/plan.md).

## Overview

Replace the fixed −40 dBFS silence threshold with `max(−40, min(p10 + 6, p90 − 12))` dB per recording, end a gap only after 30 ms of sound, share the threshold with the burst estimator, and record the threshold and noise floor in `edit_dna.json`. Five tasks: the pure threshold rule, then the gap change wired through the pipeline (the risky one, because it must leave every existing clip's gaps identical), then the schema, then a noisy fixture with an end-to-end test, then measurements on real footage. Each task updates its docs in the same commit.

## Architecture Decisions

- **Levels are computed once.** `silence::window_levels(pcm, sample_rate)` returns one dB value per 10 ms window. The threshold, the gaps, and the noise floor all read from it. Today `window_db` runs in both `silence.rs` and `asr.rs`.
- **One result type.** `silence::analyze(pcm, sample_rate) -> SilenceAnalysis { gaps, threshold_db, noise_floor_db }`. The pipeline passes `&SilenceAnalysis` to `asr::transcribe`, which hands the gaps to whisper and the threshold to the burst estimator.
- **Percentile:** nearest rank, `sorted[min(n - 1, floor(p / 100 × n))]`. That matches the Python model the spec's numbers came from.
- **No audio:** an empty level list gives a threshold of −40 and a noise floor of `None`.
- **Schema fields:**
  - `silence_threshold_db: f64`. A file without it deserializes as −40, which is true of every version-1 file.
  - `noise_floor_db: Option<f64>`. It is `null` when there is no audio, and a file without it deserializes as `null`, because version-1 files never measured it.
  - This is how the spec's "`#[serde(default)]` on both fields" is carried out without inventing a value. Both fields are rounded to 2 decimals, like `integrated_lufs`. `SCHEMA_VERSION` becomes 2.
- **Timing:** the threshold calculation sits inside the existing silence stage, so it counts toward `signal_ms` (spec Behavior).
- **Fixed values:** all the rule's numbers (−40, 6, 12, 30 ms, and the 80 ms minimum gap) are named constants in `silence.rs`, with the evidence for each summarized in the module comment.

## Task List

### Phase 1: The rule
- [ ] Task 1: Threshold from window levels (pure functions)
- [ ] Task 2: Gaps and bursts use the threshold, and gaps survive short sounds

### Checkpoint A
- [ ] `cargo test` passes; the four synthetic clips' `silence_gaps` match the baseline captured before Task 2; the real clip meets spec criterion 3

### Phase 2: Output and proof
- [ ] Task 3: Record the threshold and noise floor in `edit_dna.json`
- [ ] Task 4: `noisy12` fixture and end-to-end test
- [ ] Task 5: Measure and close out

### Checkpoint B
- [ ] All seven spec success criteria verified, reviewed with the user, merged into `main`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| The 30 ms rule merges short gaps on an existing clip, which breaks success criterion 2 | Medium | Before Task 2, save each synthetic clip's `silence_gaps` from the `main` build, and compare all gaps afterwards, not only those of 180 ms or more. If only gaps under 180 ms differ, stop and ask before changing the criterion |
| The rule was tuned on one real recording; other rooms, music beds, or loud background speech are unmeasured | Medium | Report the threshold and noise floor in every output (Task 3), so a bad call can be seen. Record the single-recording limit in `docs/status.md` and ADR-008 |
| More gaps mean more whisper word ends cut at a gap that is really speech | Low | Spec criteria 3 and 4 check word starts and word ends on the real clip in Checkpoint A and Task 5 |
| Without a model, `words_per_minute` changes on noisy recordings, because bursts now split at the new threshold | Low | Expected. CHANGELOG entry in Task 2. The synthetic clips keep their bursts, because their threshold change only moves between levels no window has |
| The white-noise level in `noisy12` sits too close to −40 dB or to the computed threshold | Low | Task 4 checks that every pause window is at least 1 dB above −40 and at least 3 dB below the threshold, and changes the amplitude (spec: 0.02 gives −39.2 to −38.4 dB) if not |

## Open Questions

None. The spec's three questions were answered with the recommended options on 2026-10-25.
