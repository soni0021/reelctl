# reelctl

`reelctl` measures how a short vertical video is edited and compares a creator's draft against a top-performing reel (an "outlier") in the same niche.

- `reelctl decompile` turns an MP4 into `edit_dna.json`: cut times, punch-ins, zoom-outs and overlay changes measured in pixels (`visual_events`), the easing curve of each punch-in and zoom-out (`zoom_curves`, a cubic Bézier fitted to the measured scale), silence gaps, loudness, per-shot color, whether there is a cut near the 9-second payoff mark, and, given a whisper model, timed words. It also writes a `timeline_view` PNG at each cut, so an agent can inspect cuts without opening the video.
- `reelctl diff` decompiles a draft and flags dead air, long stretches with no cut, and a missing payoff cue. With one to three outliers (`--outlier` up to three times) it also compares cut rhythm in 5 s windows, pause density, speaking rate, and loudness, as JSON or as a text table (`--format text`).

- `reelctl look` builds a `.cube` 3D LUT that moves a draft's colour toward an outlier's by per-channel histogram matching on sampled frames. It works well on the same content graded differently and poorly between very different content (see [docs/cli.md](docs/cli.md)).
- `reelctl timeline`, `transform punch-in`, `funnel attach-bridge`, `render` and `qa inspect-boundaries` edit an OpenTimelineIO project deterministically (cut silence and filler words, punch-in zoom, a long-form bridge card) and render it with `ffmpeg`. See [docs/timeline.md](docs/timeline.md) and [docs/cli.md](docs/cli.md).
- `reelctl compositor isolate-presenter` cuts a presenter out of a video with RobustVideoMatting (an ONNX model you supply, run through `ort` and an ONNX Runtime library named in `ORT_DYLIB_PATH`) and writes them over a colour, image or video background, or writes the alpha matte, or ProRes 4444 with alpha. SDR input only; on a CPU it is not real time. See [docs/cli.md](docs/cli.md) and [docs/specs/rvm-presenter.md](docs/specs/rvm-presenter.md).

- `watchtower/` is a separate Go module (not part of the Cargo workspace) that ranks a niche's creators by outlier multiplier and growth velocity, runs `reelctl decompile` on their breakout Reels, compares the last five breakouts with the baseline posts on measured fields, and writes a digest. It reads recorded JSON files; the live Virlo client is not built. See [docs/watchtower.md](docs/watchtower.md).

- `reelctl understand` takes a 20–60 minute video and its whisper-timed `edit_dna.json`, sends sampled frames and numbered transcript segments to a hosted vision-language model (OpenRouter; needs `OPENROUTER_API_KEY`), and writes `understanding.json`: Reel-sized clips picked by segment ID and checked by code (hook, cliffhanger, length; a clip that breaks a rule is kept with a flag naming it), and a label for what each cut really was. Every quote is whisper's text and every time is measured. See [docs/understanding.md](docs/understanding.md).

This repository is P0 of the plan in [planer.md](planer.md). P1 has a first slice (the Go watchtower, reading recorded data) and P2 has a first slice (OpenTimelineIO edits and an ffmpeg render) and RobustVideoMatting presenter cut-out (`reelctl compositor isolate-presenter`); the rest of P2 (Smelter, manimgx, reframe, captions, voice) and P3 (a fine-tuned vision-language model, deferred by the user) are specified there and not built. Current state, measurements, and known gaps are in [docs/status.md](docs/status.md).

## Quick start

Tested on macOS with Apple Silicon. You need:

