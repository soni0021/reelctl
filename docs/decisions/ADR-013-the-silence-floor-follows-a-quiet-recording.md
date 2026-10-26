# ADR-013: The silence floor follows the loud part of a quiet recording

## Status
Accepted. Supersedes the threshold formula in [ADR-008](ADR-008-silence-relative-to-the-noise-floor.md) for recordings whose sound is quieter than −28 dB.

## Date
2026-10-26

## Context
ADR-008 set the threshold to `max(−40, min(p10 + 6, p90 − 12))` dB. The −40 floor assumed sound is louder than −40 dB. On the first reel downloaded for the comparison workflow (`Reels/`, a 15 s Instagram download with almost no audio), all the sound was quieter: p10 −55.05, p50 −49.4, p90 −45.5, p99 −41.9, maximum −34.3 dB. The threshold came out at −40, only 36 of 1,509 windows reached it, and `silence_gaps` was one gap over the whole 15,092 ms. `diff` then told the creator to trim 15 s of dead air that was not there. The same would happen to a quiet speaker or a low-gain microphone. The requirements are in the [spec](../specs/quiet-audio.md).

## Decision
- The floor is no longer a constant. It is `max(min(−40, p99 − 12), −70)`, and the threshold is `max(floor, min(p10 + 6, p90 − 12))`.
- **p99, not p90.** A recording that is mostly digital silence or room noise has a p90 equal to the silence. Capping the whole threshold at `p90 − 12` would put it below the silence, and real silence would stop being a gap (95 windows at −120 plus 5 at −14 must stay at −40).
- **A lower bound of −70.** A recording with under 1% sound, such as a silent clip with one click, has a p99 equal to its silence (−120). `p99 − 12` would be −132, below the silence. −70 is clear of 16-bit dither (about −96) and of every quiet recording measured (the Reels file −53.9, a −50 dB tone −62).
- Recordings whose loudest 1% of windows are at −28 dB or louder keep today's threshold exactly. No new flag and no `edit_dna.json` field; `silence_threshold_db` already records what was used, and `integrated_lufs` shows a quiet recording.

## Alternatives Considered

### Report "audio too quiet to judge" and skip the silence flags
- A schema change (version 3), and the creator still gets no pause detection on a quiet recording. Recommended against in the spec and not chosen. Revisit if `comparative-diff` needs it.

### Apply the `p90 − 12` cap after the −40 floor
- Fixes the Reels file, breaks recordings that are mostly silence (threshold −132 for 95 windows at −120 plus 5 at −14). Found in review of the first spec draft.

### Lower the fixed floor, for example to −55
- Works for the Reels file, but is another absolute number tuned on one recording, and it would call faint noise sound on clean recordings that the −40 floor treats as silence.

## Consequences
- The Reels file: threshold −53.92 dB and three gaps (0–990, 1,370–1,640, 1,760–1,850 ms) instead of one over the whole clip. `diff` still flags the first two as dead air, since they exceed 180 ms; they are the quiet opening of the file.
- 16 of the 17 clips measured, including the real iPhone recording and every synthetic fixture, give the same output apart from timings. `quiet12` (a tone at −46 dB peak with a 0.5 s mute) gives its pause only.
- The 12 dB distance is the same headroom as ADR-008, tuned on one recording. A second quiet recording would be the time to retune it.
- Without a whisper model the Reels file now has 5 burst "words" instead of 1, because bursts use the same threshold.
