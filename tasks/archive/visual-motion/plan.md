# Plan: visual-motion

Spec: [docs/specs/visual-motion.md](../../../docs/specs/visual-motion.md), written 2026-10-28 under "Do all of them". Branch: `feature/visual-motion`. Checklist: [todo.md](todo.md).

## Overview
Measure punch-ins, zoom-outs and overlay changes from the luma grids the decoder already builds, write them to `edit_dna.json` as `visual_events` (schema 3), and split `static_window` at them in `diff`.

## Dependency graph
```
edit-dna type + schema ──► diff change ─────────────┐
motion detector (pure, on grids) ──► decode.rs wiring ──► p0 end-to-end ──► real footage + cost
fixtures (generate.sh) ───────────────────────────────┘
```
The first three tasks touch different files and run in parallel in worktrees; the wiring, the end-to-end tests and the real-footage measurement follow in the main session.

## Tasks
1. **Type and diff** (`crates/edit-dna`, `crates/diff`, `docs/edit-dna.md`, schema): `VisualEvent`, `visual_events` defaulted, version 3, `static_window` split at events, tests.
2. **Detector** (`crates/decompile/src/motion.rs`): pure functions over luma grids, unit tests on synthetic grids, mutation checks.
3. **Fixtures** (`fixtures/generate.sh`): `motion12`, `still12`, `motion12_wide`, `bench60_textured`, sidecar JSON of planted events.
4. **Wiring** (`decode.rs`, `pipeline.rs`): feed the grids, map events, add the time to `signal_ms`.
5. **End to end** (`p0_fixture.rs`) and the `textured_cuts12` probe.
6. **Real footage**: the NEET video and the two reels, cost, docs, status.

## Risks
| Risk | Impact | Mitigation |
|---|---|---|
| Thresholds fit the synthetic clips only | High | Tune on the NEET video and two reels as well; record the limit |
| The scale fit costs too much on real footage | Medium | Run it only above the plain-difference floor; measure; ADR on a breach |
| A speaker's motion reads as an overlay | Medium | Quiet-remainder and persistence rules; tests with a moving block |
