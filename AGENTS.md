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
- `crates/timeline` — the OpenTimelineIO subset, edit operations, the ffmpeg filter-graph builder and the Smelter request builder (`smelter.rs`) behind `reelctl timeline|transform|funnel|render|qa` (pure; the commands in `crates/reelctl/src/timeline_cmd.rs` run `ffprobe` and `ffmpeg`; `render_backend.rs`, `smelter_backend.rs` and `smelter_client.rs` in `crates/reelctl/src` run the optional Smelter backend, `reelctl render --backend smelter`: the client talks to 127.0.0.1 only, but the Smelter server it starts listens on every interface for the length of a render, so `reelctl understand` is no longer the only command that touches a network)
- `crates/compositor` — `reelctl compositor isolate-presenter`: RobustVideoMatting presenter cut-out. A pure core (downsample ratio, recurrent state, alpha, compositing, frame pipeline) behind a `MattingModel` trait, the `ort` session (`ORT_DYLIB_PATH`, `--weights` or `REELCTL_RVM_WEIGHTS`), and `ffprobe`/`ffmpeg` command line edges in `media.rs`; `#![forbid(unsafe_code)]`; its own ONNX environment, so `main` calls `compositor::release_runtime()` as well as `decompile::release_onnx_runtime()`, and one process must not use both models
- `crates/scenes` — chart specs, the fixed `manimgx` scene template and the fail-closed worker runner behind `reelctl overlay add-chart|remove` (the track and the ffmpeg overlay filters are in `crates/timeline`; the command is `crates/reelctl/src/overlay_cmd.rs`)
- `crates/understand` — `reelctl understand`: transcript segments, frame plan, the model request and cost bound, clip rules, and `understanding.json` (no dependency on `decompile`)
- `crates/studio` — the core of `reelctl serve` ([spec](docs/specs/studio-service.md)), being built: the content-addressed store (`store.rs`: BLAKE3 objects, read-only, tree objects for folders, no symbolic links) the command catalogue (`commands.rs`: the one place a job's argv is built, from typed arguments, input and output roles; `make-reels` uses it too), jobs and the producer–consumer queue (`jobs.rs`, `queue.rs`: per-class limits, `after` chains, cancel, restart recovery, cache hits, a `Runner` trait so tests need no process), checkpoints (`checkpoint.rs`: append-only, revert appends a copy, branch makes a subject, the cache key), and the SQLite catalogue (`catalogue.rs`: `rusqlite` bundled, WAL, schema version 1, a newer schema refused untouched; assets imported by reference; never a secret). `#![forbid(unsafe_code)]`
- `crates/progress` — `--progress json` events (NDJSON on stderr, schema 1: stage, progress, warning, result, error) and cancellation: the cancel flag a SIGTERM/SIGINT handler sets (`ctrlc` in `main`), polled by the decode and matting loops, and a registry of child pids that get SIGTERM; a cancelled run exits 130 and its temporary files are removed by their guards. Every new long loop ticks it, every new child process is `progress::track`ed, and every result line goes through `out!` (or `progress::result_line`)
- `crates/dataset` — `reelctl dataset build` and `reelctl eval`: JSONL instruction-tuning records from `edit_dna.json` and `understanding.json`, split by reel, and the metrics that score a model's answers (pure; takes bytes, returns bytes; the commands in `crates/reelctl/src/dataset_cmd.rs` read and write the files; no training, no model call)
- `watchtower/` — the Go module for P1 (outlier and growth digest); `cd watchtower && go test ./...`; reads recorded data, calls the `reelctl` binary as a process, no network in tests
- `fixtures/` — synthetic ffmpeg clips (no scraped Reels)

`reelctl decompile` writes `edit_dna.json` and one `timeline_view` PNG per cut. `signal_ms` is structural extraction only. `asr_ms` is word timing and is not part of the 500ms gate.

P1–P3 in planer.md (Go watchtower, OpenTimelineIO / Smelter / manimgx, Qwen3-VL) are specified there. Built so far: a first slice of P1 (`watchtower/`, recorded data, no live Virlo client) and of P2 (OpenTimelineIO edits and an ffmpeg render, and RobustVideoMatting presenter cut-out, `reelctl compositor isolate-presenter`, chart overlays through a separately installed manimgx, `reelctl overlay add-chart` ([ADR-022](docs/decisions/ADR-022-draw-chart-overlays-with-a-separately-installed-manimgx-and-key-them-over-the-video.md); the real manimgx was not run when it was built, only a fake), an optional Smelter render backend driven over its HTTP API ([ADR-023](docs/decisions/ADR-023-drive-smelter-as-a-separate-process-over-its-http-api.md)); Smelter is a separate program: never download, vendor or commit it, never start it with its web renderer, WHIP or RTMP servers, and never claim frame accuracy for it (see status). `fixtures/smelter_agreement.sh` is the manual check against a real server; tests use a fake on 127.0.0.1 and remove `SMELTER_BIN` from the child environment); P3 training is deferred by the user, but its offline half is built (`crates/dataset`, [docs/finetune.md](docs/finetune.md)). Also, `reelctl understand` gives the vision-language role a first, command-line implementation through a hosted model ([ADR-015](docs/decisions/ADR-015-combine-measured-time-with-a-model-that-answers-by-segment-id.md)). Do not vendor the reference repos listed in planer.md. Optional P2+ reel worker (spec only): `edenfunf/reelmimic` may restyle 2D explainer/B-roll from an outlier reference via `reelctl motion mimic-2d`; it is not P0 decompile and is not live-action, talking-head, face, or lip-sync.

## Verify

- After a code change: `cargo check` on edit, `cargo test` once at task end. Do not re-run the suite on unchanged code.
- The exception: after a task's commit, the `audit` subagent runs the suite again on a clean checkout of that commit. It catches a file that was never `git add`ed, which a run in the working tree cannot.
- A green P0 run is the workspace unit tests plus `crates/reelctl/tests/p0_fixture.rs` (synthetic `fixtures/generate.sh` clips: planted cut, silence, a pause under noise, static shot, payoff, 10-bit HLG video) and `whisper_fixture.rs`. No network, no scraped Reels.
- The whisper model test is `#[ignore]`d. After changing word timing, also run `REELCTL_WHISPER_MODEL=~/models/whisper/ggml-apex-hinglish-q8_0.bin cargo test -p reelctl --test whisper_fixture -- --include-ignored` (macOS only: it needs `say`). Tests that spawn `reelctl` must remove `REELCTL_WHISPER_MODEL`, `REELCTL_TRANSNET_WEIGHTS`, `REELCTL_RVM_WEIGHTS`, `ORT_DYLIB_PATH`, `MANIMGX_BIN` and `OPENROUTER_API_KEY` from its environment, and `SMELTER_BIN` when they test `render --backend smelter`. The manimgx tests use a fake worker script, never the real tool.
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
- TransNetV2 runs through `ort` with a user-supplied ONNX Runtime library (`ORT_DYLIB_PATH`) and weight file (`--transnet-weights` or `REELCTL_TRANSNET_WEIGHTS`); neither is vendored or downloaded by the build. It is slow (about 7 ms per frame, far over the 500 ms budget), so `--cut-detector auto` uses it only when a weights path is given, and falls back to the scene score with the reason in `cut_detector_note` if anything is missing. Library code that opens the model must call `decompile::release_onnx_runtime()` before exiting (ONNX Runtime's static destructors abort otherwise; `reelctl`'s `main` does).
- Zoom curves are measured (`zoom_curves`, schema version 4): one cubic-bezier(1/3, y1, 2/3, y2) per punch-in and zoom-out. Slow drifts have no curve. The `.cube` LUT stays `StubbedPass` in `edit_dna.json`; `reelctl look` makes one from two videos. Punch-in zooms, zoom-outs and overlay changes are measured as edit events (`visual_events`, module `visual-motion` in [docs/specs/p0-capability-map.md](docs/specs/p0-capability-map.md)); only a move that completes within 600 ms (a longer run is a drift) counts, so a slow push-in is not an event.

## P2 gaps (documented)

- `reelctl compositor isolate-presenter` runs RobustVideoMatting (`rvm_mobilenetv3_fp32.onnx` or `_fp16.onnx`, which the user supplies; never vendored or committed) on the CPU through `ort` and an ONNX Runtime library named by an absolute `ORT_DYLIB_PATH`. It is not real time here (about 80 to 180 milliseconds per 720p frame under load; the planer's 172 FPS is a GPU figure), reads SDR video only (HDR is refused), and has no OpenTimelineIO integration. Tests never use a real model: the pipeline runs with a fake `MattingModel`, and the real-model checks are the manual commands in [docs/status.md](docs/status.md#rvm-presenter-2026-10-29). Real clips for a manual run come from `Reels/` and are never copied into the repo.

## Learned User Preferences

- Strictly follow the project skills in `.agents/skills/`, including the `addyosmani/agent-skills` pack from `npx skills add addyosmani/agent-skills`.
- Always write or update docs as part of every change (see Docs above).
- Do future work in a Claude Code cloud session (Linux container), not on the local Mac. There, `cargo`, `ffmpeg`, `ffprobe` and `go` are installed, but there is no VideoToolbox (hardware decode falls back to software), no `say` (the whisper model test cannot run), no `~/models`, no `Reels/`, and no user-supplied ONNX Runtime or weights; say which checks were skipped for that reason, and leave the macOS-only checks to the user.

## Learned Workspace Facts

- Project skills live in `.agents/skills/` and are pinned in `skills-lock.json`: `actionbook/rust-skills` (`coding-guidelines`, `domain-cli`, `m10-performance`), `digitalsamba/claude-code-video-toolkit` (`ffmpeg`), `affaan-m/ecc` (`video-editing`), and `addyosmani/agent-skills`. Leave text-to-video, Remotion, Manim, Seedance, Hyperframes, and fal.ai skills out. `edenfunf/reelmimic` is an allowed optional 2D restyle worker for informational reels (see planer.md); do not vendor it or add HyperFrames as a skill.
- Claude Code setup: subagents `search`, `grep`, and `audit`, plus an `Explore` that replaces the built-in one, are in `.claude/agents/`, all on Haiku 5.5 at effort `high`; the delegation rule is in `.claude/rules/delegation.md`, and `.claude/settings.json` sets `worktree.baseRef` to `head` so `audit` sees the current branch. Do not add a `CLAUDE.md` or `CLAUDE.local.md`: Claude Code then stops reading this file.
- HEVC decodes on the Apple media engine (VideoToolbox) on macOS, through `crates/decompile/src/hwdecode.rs`, the only module with `unsafe` code; `--decoder software` forces software. Both decoders filter every frame and give the same cuts, scores, colors, and thumbnails ([ADR-012](docs/decisions/ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md)). A test that wants hardware must check that `decode_note` says `Apple media engine`: a silent fallback would match software by construction.
- `reelctl understand` is the only command that uses the network. The key is `OPENROUTER_API_KEY` from the environment (never read from `.env` by the program, never printed or written). Every request denies data collection, and tests never call a model: they use `--replay` and `--catalog`. A live call spends money and sends frames and a transcript off the machine, so it needs the user's yes each time.
- `Reels/` holds real downloaded reels, the helper scripts, and their analysis output. It is git-ignored; never commit anything from it.
- Structural decompile uses `ffmpeg-next` with no ffmpeg shell-out. Synthetic clips come from `fixtures/generate.sh`.
- `reelctl diff` flags dead air longer than 180ms, a visual window longer than 2.5s with no cut and no `visual_events` entry, and a missing payoff cue in the 8–10s window when more than half of the outliers (one to three, `--outlier` repeated) have one. It also prints a `comparison` (metrics, cut counts in 5 s windows, `rhythm_gap`, `pause_density`; `--format text` for a table). Silence detection uses a per-recording threshold, `max(max(min(−40, p99 − 12), −70), min(p10 + 6, p90 − 12))` dB over 10 ms window levels, and is separate from the 180ms dead-air flag ([ADR-008](docs/decisions/ADR-008-silence-relative-to-the-noise-floor.md), [ADR-013](docs/decisions/ADR-013-the-silence-floor-follows-a-quiet-recording.md)).
