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
- [ ] Task 2: Resample 48 kHz to 16 kHz
- [ ] Task 3: whisper module

### Checkpoint A
- [ ] `cargo test` passes and the release build is clean

### Phase 3: Wire it in
- [ ] Task 4: Use whisper for words when a model is given

### Phase 4: Prove it end to end
- [ ] Task 5: `speech12` fixture and end-to-end tests
- [ ] Task 6: Measure and close out

### Checkpoint B
- [ ] All seven spec success criteria verified, reviewed with the user, merged into `main`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| `whisper-rs-sys` fails to build with CMake 4.4 or to link on Apple Silicon | High | Task 1 comes first. Fallbacks: set `CMAKE_POLICY_VERSION_MINIMUM=3.5`, which `whisper-rs-sys` passes to CMake, or add the link flags from its `BUILDING.md` to `.cargo/config.toml` |
| Word times too loose for the 200 ms pause check | Medium | Measured in Task 5. If they fail, stop and ask about DTW timestamps (the spec says ask first) |
| whisper invents words on tones or music | Low | The default tests never run whisper, and the spawned test processes get no model path |
| Slower builds | Low | whisper.cpp compiles once and is cached; the first-build time goes in `docs/status.md` |
| The `Samantha` voice is missing on another Mac | Low | `generate.sh speech12` fails with `say`'s error; the clip is documented as macOS-only |

## Open Questions

None. The spec's three questions were answered with the recommended options on 2026-10-20.
