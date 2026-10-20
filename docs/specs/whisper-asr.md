# Spec: Real word timing with whisper.cpp

Status: draft, awaiting approval (2026-10-20). Branch: `feature/whisper-asr`.

## Objective

Today `words` in `edit_dna.json` are voiced bursts named `burst-N`, and `words_per_minute` counts bursts ([ADR-003](../decisions/ADR-003-silence-burst-word-estimator.md)). planer.md's Pass 1 calls for whisper.cpp word timing and real words per minute. This change makes `reelctl` run whisper.cpp whenever a model file is supplied, so creators and agents get real words with start and end times, a real speaking rate, and word ticks in `timeline_view` PNGs that mark real words.

Behavior:

- **With a model** (`--whisper-model <path>` or `REELCTL_WHISPER_MODEL`): `words` come from whisper.cpp, `asr_backend` is `"whisper_cpp"`, and `asr_note` names the model file and the whisper.cpp version.
- **Without a model**: the burst estimator runs as it does today. Only `asr_note` changes, because "whisper.cpp was not linked" stops being true.
- **Model path given but missing or unloadable**: `decompile` and `diff` exit with code 1 and an error naming the path. An explicit request never silently falls back ([ADR-004](../decisions/ADR-004-report-missing-passes-in-the-output.md)).
- **Language**: English for English-only (`.en`) models; auto-detected for multilingual models.
- `asr_ms` covers model loading, resampling, and transcription. It stays outside `signal_ms` and the 500 ms target.
- whisper.cpp prints nothing to stdout or stderr.
- The `edit_dna.json` schema does not change: `AsrBackend::WhisperCpp` and the `Word` type already exist, so `SCHEMA_VERSION` stays 1.

Out of scope: voice activity detection (silence gaps are already measured), GPU acceleration, DTW timestamps, translation, real-reel accuracy studies.

## Tech Stack

- `whisper-rs` 0.16.0 (Unlicense), the current release. It bundles whisper.cpp 1.8.3 through `whisper-rs-sys` 0.15.0 and builds it with CMake. Default features only: CPU with Apple's Accelerate on macOS; Metal is off unless the `metal` feature is enabled (`whisper-rs-sys` `build.rs`).
- CMake 4.4.4 (`/opt/homebrew/bin/cmake`), installed 2026-10-20.
- Model for development: `ggml-base.en.bin`, 147,964,211 bytes, SHA-256 `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002`, stored outside the repo at `~/models/whisper/`.

Sources, checked against the downloaded 0.16.0 crate:

- whisper-rs API (`WhisperContext::new_with_params`, `create_state`, `WhisperState::full`, `as_iter`, segment timestamps in centiseconds): https://docs.rs/whisper-rs/0.16.0/whisper_rs/
- Word-level timestamps with `max_len = 1`: https://github.com/ggml-org/whisper.cpp/blob/v1.8.3/README.md#word-level-timestamp-experimental. In the 1.8.3 source, segments are only re-split by `max_len` when `token_timestamps` is on, so both must be set.
- `install_logging_hooks()` with neither log-backend feature discards whisper.cpp's logs (whisper-rs `lib.rs` doc comment).
- `WhisperState::full` requires 16 kHz mono `f32` PCM (its doc comment).

Approach:

1. Resample the decoded 48 kHz mono PCM to 16 kHz inside the ASR stage: a windowed-sinc low-pass FIR, then keep every third sample.
2. Load the model and call `full` with greedy sampling, `token_timestamps = true`, `max_len = 1`, and `split_on_word = true`. This gives one segment per word. Multiply segment start and end (centiseconds) by 10 for milliseconds.
3. Trim each word's text, and drop empty segments and bracketed non-speech markers such as `[BLANK_AUDIO]`.
4. `words_per_minute` keeps its formula: word count divided by duration in minutes.

## Commands

```bash
cargo check                    # after every edit
cargo test                     # default suite; needs no model
REELCTL_WHISPER_MODEL=~/models/whisper/ggml-base.en.bin cargo test -- --include-ignored
cargo build --release          # needs cmake; the first build also compiles whisper.cpp
target/release/reelctl decompile --input clip.mp4 --out clip.edit_dna.json \
  --whisper-model ~/models/whisper/ggml-base.en.bin
```

## Project Structure

| Path | Change |
|---|---|
| `crates/decompile/Cargo.toml` | Add `whisper-rs = "0.16.0"` |
| `crates/decompile/src/resample.rs` | New: 48 kHz → 16 kHz |
| `crates/decompile/src/whisper.rs` | New: load model, set parameters, map segments to `Word`s |
| `crates/decompile/src/asr.rs` | Choose whisper or the burst estimator; return a `Result` |
| `crates/decompile/src/pipeline.rs` | Propagate ASR errors |
| `fixtures/generate.sh` | New `speech12` clip: macOS `say` speech over a solid-color video |
| `crates/reelctl/tests/whisper_fixture.rs` | New: end-to-end test with the model, ignored by default |
| Docs | README, `docs/cli.md`, `docs/edit-dna.md`, `docs/status.md`, ADR-007 (supersedes ADR-003), CHANGELOG, AGENTS.md |

