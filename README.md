# reelctl

`reelctl` measures how a short vertical video is edited and compares a creator's draft against a top-performing reel (an "outlier") in the same niche.

- `reelctl decompile` turns an MP4 into `edit_dna.json`: cut times, silence gaps, loudness, per-shot color, whether there is a cut near the 9-second payoff mark, and, given a whisper model, timed words. It also writes a `timeline_view` PNG at each cut, so an agent can inspect cuts without opening the video.
- `reelctl diff` decompiles a draft and flags dead air, long stretches with no cut, and a missing payoff cue.

This repository is P0 of the plan in [planer.md](planer.md). P1–P3 (trend watchtower, timeline editor, specialist video model) are specified there and not built. Current state, measurements, and known gaps are in [docs/status.md](docs/status.md).

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

Add `--whisper-model <model.bin>` to `decompile` for real words instead of voiced bursts.

`decompile` prints a one-line summary, writes the JSON, and writes one PNG per cut to `fixtures/generated/outlier_12s.edit_dna_timeline/`. `diff` prints a JSON list of flags. See [docs/cli.md](docs/cli.md).

## Commands

| Command | Use |
|---|---|
| `cargo check` | Type-check the workspace after every edit |
| `cargo test` | Unit tests and the end-to-end tests; run once at the end of a task. Needs no whisper model |
| `REELCTL_WHISPER_MODEL=<model.bin> cargo test -- --include-ignored` | Also run the whisper model tests on the `speech12` and `tonespeech` clips (macOS only) |
| `cargo build --release` | Build `target/release/reelctl`; take timings with this build, not the debug one |
| `bash fixtures/generate.sh draft12\|draft12_10bit\|outlier12\|noisy12\|bench60\|speech12\|tonespeech <out.mp4>` | Generate one synthetic clip. `draft12_10bit` is `draft12` as 10-bit HEVC tagged HLG, like an iPhone recording. `noisy12` is `draft12` with white noise under its planted pause, like room noise. `speech12` is 12 s of `say` speech with a planted 1 s pause, and `tonespeech` is a 25 s tone followed by speech; both need macOS |
| `bash fixtures/generate.sh all` | Generate `draft12`, `draft12_10bit`, `outlier12`, `noisy12`, and `bench60` into `fixtures/generated/` |
| `cargo run -p edit-dna --example schema` | Regenerate `crates/edit-dna/schema/edit_dna.schema.json` after changing the types |
| `REELCTL_PROFILE=1 target/release/reelctl decompile ...` | Also print per-stage timings to stderr |

`cargo test` needs `bash` and `ffmpeg` on `PATH`, because the end-to-end test generates its clips when it runs. Tests use no network and no real reels.

## Architecture

The Cargo workspace has five crates:

| Crate | Role |
|---|---|
| `edit-dna` | Serde types and JSON Schema for `edit_dna.json` |
| `decompile` | Decode with ffmpeg-next, then find cuts, silence, loudness, per-shot color, and the payoff cue |
| `diff` | Pacing flags for a draft against an outlier |
| `filmstrip` | `timeline_view` PNGs at cut boundaries |
| `reelctl` | The command-line tool |

```
reel.mp4 ──► decompile ──► edit_dna.json
                 └───────► timeline_view PNGs, one per cut

draft.mp4 ──► decompile (in memory) ──┐
outlier edit_dna.json ────────────────┴──► diff ──► flags (JSON on stdout)
```

`decompile` makes one pass over the file, in-process, with no `ffmpeg` subprocess. Cut scores, color samples, and boundary thumbnails are taken from each video frame as it is decoded. Silence, loudness, and word timing then run on the audio, mixed to mono at 48 kHz. `signal_ms` in the output is the measured time of this structural work; word timing is measured separately as `asr_ms`.

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
