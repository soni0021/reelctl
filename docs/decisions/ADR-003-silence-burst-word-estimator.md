# ADR-003: Estimate word timing from silence while whisper.cpp is unlinked

## Status
Accepted

## Date
2026-10-18. Recorded on 2026-10-20 from [asr.rs](../../crates/decompile/src/asr.rs) and [AGENTS.md](../../AGENTS.md).

## Context
planer.md specifies whisper.cpp with voice activity detection for word timing and words per minute. `whisper-rs` 0.16 builds whisper.cpp with CMake, `cmake` was not on PATH on the development machine, and P0 does not download ggml model weights.

## Decision
- `asr::transcribe` reports voiced bursts as words. A burst is audio above −40 dBFS, measured in 10 ms windows, that ends at the first 40 ms of silence; bursts shorter than 40 ms are dropped. Each is named `burst-N`.
- `words_per_minute` is the number of bursts divided by the duration in minutes.
- Every output says so: `asr_backend` is `"silence_bursts"`, and `asr_note` explains why and mentions any `--whisper-model` path that was ignored.
- Word timing is timed on its own as `asr_ms` and kept out of `signal_ms`, because the 500 ms target covers structural extraction only (AGENTS.md).

## Alternatives Considered

### Link whisper.cpp through whisper-rs
- Blocked: no `cmake` to build it and no model file.

## Consequences
- `words` and `words_per_minute` are not speech measurements. Tones, music, and noise above −40 dBFS count as words, and the word ticks in `timeline_view` PNGs mark bursts.
- Replacing the estimator means changing `asr::transcribe` and setting `asr_backend` to `"whisper_cpp"`. Consumers can tell which one produced a file.
- Real ASR will take far longer than the estimator (about 2 ms on the 60 s benchmark). It stays outside the 500 ms target either way.
