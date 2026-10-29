# Plan: Smelter render backend

Spec: [smelter-backend](smelter-backend.md). Written 2026-10-29; kept in `docs/specs/` and not in `tasks/` because other branches use `tasks/plan.md` at the same time. Each task ends with its tests green, `cargo fmt --all`, `cargo check --workspace --all-targets` with no warning, and its own commit.

## Tasks

- [x] **0. Find out what Smelter is and whether it runs here.** Docs, releases, licence; v0.6.0 binary for macOS arm64 run on this machine; experiments for seek, offset, scheduled scene updates, end of output, unsupported media, encoder options. Recorded in the spec's Evidence.
- [x] **0b. Fix a pre-existing warning** (`TRANSNET_NOTE` unused import in `crates/decompile/src/pipeline.rs`) so the workspace builds with none. Own commit.
- [x] **1. Spec and plan** (this file and the spec).
- [x] **2. Segments and the Smelter plan, red then green** (`crates/timeline/src/smelter.rs`): inputs, scenes, punch-in sampling, card, gap, audio, clock input, refusals, determinism, requests in fixed order. Tests first.
- [x] **3. Agreement test** between the ffmpeg graph's trims and the Smelter requests, over projects made by the real edit operations.
- [x] **4. HTTP and WebSocket client** (`crates/reelctl/src/smelter_client.rs`) with a loopback fake in its tests.
- [x] **5. The backend** (`smelter_backend.rs`): resolve, spawn, ready, register, start, wait, log scan, frame check, rename; guard; tests with the fake and shell-script children.
- [x] **6. The trait and `--backend`** (`render_backend.rs`, small edits to `timeline_cmd.rs` and `main.rs`): ffmpeg behaviour unchanged; CLI tests.
- [x] **7. Real run.** `fixtures/smelter_agreement.sh`; measure on `draft12` and `textured_cuts12`; many-input stress; record the numbers in status.
- [x] **8. Mutation pass** on the pure translation; fix survivors; list the rest.
- [x] **9. Docs:** ADR-023, README, status, cli, CHANGELOG, AGENTS.md, timeline.md, p0-capability-map.md.
- [x] **10. Full `cargo test --workspace` once, audit commit, final report.**
