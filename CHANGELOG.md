# Changelog

Notable changes to `reelctl`. Add an entry under Unreleased in the same commit as the change. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `reelctl decompile` writes `edit_dna.json` (cuts, silence gaps, loudness, per-shot color, the 9-second payoff cue, and the measured `signal_ms`) and one `timeline_view` PNG per cut.
- `reelctl diff` flags dead air over 180 ms, more than 2.5 s without a cut, and a missing payoff cue compared with an outlier's `edit_dna.json`.
- A JSON Schema for `edit_dna.json`.
- Synthetic test clips (`fixtures/generate.sh`), including `speech12` and `tonespeech` with synthesized speech on macOS, and end-to-end tests.
- whisper runs in a child process. A whisper.cpp abort, which 1.8.3 can hit on short windows with DTW on, no longer ends `reelctl`: the pass is retried without DTW and `asr_note` says so. `REELCTL_PROFILE=1` also prints `asr_first_ms` and `asr_second_ms`.
- With a whisper model, stretches of sound that whisper.cpp left without words are transcribed a second time, and `asr_note` lists them. On a 60 s iPhone clip this brought the word count from 125 to 183.
- `reelctl decompile` reads 10-bit 4:2:0 video (`yuv420p10le`), so iPhone HEVC Main 10 recordings no longer need converting first. `decode_note` says when the video was 10-bit and when it was HDR (HLG or PQ), whose colors are not tone-mapped. 8-bit input gives the same output as before.
- Project docs: README, status, CLI and `edit_dna.json` references, and decision records.
- `--whisper-model` now transcribes with whisper.cpp: `words` are real words timed with DTW, `words_per_minute` is a real speaking rate, and `asr_backend` is `"whisper_cpp"`. English-only and multilingual models work, including the Hinglish Apex fine-tune.

### Changed

- Audio that is not 48 kHz is now resampled as one stream instead of one decoded frame at a time. Per-frame resampling dropped a fractional sample at each frame, so at 44.1 kHz silence gaps and word times came out early: a pause at 50 s by 16–33 ms, and about 0.5 s by 18 minutes. 48 kHz audio is untouched. The interpolation is linear with no low-pass filter, so audio above 48 kHz (88.2 or 96 kHz) can alias.
- `edit_dna.json` is now schema version 2, with `silence_threshold_db` and `noise_floor_db`. Version 1 files still load in `reelctl diff`.
- `silence_gaps` now uses a threshold set from each recording's own levels instead of a fixed −40 dBFS, and a sound shorter than 30 ms no longer splits a gap. Pauses under room noise become gaps, so `diff` now flags dead air in real recordings; on a 60 s iPhone recording, gaps of 300 ms or more went from 0 to 24. Clean recordings get the same gaps as before. Without a whisper model, burst words use the same threshold, so `words_per_minute` changes on noisy recordings.
- An unsupported pixel format's error now names the format, for example `unsupported pixel format yuv422p`.
- `reelctl --help` no longer says `--whisper-model` is ignored.
- Without `--whisper-model`, `asr_note` now says no model was given, instead of saying whisper.cpp is not linked.
- A `--whisper-model` path that is missing or not a usable whisper model now exits with code 1. It used to be ignored.
- Building now needs CMake, because the `whisper-rs` dependency compiles whisper.cpp 1.8.3 from source.
