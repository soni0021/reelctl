# Changelog

Notable changes to `reelctl`. Add an entry under Unreleased in the same commit as the change. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `reelctl decompile` writes `edit_dna.json` (cuts, silence gaps, loudness, per-shot color, the 9-second payoff cue, and the measured `signal_ms`) and one `timeline_view` PNG per cut.
- `reelctl diff` flags dead air over 180 ms, more than 2.5 s without a cut, and a missing payoff cue compared with an outlier's `edit_dna.json`.
- A JSON Schema for `edit_dna.json`.
- Synthetic test clips (`fixtures/generate.sh`) and an end-to-end test.
- Project docs: README, status, CLI and `edit_dna.json` references, and decision records.
- `--whisper-model` now transcribes with whisper.cpp: `words` are real words timed with DTW, `words_per_minute` is a real speaking rate, and `asr_backend` is `"whisper_cpp"`. English-only and multilingual models work, including the Hinglish Apex fine-tune.

### Changed

- Without `--whisper-model`, `asr_note` now says no model was given, instead of saying whisper.cpp is not linked.
- A `--whisper-model` path that is missing or not a usable whisper model now exits with code 1. It used to be ignored.
- Building now needs CMake, because the `whisper-rs` dependency compiles whisper.cpp 1.8.3 from source.
