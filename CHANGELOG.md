# Changelog

Notable changes to `reelctl`. Add an entry under Unreleased in the same commit as the change. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `reelctl decompile` writes `edit_dna.json` (cuts, silence gaps, loudness, per-shot color, the 9-second payoff cue, and the measured `signal_ms`) and one `timeline_view` PNG per cut.
- `reelctl diff` flags dead air over 180 ms, more than 2.5 s without a cut, and a missing payoff cue compared with an outlier's `edit_dna.json`.
- A JSON Schema for `edit_dna.json`.
- Synthetic test clips (`fixtures/generate.sh`) and an end-to-end test.
- Project docs: README, status, CLI and `edit_dna.json` references, and decision records.
