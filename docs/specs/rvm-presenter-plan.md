# Plan: `rvm-presenter`

`tasks/plan.md` belongs to other work in parallel, so this plan lives beside its spec ([rvm-presenter](rvm-presenter.md)). Each task was written with its tests; ticked in the commit that finished it.

## Tasks

- [x] 1. Spec, plan, crate skeleton (`crates/compositor`, `#![forbid(unsafe_code)]`), ADR-021. `cargo check` passes.
- [x] 2. Pure core: `ratio`, `tensor`, `composite`, `background`, `model` trait, with their tests (ratio table, planar conversion, composite arithmetic, alpha clamp and non-finite refusal, colour parsing).
- [x] 3. `pipeline`: exact frame reads, state feedback (hostile fake), ratio and size reach the model, errors carry the frame number.
- [x] 4. `ort_model`: weights and library checks (as TransNet), model signature check, `float32` and `float16` adapters, `release_runtime`.
- [x] 5. `media`: probe parser (rotation, rates, HDR), child processes, temporary file and rename. Integration tests with real `ffmpeg` and a fake model.
- [x] 6. CLI `reelctl compositor isolate-presenter` and its failure tests.
- [x] 7. Docs: README, `docs/cli.md`, `docs/status.md`, CHANGELOG, AGENTS.md, capability map, ADR-021.
- [x] 8. Real-model manual runs (`fp32`, `fp16`), Python reference comparison, numbers in `status.md`.
- [x] 9. Mutation pass on the pure core; two survivors got tests, and the colour mutants led to a real fix (tags).
- [x] 10. `cargo fmt`, `cargo check --workspace --all-targets` (zero warnings), `cargo test --workspace`.

## Notes

- The tests were written with the code, not one red run at a time: the evidence that they bite is the mutation pass in [status](../status.md#rvm-presenter-2026-10-29) (49 of 51 first-run kills, the two survivors then killed).
- The state feedback and the colour handling are the two ways this can be silently wrong with a fake model passing, so the hostile fake (task 3) and the alpha-1-everywhere colour test plus the tag test (task 5) came before the CLI, and the Python comparison (task 8) is the only check that proves the real tensors are laid out right.
- ONNX Runtime 1.21 aborts at exit unless the environment is released; the manual runs check the exit status is 0 (it was, every run).
- Left to a later slice: OpenTimelineIO placement of a matted clip, HDR input, a GPU provider, a shared ONNX environment for a command that needs both models.
