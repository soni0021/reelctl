# Plan: P3 dataset collector and eval harness

Spec: [p3-dataset-eval.md](p3-dataset-eval.md). Branch `feature/p3-dataset-eval`. This is the feature's plan; `tasks/plan.md` belongs to another feature. Vertical slices, each with a red test first, `cargo check` on edit, `cargo test` once at the end.

## Order and dependencies

```
sha256 -> ids/split -> metrics -> records (types, schema) -> builder -> jsonl/manifest
                                       \-> eval (clip_picks, edit_events, pacing) -> CLI -> end-to-end -> docs, ADR, mutation pass
```

## Tasks

- [x] 1. Spec and plan written (this commit).
- [ ] 2. `crates/dataset` skeleton in the workspace; `sha256` with the NIST vectors.
- [ ] 3. Reel id check and `split_for`; tests for stability, no `val` at 0 percent, distribution.
- [ ] 4. `metrics`: matching, precision/recall/F1 from counts, IoU, means, medians, rates, non-finite refused; hand-made and degenerate cases.
- [ ] 5. Record types, `dataset_schema_version`, JSON schema file and its up-to-date test.
- [ ] 6. Builder: `edit_events`, `pacing` records from `EditDna` with schema gating; skips with reasons; refusal of unknown schema versions.
- [ ] 7. Builder: `clip_picks` from `Understanding` and the DNA; leakage tests (teacher prose, paths); determinism under shuffled input.
- [ ] 8. JSONL writer, manifest, warnings for an empty split.
- [ ] 9. Eval: `clip_picks` scoring and the round-trip test; `edit_events`; `pacing`; the report.
- [ ] 10. `reelctl dataset build` and `reelctl eval` (`crates/reelctl/src/dataset_cmd.rs`), additive edits to `main.rs`.
- [ ] 11. End-to-end test `dataset_fixture.rs` on generated fixtures with the scrubbed environment.
- [ ] 12. Docs: README, `status.md`, `cli.md`, CHANGELOG, AGENTS.md crate list, `finetune.md`, ADR-024, capability map; real command output recorded in `status.md`.
- [ ] 13. Mutation pass on metrics, matching, split, hash; survivors in `status.md`; `cargo fmt --all`, `cargo check --workspace --all-targets` with no warnings, `cargo test --workspace` once.
- [ ] 14. `audit` subagent on the committed branch.

## Commits

1. `docs: spec and plan for the P3 dataset collector and eval harness`
2. `feat: dataset crate with records, builder, metrics and eval` (tasks 2 to 9, with the crate-list docs)
3. `feat: reelctl dataset build and reelctl eval` (tasks 10 to 12)
4. `test: mutation survivors in the dataset metrics and split` (task 13)

## Risks

- **Real transcripts are large** (a 28-minute lecture has 535 segments): a `clip_picks` line can be about 100 KB. Accepted; JSONL streams.
- **Tolerances are guesses**: 100 ms for cuts is three frames at 30 fps, 250 ms for a zoom move. They are flags, not constants in the metric.
- **Few fixture reels** make `val` empty by chance; the fixtures and percentage used in the test and the pasted run are chosen so it is not.
