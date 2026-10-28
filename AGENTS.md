# Agent notes

The product spec is [planer.md](planer.md). Read it before changing the engine. Current state, measurements, and known gaps are in [docs/status.md](docs/status.md).

Read CONSTRAINTS.md before writing code. Do not weaken it to make a change pass.

This repository is a Cargo workspace. P0 is the Rust decompiler and reel-diff CLI:

- `crates/reelctl` — `reelctl decompile`, `reelctl diff`, `reelctl understand`, and `reelctl look`
- `crates/edit-dna` — serde types and JSON schema for `edit_dna.json`
- `crates/decompile` — decode, cuts, silence, loudness, per-shot color
- `crates/diff` — pacing flags
- `crates/filmstrip` — `timeline_view` PNGs at cut boundaries
- `crates/look` — `reelctl look`: per-channel histogram matching and the `.cube` writer and reader (pure)
- `crates/timeline` — the OpenTimelineIO subset, edit operations and the ffmpeg filter-graph builder behind `reelctl timeline|transform|funnel|render|qa` (pure; the commands in `crates/reelctl/src/timeline_cmd.rs` run `ffprobe` and `ffmpeg`)
- `crates/understand` — `reelctl understand`: transcript segments, frame plan, the model request and cost bound, clip rules, and `understanding.json` (no dependency on `decompile`)
- `watchtower/` — the Go module for P1 (outlier and growth digest); `cd watchtower && go test ./...`; reads recorded data, calls the `reelctl` binary as a process, no network in tests
- `fixtures/` — synthetic ffmpeg clips (no scraped Reels)

`reelctl decompile` writes `edit_dna.json` and one `timeline_view` PNG per cut. `signal_ms` is structural extraction only. `asr_ms` is word timing and is not part of the 500ms gate.

P1–P3 in planer.md (Go watchtower, OpenTimelineIO / Smelter / manimgx, Qwen3-VL) are specified there. Built so far: a first slice of P1 (`watchtower/`, recorded data, no live Virlo client) and of P2 (OpenTimelineIO edits and an ffmpeg render, no Smelter, manimgx or RobustVideoMatting); P3 is deferred. Also, `reelctl understand` gives the vision-language role a first, command-line implementation through a hosted model ([ADR-015](docs/decisions/ADR-015-combine-measured-time-with-a-model-that-answers-by-segment-id.md)). Do not vendor the reference repos listed in planer.md. Optional P2+ reel worker (spec only): `edenfunf/reelmimic` may restyle 2D explainer/B-roll from an outlier reference via `reelctl motion mimic-2d`; it is not P0 decompile and is not live-action, talking-head, face, or lip-sync.

## Verify

- After a code change: `cargo check` on edit, `cargo test` once at task end. Do not re-run the suite on unchanged code.
- The exception: after a task's commit, the `audit` subagent runs the suite again on a clean checkout of that commit. It catches a file that was never `git add`ed, which a run in the working tree cannot.
- A green P0 run is the workspace unit tests plus `crates/reelctl/tests/p0_fixture.rs` (synthetic `fixtures/generate.sh` clips: planted cut, silence, a pause under noise, static shot, payoff, 10-bit HLG video) and `whisper_fixture.rs`. No network, no scraped Reels.
- The whisper model test is `#[ignore]`d. After changing word timing, also run `REELCTL_WHISPER_MODEL=~/models/whisper/ggml-apex-hinglish-q8_0.bin cargo test -p reelctl --test whisper_fixture -- --include-ignored` (macOS only: it needs `say`). Tests that spawn `reelctl` must remove `REELCTL_WHISPER_MODEL`, `REELCTL_TRANSNET_WEIGHTS` and `OPENROUTER_API_KEY` from its environment.
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
- Optical-flow Bézier zoom curves stay `StubbedPass` (`status: unimplemented`) until the `zoom-curves` module lands. A `.cube` LUT is made by `reelctl look` from two videos; `cube_lut` in `edit_dna.json` stays `unimplemented` because one video cannot make one. Punch-in zooms, zoom-outs and overlay changes are measured as edit events (`visual_events`, module `visual-motion` in [docs/specs/p0-capability-map.md](docs/specs/p0-capability-map.md)); only moves that complete within 12 frames count, so a slow drift is a zoom-curves matter.

## Learned User Preferences

- Strictly follow the project skills in `.agents/skills/`, including the `addyosmani/agent-skills` pack from `npx skills add addyosmani/agent-skills`.
- Always write or update docs as part of every change (see Docs above).

## Learned Workspace Facts

- Project skills live in `.agents/skills/` and are pinned in `skills-lock.json`: `actionbook/rust-skills` (`coding-guidelines`, `domain-cli`, `m10-performance`), `digitalsamba/claude-code-video-toolkit` (`ffmpeg`), `affaan-m/ecc` (`video-editing`), and `addyosmani/agent-skills`. Leave text-to-video, Remotion, Manim, Seedance, Hyperframes, and fal.ai skills out. `edenfunf/reelmimic` is an allowed optional 2D restyle worker for informational reels (see planer.md); do not vendor it or add HyperFrames as a skill.
- Claude Code setup: subagents `search`, `grep`, and `audit`, plus an `Explore` that replaces the built-in one, are in `.claude/agents/`, all on Haiku 5.5 at effort `high`; the delegation rule is in `.claude/rules/delegation.md`, and `.claude/settings.json` sets `worktree.baseRef` to `head` so `audit` sees the current branch. Do not add a `CLAUDE.md` or `CLAUDE.local.md`: Claude Code then stops reading this file.
- HEVC decodes on the Apple media engine (VideoToolbox) on macOS, through `crates/decompile/src/hwdecode.rs`, the only module with `unsafe` code; `--decoder software` forces software. Both decoders filter every frame and give the same cuts, scores, colors, and thumbnails ([ADR-012](docs/decisions/ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md)). A test that wants hardware must check that `decode_note` says `Apple media engine`: a silent fallback would match software by construction.
- `reelctl understand` is the only command that uses the network. The key is `OPENROUTER_API_KEY` from the environment (never read from `.env` by the program, never printed or written). Every request denies data collection, and tests never call a model: they use `--replay` and `--catalog`. A live call spends money and sends frames and a transcript off the machine, so it needs the user's yes each time.
- `Reels/` holds real downloaded reels, the helper scripts, and their analysis output. It is git-ignored; never commit anything from it.
- Structural decompile uses `ffmpeg-next` with no ffmpeg shell-out. Synthetic clips come from `fixtures/generate.sh`.
- `reelctl diff` flags dead air longer than 180ms, a visual window longer than 2.5s with no cut and no `visual_events` entry, and a missing payoff cue in the 8–10s window when more than half of the outliers (one to three, `--outlier` repeated) have one. It also prints a `comparison` (metrics, cut counts in 5 s windows, `rhythm_gap`, `pause_density`; `--format text` for a table). Silence detection uses a per-recording threshold, `max(max(min(−40, p99 − 12), −70), min(p10 + 6, p90 − 12))` dB over 10 ms window levels, and is separate from the 180ms dead-air flag ([ADR-008](docs/decisions/ADR-008-silence-relative-to-the-noise-floor.md), [ADR-013](docs/decisions/ADR-013-the-silence-floor-follows-a-quiet-recording.md)).