- Rust (tested with rustc 1.98.1)
- FFmpeg with its development libraries, and `pkg-config`: `brew install ffmpeg pkg-config` (tested with FFmpeg 8.1). The `ffmpeg` command, with libx264, also generates the test clips.
- CMake: `brew install cmake` (tested with 4.4.4). The build compiles whisper.cpp from source, which took under a minute on an Apple M3 the first time.
- Xcode Command Line Tools, for clang, which compiles whisper.cpp and generates the FFmpeg and whisper.cpp bindings
- Optional, for real words: a ggml whisper model, kept outside the repo. For English, `curl -L -o ~/models/whisper/ggml-base.en.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin` (148 MB). For Hinglish, this project uses [Oriserve Whisper-Hindi2Hinglish-Apex](https://huggingface.co/Oriserve/Whisper-Hindi2Hinglish-Apex) in a community ggml conversion at Q8_0: `curl -L -o ~/models/whisper/ggml-apex-hinglish-q8_0.bin https://huggingface.co/gouravg8/gtm-hi2hing-ggml/resolve/main/ggml-apex-hinglish-q8_0.bin` (874 MB; SHA-256 `bef6d9d5…0d4c`, full value in the [spec](docs/specs/whisper-asr.md)). Pass it with `--whisper-model` or `REELCTL_WHISPER_MODEL`

```bash
cargo build --release
bash fixtures/generate.sh all    # synthetic clips in fixtures/generated/

target/release/reelctl decompile \
  --input fixtures/generated/outlier_12s.mp4 \
  --out fixtures/generated/outlier_12s.edit_dna.json

target/release/reelctl diff \
  --draft fixtures/generated/draft_12s.mp4 \
  --outlier fixtures/generated/outlier_12s.edit_dna.json
```

Add `--whisper-model <model.bin>` to `decompile` for real words instead of voiced bursts. Add `--cut-detector transnet --transnet-weights <weights.onnx>` with `ORT_DYLIB_PATH` set to an ONNX Runtime library to find dissolves and fades with TransNetV2 (slow: about 7 ms per frame). HEVC video decodes on the Apple media engine on macOS; add `--decoder software` to force software decoding.

`decompile` prints a one-line summary, writes the JSON, and writes one PNG per cut to `fixtures/generated/outlier_12s.edit_dna_timeline/`. `diff` prints a JSON list of flags and a `comparison` of the draft with each outlier. See [docs/cli.md](docs/cli.md).

## Commands

| Command | Use |
|---|---|
| `cargo check` | Type-check the workspace after every edit |
| `cargo test` | Unit tests and the end-to-end tests; run once at the end of a task. Needs no whisper model |
| `REELCTL_WHISPER_MODEL=<model.bin> cargo test -- --include-ignored` | Also run the whisper model tests on the `speech12` and `tonespeech` clips (macOS only) |
| `cargo build --release` | Build `target/release/reelctl`; take timings with this build, not the debug one |
| `bash fixtures/generate.sh draft12\|draft12_10bit\|draft12_44k\|textured_cuts12\|textured_cuts12_hevc\|textured_cuts12_hevc8\|textured_cuts12_hevc_full\|resize12_hevc\|shrink12_hevc\|depth12_hevc\|outlier12\|noisy12\|quiet12\|lecture70\|bench60\|speech12\|tonespeech <out.mp4>` | Generate one synthetic clip. `draft12_10bit` is `draft12` as 10-bit HEVC tagged HLG, like an iPhone recording. `draft12_44k` is 60 s of 44.1 kHz AAC audio with a planted pause at 50 s. `textured_cuts12` is four textured 3 s shots with B-frames and no keyframe at any of its three cuts, and `textured_cuts12_hevc` and `textured_cuts12_hevc8` are the same shots as 10-bit and 8-bit HEVC, `textured_cuts12_hevc_full` as full-range 8-bit HEVC, and `resize12_hevc` is raw HEVC that grows from 320×180 to 640×360 `shrink12_hevc` is the reverse, and `depth12_hevc` is raw HEVC that changes from 8-bit to 10-bit (write both to `.hevc` files). `noisy12` is `draft12` with white noise under its planted pause, like room noise. `quiet12` is a tone at −46 dB peak, quieter than the old −40 dB silence floor, with a planted 0.5 s mute. `lecture70` is 70 s with one hard cut at 35 s, for the `understand` tests. `speech12` is 12 s of `say` speech with a planted 1 s pause, and `tonespeech` is a 25 s tone followed by speech; both need macOS |
| `bash fixtures/generate.sh all` | Generate `draft12`, `draft12_10bit`, `draft12_44k`, `textured_cuts12`, the HEVC variants (`textured_cuts12_hevc`, `_hevc8`, `_hevc_full`), `resize12_hevc`, `shrink12_hevc`, `depth12_hevc`, `outlier12`, `noisy12`, `quiet12`, `lecture70`, and `bench60` into `fixtures/generated/` |
| `cargo run -p edit-dna --example schema` | Regenerate `crates/edit-dna/schema/edit_dna.schema.json` after changing the types |
| `cargo run -p understand --example understanding_schema` | Regenerate `crates/understand/schema/understanding.schema.json` |
| `REELCTL_PROFILE=1 target/release/reelctl decompile ...` | Also print per-stage timings to stderr |

`cargo test` needs `bash` and `ffmpeg` on `PATH`, because the end-to-end test generates its clips when it runs. Tests use no network and no real reels.

## Architecture

The Cargo workspace has these crates (the Go watchtower is a separate module in `watchtower/`):

| Crate | Role |
|---|---|
| `edit-dna` | Serde types and JSON Schema for `edit_dna.json` |
| `decompile` | Decode with ffmpeg-next, then find cuts, punch-ins and overlay changes, silence, loudness, per-shot color, and the payoff cue |
| `diff` | Pacing flags, and a comparison, for a draft against one to three outliers |
| `filmstrip` | `timeline_view` PNGs at cut boundaries |
| `look` | `reelctl look`: per-channel histogram matching and the `.cube` writer and reader (pure) |
| `understand` | Long-video understanding: transcript segments, frame plan, the model request and cost bound, clip rules, and `understanding.json` (no ffmpeg or whisper dependency) |
| `timeline` | The OpenTimelineIO subset, edit operations and the ffmpeg filter-graph builder (pure) |
| `compositor` | `reelctl compositor isolate-presenter`: RobustVideoMatting presenter cut-out. A pure core (downsample ratio, recurrent state, alpha, compositing, frame pipeline) behind a `MattingModel` trait, the `ort` session, and the `ffprobe`/`ffmpeg` edges; no `unsafe` |
| `reelctl` | The command-line tool |

```
reel.mp4 ──► decompile ──► edit_dna.json
                 └───────► timeline_view PNGs, one per cut

draft.mp4 ──► decompile (in memory) ──┐
outlier edit_dna.json ────────────────┴──► diff ──► flags (JSON on stdout)

long.mp4 + edit_dna.json (whisper) ──► understand ──► understanding.json (clips, labelled cuts)
```

`decompile` makes one pass over the file, in-process, with no `ffmpeg` subprocess. Cut scores, color samples, boundary thumbnails, and the punch-in and overlay events are taken from each video frame as it is decoded. Silence, loudness, and word timing then run on the audio, mixed to mono at 48 kHz. `signal_ms` in the output is the measured time of this structural work; word timing is measured separately as `asr_ms`.

The reasons behind these choices are in [docs/decisions/](docs/decisions/).

## Docs

- [docs/status.md](docs/status.md): what works, measured numbers, gaps, next steps
- [docs/cli.md](docs/cli.md): `reelctl` flags, output, exit codes, input requirements
- [docs/edit-dna.md](docs/edit-dna.md): every field in `edit_dna.json`
- [docs/decisions/](docs/decisions/): architecture decision records (ADRs)
- [docs/specs/](docs/specs/): feature specs, written and approved before the code
- [CHANGELOG.md](CHANGELOG.md): notable changes
- [planer.md](planer.md): product spec and the P0–P3 roadmap

## Contributing

Before changing code, read [AGENTS.md](AGENTS.md) (project rules), [CONSTRAINTS.md](CONSTRAINTS.md) (quality bar), and [SKILLS.md](SKILLS.md) (required agent workflows). Update the docs in the same commit as the change; the Docs section of AGENTS.md lists which file covers what. Commit messages use `<type>: <description>`, where type is `feat`, `fix`, `refactor`, `test`, `docs`, or `chore`.
