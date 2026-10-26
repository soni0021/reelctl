# Agent notes

The product spec is [planer.md](planer.md). Read it before changing the engine. Current state, measurements, and known gaps are in [docs/status.md](docs/status.md).

Read CONSTRAINTS.md before writing code. Do not weaken it to make a change pass.

This repository is a Cargo workspace. P0 is the Rust decompiler and reel-diff CLI:

- `crates/reelctl` — `reelctl decompile` and `reelctl diff`
- `crates/edit-dna` — serde types and JSON schema for `edit_dna.json`
- `crates/decompile` — decode, cuts, silence, loudness, per-shot color
- `crates/diff` — pacing flags
- `crates/filmstrip` — `timeline_view` PNGs at cut boundaries
- `fixtures/` — synthetic ffmpeg clips (no scraped Reels)

`reelctl decompile` writes `edit_dna.json` and one `timeline_view` PNG per cut. `signal_ms` is structural extraction only. `asr_ms` is word timing and is not part of the 500ms gate.

P1–P3 in planer.md (Go watchtower, OpenTimelineIO / Smelter / manimgx, Qwen3-VL) are specified there and are not implemented in this tree. Do not vendor the reference repos listed in planer.md. Optional P2+ reel worker (spec only): `edenfunf/reelmimic` may restyle 2D explainer/B-roll from an outlier reference via `reelctl motion mimic-2d`; it is not P0 decompile and is not live-action, talking-head, face, or lip-sync.

## Verify

- After a code change: `cargo check` on edit, `cargo test` once at task end. Do not re-run the suite on unchanged code.
- The exception: after a task's commit, the `audit` subagent runs the suite again on a clean checkout of that commit. It catches a file that was never `git add`ed, which a run in the working tree cannot.
- A green P0 run is the workspace unit tests plus `crates/reelctl/tests/p0_fixture.rs` (synthetic `fixtures/generate.sh` clips: planted cut, silence, a pause under noise, static shot, payoff, 10-bit HLG video) and `whisper_fixture.rs`. No network, no scraped Reels.
- The whisper model test is `#[ignore]`d. After changing word timing, also run `REELCTL_WHISPER_MODEL=~/models/whisper/ggml-apex-hinglish-q8_0.bin cargo test -p reelctl --test whisper_fixture -- --include-ignored` (macOS only: it needs `say`). Tests that spawn `reelctl` must remove `REELCTL_WHISPER_MODEL` and `REELCTL_TRANSNET_WEIGHTS` from its environment.
- CLI: `reelctl decompile --input <reel.mp4> --out <edit_dna.json>` and `reelctl diff --draft <draft.mp4> --outlier <edit_dna.json>`.
- `signal_ms` must be a measured structural duration (decode, cuts, silence, loudness, histograms, JSON write). Do not invent or pad it.

## Docs

Update the docs in the same commit as every change, without waiting to be asked; the user requires it. A task is not done until these files match the code:

- `README.md`: overview, quick start, commands, crate list.
- `docs/status.md`: what works, measured numbers (with date and machine), gaps, next steps.
- `docs/cli.md`: `reelctl` flags, output, exit codes, input requirements.
- `docs/edit-dna.md`: every `edit_dna.json` field. Regenerate the schema with `cargo run -p edit-dna --example schema` when the types change.
- `docs/decisions/`: one ADR per significant decision, named `ADR-NNN-short-title.md` with the next number. When a decision changes, add a new ADR and mark the old one superseded; never delete ADRs.
- `docs/specs/`: one spec per feature, approved by the user before coding starts (spec-driven-development). Keep it current while building.
- `tasks/plan.md` and `tasks/todo.md`: the active implementation plan and its checklist (planning-and-task-breakdown). Tick a task's boxes in the commit that finishes it. When a feature is merged, move both into `tasks/archive/<feature>/`.
- `CHANGELOG.md`: an entry under Unreleased for each change a `reelctl` user would notice.

## P0 gaps (documented, do not implement here)

- whisper.cpp runs only when `--whisper-model` or `REELCTL_WHISPER_MODEL` names a model; weights are never downloaded or committed. Without one, `asr_backend` is `silence_bursts` and `asr_ms` times that estimator.
- TransNetV2 / `ort` is unlinked: weights are not vendored. `--cut-detector auto` uses the FFmpeg scene-score `CutDetector` unless a weight file exists (then detection still fails closed until an ort-backed build is wired).
- Optical-flow Bézier zoom curves and `.cube` LUT stay `StubbedPass` (`status: unimplemented`). Those are Pass 1b, not this tree. Detecting punch-in zooms and overlay changes as edit events is in P0 (module `visual-motion` in [docs/specs/p0-capability-map.md](docs/specs/p0-capability-map.md), approved 2026-10-25).

## Learned User Preferences

- Strictly follow the project skills in `.agents/skills/`, including the `addyosmani/agent-skills` pack from `npx skills add addyosmani/agent-skills`.
- Always write or update docs as part of every change (see Docs above).

## Learned Workspace Facts

- Project skills live in `.agents/skills/` and are pinned in `skills-lock.json`: `actionbook/rust-skills` (`coding-guidelines`, `domain-cli`, `m10-performance`), `digitalsamba/claude-code-video-toolkit` (`ffmpeg`), `affaan-m/ecc` (`video-editing`), and `addyosmani/agent-skills`. Leave text-to-video, Remotion, Manim, Seedance, Hyperframes, and fal.ai skills out. `edenfunf/reelmimic` is an allowed optional 2D restyle worker for informational reels (see planer.md); do not vendor it or add HyperFrames as a skill.
- Claude Code setup: subagents `search`, `grep`, and `audit`, plus an `Explore` that replaces the built-in one, are in `.claude/agents/`, all on Haiku 5.5 at effort `high`; the delegation rule is in `.claude/rules/delegation.md`, and `.claude/settings.json` sets `worktree.baseRef` to `head` so `audit` sees the current branch. Do not add a `CLAUDE.md` or `CLAUDE.local.md`: Claude Code then stops reading this file.
- Structural decompile uses `ffmpeg-next` with no ffmpeg shell-out. Synthetic clips come from `fixtures/generate.sh`.
- `reelctl diff` flags dead air longer than 180ms, a visual window longer than 2.5s with no cut, and a missing payoff cue in the 8–10s window when the outlier has one. Silence detection uses a per-recording threshold, `max(−40, min(p10 + 6, p90 − 12))` dB over 10 ms window levels, and is separate from the 180ms dead-air flag ([ADR-008](docs/decisions/ADR-008-silence-relative-to-the-noise-floor.md)).