## Code Style

Match the existing crates: small free functions, errors as `DecompileError` variants, notes that say what ran and why, no `unwrap` outside tests, comments only for the why. For example:

```rust
pub fn transcribe(
    pcm: &[f32],
    sample_rate: u32,
    duration_ms: u64,
    model: Option<&Path>,
) -> Result<AsrEstimate, DecompileError> {
    match model {
        Some(path) => whisper::transcribe(path, pcm, sample_rate, duration_ms),
        None => Ok(silence_bursts(pcm, sample_rate, duration_ms)),
    }
}
```

## Testing Strategy

Test-first, with the repo's `cargo test`.

- **Unit tests, no model:**
  - The resampler keeps a 1 kHz tone, strongly attenuates a 12 kHz tone, and returns one third of the samples.
  - Segment-to-word mapping converts centiseconds to milliseconds, trims text, and drops `[BLANK_AUDIO]` and empty text.
  - No model gives `silence_bursts` with the new note.
  - A missing model file gives an error that names the path.
- **Default integration tests:** `p0_fixture.rs` keeps passing; its `asr_note` assertion moves to the new note. A new case checks that `--whisper-model /missing.bin` exits with code 1.
- **Model integration test, `#[ignore]` by default:**
  - On the `speech12` clip, `asr_backend` is `"whisper_cpp"`.
  - `cut`, `dead`, `air`, `proof`, `viewer`, and `scrolls` appear in that order.
  - Word times increase and stay inside the clip.
  - No word starts or ends more than 200 ms inside the planted 1-second pause.
  - Run with `REELCTL_WHISPER_MODEL` set and `--include-ignored`. The ignore attribute and the commit message both give the reason.
- `say` is macOS-only, so the speech clip and the model test run only on macOS.

## Boundaries

- **Always:**
  - Keep the burst estimator as the no-model path.
  - Fail closed on a bad model path.
  - Keep ASR out of `signal_ms`.
  - Update the docs in the same commit.
  - Run `cargo check` after edits and `cargo test` at the end.
- **Ask first:**
  - Enabling Metal or CoreML.
  - DTW timestamps.
  - Voice activity detection or its model.
  - Any change to the `edit_dna.json` schema.
  - Picking a model path automatically.
- **Never:**
  - Commit model files, audio, or real reels.
  - Download anything from build scripts or tests.
  - Use the network in tests.

## Success Criteria

1. With `--whisper-model ~/models/whisper/ggml-base.en.bin`, the `speech12` clip gives `asr_backend: "whisper_cpp"` and the six key words in order.
2. No word starts or ends more than 200 ms inside the planted 1-second pause.
3. Without a model, `edit_dna.json` matches today's output except for `asr_note`, timings, and `signal_ms`/`asr_ms`, and `cargo test` passes on a machine with no model.
4. A missing model path exits with code 1 and an error naming the path.
5. The 60 s benchmark's `signal_ms` stays in its current range, because ASR is not part of it. `asr_ms` for the 12 s speech clip and for a 60 s speech clip is recorded in `docs/status.md`.
6. A normal run prints no whisper.cpp log lines.
7. The docs listed under Project Structure are updated.

## Risks

- `whisper-rs-sys` has not been built with CMake 4.x on this Mac. The first task builds it. If CMake rejects an old policy, `whisper-rs-sys` passes `CMAKE_*` environment variables through to CMake, so `CMAKE_POLICY_VERSION_MINIMUM=3.5` is a fallback. Its `BUILDING.md` also mentions extra Apple Silicon link flags that may or may not still be needed.
- whisper.cpp marks token timestamps as experimental, so word times are approximate. If they are too loose, DTW timestamps are the next option (ask first).
- whisper can invent words on music or tones. The fixtures with a sine tone will not be run through whisper.
- Builds get slower, and every build now needs CMake.

## Open Questions

1. **Model test:** keep the model-dependent test ignored by default (recommended), so plain `cargo test` passes without a model? The alternative is that every test run needs the model file.
2. **Bad model path:** fail with an error (recommended, matching ADR-004), or fall back to the estimator with a note?
3. **Automatic model path:** should `reelctl` use `~/models/whisper/ggml-base.en.bin` when no flag or variable is set? Recommended: no. Put `export REELCTL_WHISPER_MODEL=~/models/whisper/ggml-base.en.bin` in your shell profile instead, so the choice stays explicit.
