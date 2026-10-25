# ADR-008: Set the silence threshold from each recording's noise floor

## Status
Accepted

## Date
2026-10-25

## Context
`silence_gaps` used a fixed −40 dBFS threshold. On the first 60 s of a real iPhone recording, room noise sat a few dB above it, so there was 1 gap of 180 ms or more and none of 300 ms or more. `diff` therefore reported almost no dead air, and whisper word ends, which stop at gaps ([ADR-007](ADR-007-whisper-cpp-word-timing.md)), ran into pauses. The measurements behind this decision are in the [spec](../specs/relative-silence.md#evidence).

## Decision
- The threshold is `max(−40, min(p10 + 6, p90 − 12))` dB, where p10 and p90 are nearest-rank percentiles of the recording's 10 ms window levels.
  - p10 + 6 dB sits above the noise floor.
  - The p90 − 12 dB cap stops a recording with no quiet stretch, such as a steady tone, from being called silent.
  - The −40 floor keeps clean recordings unchanged.
- A gap ends only after 30 ms of sound in a row, so short noise spikes do not split a pause. The 80 ms minimum gap is unchanged.
- The no-model burst estimator uses the same threshold and the same window levels.
- The calculation is part of the silence stage, so it counts toward `signal_ms`.

## Alternatives Considered

### Keep −40 dBFS
- Misses pauses in every recording with audible room noise, which is most real footage.

### A fixed, higher threshold such as −35 dBFS
- Works for one room, but the level depends on the microphone, the room, and even the downmix (FFmpeg's `-ac 1` reads about 3 dB louder than `reelctl`'s channel average). It would call quiet speech silence in quieter recordings.

### A voice activity detection model
- More robust to music and background speech, but adds a model and a dependency to the structural path. Not needed for the measured problem.

### Ending a gap on any louder window (no minimum sound)
- Pink noise at room level split a 1.2 s pause into two gaps, which `diff` would report as two dead-air flags.

## Consequences
- Pauses under room noise are gaps. On the real recording: 31 gaps of 180 ms or more and 24 of 300 ms or more, with no Apex word start inside a gap.
- Every existing synthetic clip has identical gaps and burst words.
- The rule's constants were tuned on one real recording. Music beds, loud backgrounds, and very quiet speakers are unmeasured; thresholds from 2.5 dB higher (−33 dB on that recording) started calling speech silence.
- Without a whisper model, `words_per_minute` changes on noisy recordings, because bursts now split at the new threshold.
