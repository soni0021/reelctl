# ADR-005: `signal_ms` measures the structural path only

## Status
Accepted

## Date
2026-10-18. Recorded on 2026-10-20 from [pipeline.rs](../../crates/decompile/src/pipeline.rs) and [AGENTS.md](../../AGENTS.md).

## Context
The P0 target is under 500 ms per 60-second reel. AGENTS.md defines `signal_ms` as a measured structural duration (decode, cuts, silence, loudness, histograms, JSON write) that must not be invented or padded, and says `asr_ms` (word timing) is not part of the 500 ms gate.

## Decision
- `signal_ms` is the sum of measured stages: decode (including cut scoring, color sampling, and thumbnail capture), analysis (cut list, per-shot histograms, payoff cue), silence, loudness, and the JSON write.
- So that the recorded number includes its own write, the file is written, the write time is added, and the file is rewritten if the rounded total changed, up to four times.
- `asr_ms` is timed separately.
- `structural_under_500ms` on stdout compares the unrounded total with 500 ms.
- `REELCTL_PROFILE` prints the time of each stage to stderr.

## Alternatives Considered
None recorded.

## Consequences
- The number leaves out process start, word timing, PNG writing, and the in-memory copies between stages, so it is lower than wall-clock time. On the 60 s benchmark, `signal_ms` was 284–316 ms while the whole command took 0.30–0.80 s.
- A new stage on the structural path has to be timed and added to the sum; leaving it out would understate `signal_ms`.
