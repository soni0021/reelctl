# Spec: Real word timing with whisper.cpp

Status: implemented 2026-10-21 on `feature/whisper-asr`; all seven success criteria verified (see [docs/status.md](../status.md#word-timing-2026-10-21)), awaiting review and merge. Approved 2026-10-20, with the recommended answer to each open question (see Resolved Questions). Revised and approved 2026-10-21: DTW word timing, multilingual models, and the DTW preset read from the model header (see [Revision 2026-10-21](#revision-2026-10-21)). Branch: `feature/whisper-asr`. Plan: [tasks/plan.md](../../tasks/plan.md).

## Objective

Today `words` in `edit_dna.json` are voiced bursts named `burst-N`, and `words_per_minute` counts bursts ([ADR-003](../decisions/ADR-003-silence-burst-word-estimator.md)). planer.md's Pass 1 calls for whisper.cpp word timing and real words per minute. This change makes `reelctl` run whisper.cpp whenever a model file is supplied, so creators and agents get real words with start and end times, a real speaking rate, and word ticks in `timeline_view` PNGs that mark real words.

Behavior:

- **With a model** (`--whisper-model <path>` or `REELCTL_WHISPER_MODEL`): `words` come from whisper.cpp, `asr_backend` is `"whisper_cpp"`, and `asr_note` names the model file and the whisper.cpp version.
- **Without a model**: the burst estimator runs as it does today. Only `asr_note` changes, because "whisper.cpp was not linked" stops being true.
- **Model path given but missing or unloadable**: `decompile` and `diff` exit with code 1 and an error naming the path. An explicit request never silently falls back ([ADR-004](../decisions/ADR-004-report-missing-passes-in-the-output.md)).
- **Language**: English for English-only (`.en`) models; auto-detected for multilingual models, such as the Hinglish model Oriserve Whisper-Hindi2Hinglish-Apex. `asr_note` names the detected language.
- **Word timing**: each word starts at its DTW timestamp and ends where the next word starts, or where a measured silence gap starts if that comes first. `asr_note` names the DTW preset.
- **Model shape**: the DTW preset comes from the hyperparameters in the model file's header, not its name, so renamed and fine-tuned models work. A header that matches no preset exits with code 1 and an error naming the path.
- `asr_ms` covers model loading, resampling, and transcription. It stays outside `signal_ms` and the 500 ms target.
- whisper.cpp prints nothing to stdout or stderr.
- The `edit_dna.json` schema does not change: `AsrBackend::WhisperCpp` and the `Word` type already exist, so `SCHEMA_VERSION` stays 1.

Out of scope: voice activity detection (silence gaps are already measured), GPU acceleration, translation, real-reel accuracy studies.

## Tech Stack

- `whisper-rs` 0.16.0 (Unlicense), the current release. It bundles whisper.cpp 1.8.3 through `whisper-rs-sys` 0.15.0 and builds it with CMake. Default features only: CPU with Apple's Accelerate on macOS; Metal is off unless the `metal` feature is enabled (`whisper-rs-sys` `build.rs`).
- CMake 4.4.4 (`/opt/homebrew/bin/cmake`), installed 2026-10-20.
- Models for development, stored outside the repo at `~/models/whisper/`:
  - `ggml-apex-hinglish-q8_0.bin`: Oriserve Whisper-Hindi2Hinglish-Apex, a large-v3-turbo fine-tune that writes Hinglish in Roman script, quantized to Q8_0. 874,188,075 bytes, SHA-256 `bef6d9d571e9e7bd3cd2ebba8b2d5c9cc4731e210d6750ae6a9c621a87e60d4c`. The default for this project's speech, which is Hinglish.
  - `ggml-base.en.bin`: 147,964,211 bytes, SHA-256 `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002`. English only.

Sources, checked against the downloaded 0.16.0 crate:

- whisper-rs API (`WhisperContext::new_with_params`, `create_state`, `WhisperState::full`, `as_iter`, segment timestamps in centiseconds): https://docs.rs/whisper-rs/0.16.0/whisper_rs/
- Word-level timestamps with `max_len = 1`: https://github.com/ggml-org/whisper.cpp/blob/v1.8.3/README.md#word-level-timestamp-experimental. In the 1.8.3 source, segments are only re-split by `max_len` when `token_timestamps` is on, so both must be set.
- `install_logging_hooks()` with neither log-backend feature discards whisper.cpp's logs (whisper-rs `lib.rs` doc comment).
- `WhisperState::full` requires 16 kHz mono `f32` PCM (its doc comment).
- DTW token timestamps: `WhisperContextParameters::dtw_parameters` with `DtwMode::ModelPreset`, and each token's `t_dtw` in centiseconds (whisper-rs `whisper_ctx.rs`, `whisper_state/token.rs`). whisper.cpp picks the alignment heads from the preset (`g_aheads` in `whisper.cpp`), so the preset must match the model's shape.
- The model header: magic `0x67676d6c`, then eleven `i32` values, `n_vocab`, `n_audio_ctx`, `n_audio_state`, `n_audio_head`, `n_audio_layer`, `n_text_ctx`, `n_text_state`, `n_text_head`, `n_text_layer`, `n_mels`, `ftype` (`whisper_model_load` in whisper.cpp 1.8.3).

Approach:

1. Resample the decoded 48 kHz mono PCM to 16 kHz inside the ASR stage: a windowed-sinc low-pass FIR, then keep every third sample.
2. Read the model header and choose the DTW preset from `n_audio_state`, `n_audio_layer`, `n_text_layer`, and `n_vocab` (51864 means English-only; 51866 means large-v3). Large models with vocabulary 51865 are v1 or v2, which share a shape; they get the v2 preset.
3. Load the model with that DTW preset and call `full` with greedy sampling, `token_timestamps = true`, `max_len = 1`, and `split_on_word = true`. This gives one segment per word.
4. Trim each word's text, and drop empty segments and bracketed non-speech markers such as `[BLANK_AUDIO]`.
5. A word starts at the `t_dtw` of its first non-special token, or at the segment start if DTW gave none. It ends at the next word's start, or at the start of the first silence gap after its own start, whichever is earlier; the last word ends at its segment end, cut the same way. Times are centiseconds × 10.
4. `words_per_minute` keeps its formula: word count divided by duration in minutes.

## Commands

```bash
cargo check                    # after every edit
cargo test                     # default suite; needs no model
REELCTL_WHISPER_MODEL=~/models/whisper/ggml-apex-hinglish-q8_0.bin cargo test -- --include-ignored
cargo build --release          # needs cmake; the first build also compiles whisper.cpp
target/release/reelctl decompile --input clip.mp4 --out clip.edit_dna.json \
  --whisper-model ~/models/whisper/ggml-apex-hinglish-q8_0.bin
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
  - Word starts use the DTW time; ends stop at the next word or the next silence gap.
  - The headers of base.en and the Apex model map to `BaseEn` and `LargeV3Turbo`; an unknown shape or a file without the ggml magic is an error naming the path.
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
  - Voice activity detection or its model.
  - Any change to the `edit_dna.json` schema.
  - Picking a model path automatically.
- **Never:**
  - Commit model files, audio, or real reels.
  - Download anything from build scripts or tests.
  - Use the network in tests.

## Success Criteria

1. With `--whisper-model` set to either development model, the `speech12` clip gives `asr_backend: "whisper_cpp"` and the six key words in order.
2. No word starts or ends more than 200 ms inside the planted 1-second pause.
3. Without a model, `edit_dna.json` matches today's output except for `asr_note`, timings, and `signal_ms`/`asr_ms`, and `cargo test` passes on a machine with no model.
4. A missing model path exits with code 1 and an error naming the path.
5. The 60 s benchmark's `signal_ms` stays in its current range, because ASR is not part of it. `asr_ms` for the 12 s speech clip and for a 60 s speech clip is recorded in `docs/status.md`.
6. A normal run prints no whisper.cpp log lines.
7. The docs listed under Project Structure are updated.

## Risks

- `whisper-rs-sys` has not been built with CMake 4.x on this Mac. The first task builds it. If CMake rejects an old policy, `whisper-rs-sys` passes `CMAKE_*` environment variables through to CMake, so `CMAKE_POLICY_VERSION_MINIMUM=3.5` is a fallback. Its `BUILDING.md` also mentions extra Apple Silicon link flags that may or may not still be needed.
- whisper.cpp marks token and DTW timestamps as experimental, so word times are approximate. Fine-tuned models such as Apex reuse their base model's alignment heads; on 60 s of real Hinglish that gave no word inside a pause (see Revision).
- whisper can invent words on music or tones. The fixtures with a sine tone will not be run through whisper.
- Builds get slower, and every build now needs CMake.

## Resolved Questions

Answered 2026-10-20:

1. **Model test:** the model-dependent test is `#[ignore]`d by default, so plain `cargo test` passes without a model. Run it with `--include-ignored` and `REELCTL_WHISPER_MODEL` set.
2. **Bad model path:** fail with an error, matching ADR-004. No fallback to the estimator.
3. **Automatic model path:** no. `reelctl` uses a model only when `--whisper-model` or `REELCTL_WHISPER_MODEL` names one. For convenience, put `export REELCTL_WHISPER_MODEL=~/models/whisper/ggml-base.en.bin` in your shell profile.

## Revision 2026-10-21

Found while building Task 3 and running real footage (numbers in [docs/status.md](../status.md#first-real-footage-2026-10-20)):

- **Plain token timestamps smear words across pauses.** On a `say` clip with a 1.2 s pause, "dead" ran from 0.91 s to 2.00 s, across the pause. On 60 s of real Hinglish speech, 21 of 183 words ran more than 200 ms into a pause. With DTW, no word did in either case, and the run took the same 14.5 s with the Apex model.
- **The speech this project targets is Hinglish.** `base.en` invented one English sentence and repeated it. The user chose Oriserve Whisper-Hindi2Hinglish-Apex, which transcribed it in Roman script.
- **Model files are renamed.** The Apex file name matches no whisper.cpp model name, so the DTW preset comes from the header, which records the model's shape.

Approved by the user on 2026-10-21: DTW timing with silence-bounded word ends, auto-detected language for multilingual models, and a preset detected from the model, failing when it is not recognized. The proposal detected it from the file name; reading the header instead was chosen while building, because it also recognizes the Apex file.
