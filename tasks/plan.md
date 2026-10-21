# Implementation Plan: Real word timing with whisper.cpp

Spec: [docs/specs/whisper-asr.md](../docs/specs/whisper-asr.md), approved 2026-10-20. This plan was approved the same day. Branch: `feature/whisper-asr`. Checklist: [tasks/todo.md](todo.md).

## Overview

Link whisper.cpp through `whisper-rs` 0.16.0 and use it for `words` whenever a model file is given; keep the burst estimator otherwise. Six tasks, riskiest first: prove the build, then the pure pieces (resampler, segment mapping), then the wiring, then the end-to-end test and measurements. Each task updates its docs in the same commit.

## Architecture Decisions

- `whisper-rs` goes in `[workspace.dependencies]` and is used only by `decompile`, like the existing dependencies.
- Two new modules in `decompile`: `resample.rs` (48 kHz to 16 kHz) and `whisper.rs` (model loading, parameters, segment mapping). `asr.rs` keeps the burst estimator and becomes the dispatcher.
- `asr::transcribe` returns a `Result`. A bad model path becomes a new `DecompileError::Whisper` variant, which the CLI reports with exit code 1.
- `words_from_segments` is a pure function over (text, start, end) in centiseconds, so the mapping is unit-tested without a model.
- Resampling runs inside the ASR stage, so it counts toward `asr_ms`, not `signal_ms`.
- `asr_note` gives the model's file name, not its full path, so `edit_dna.json` does not record local directory names.
- Tests that spawn `reelctl` remove `REELCTL_WHISPER_MODEL` and `REELCTL_TRANSNET_WEIGHTS` from the child's environment. Otherwise exporting the model path in a shell profile (the spec's answer to open question 3) would run the sine-tone fixtures through whisper and break the default suite.

## Task List

### Phase 1: Prove the build
- [x] Task 1: `whisper-rs` builds and links

### Phase 2: Pure pieces
- [x] Task 2: Resample 48 kHz to 16 kHz
- [x] Task 3: whisper module

### Checkpoint A
- [x] `cargo test` passes and the release build is clean

### Phase 3: Wire it in
- [x] Task 4: Use whisper for words when a model is given

### Phase 4: Prove it end to end
- [ ] Task 5: `speech12` fixture and end-to-end tests
- [ ] Task 6: Measure and close out

### Checkpoint B
- [ ] All seven spec success criteria verified, reviewed with the user, merged into `main`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| `whisper-rs-sys` fails to build with CMake 4.4 or to link on Apple Silicon | High | Task 1 comes first. Fallbacks: set `CMAKE_POLICY_VERSION_MINIMUM=3.5`, which `whisper-rs-sys` passes to CMake, or add the link flags from its `BUILDING.md` to `.cargo/config.toml` |
| Word times too loose for the 200 ms pause check | Medium | **Happened in Task 3's manual run.** On a `say` clip whose 1.2 s pause runs from 0.78 s to 2.00 s, the plain timestamps put "dead" at 0.91–2.00 s, across the pause, and "air." at 2.00 s. With DTW timestamps (`base.en` preset) every word fell on the correct side of the pause. On 60 s of a real iPhone recording, DTW took 13.3 s against 4.3 s without it (flash attention was off in both runs). whisper's output there was an invented, repeated sentence, so the timing could not be compared. The speech is Hinglish, so on 2026-10-21 the user chose Oriserve Whisper-Hindi2Hinglish-Apex (`ggml-apex-hinglish-q8_0.bin`, large-v3-turbo fine-tune, 874,188,075 bytes, SHA-256 `bef6d9d5…0d4c`). On the same 60 s it transcribed real Roman-script Hinglish (183 words). Against the 19 pauses of 0.3 s or more at −25 dB, plain timestamps stretched 21 of 183 words more than 200 ms into a pause; DTW (`LargeV3Turbo` preset) placed 0 of 183 there. Both took 14.5 s. The spec was revised for DTW and multilingual models and approved on 2026-10-21; Task 4 implements it. Remaining: word ends stop at −40 dBFS silence gaps, so under room noise 9 of 183 end inside a pause |
| whisper invents words on tones or music | Low | The default tests never run whisper, and the spawned test processes get no model path |
| Slower builds | Low | whisper.cpp compiles once and is cached; the first-build time goes in `docs/status.md` |
| The `Samantha` voice is missing on another Mac | Low | `generate.sh speech12` fails with `say`'s error; the clip is documented as macOS-only |

## Open Questions

None. The spec's three questions were answered with the recommended options on 2026-10-20.
