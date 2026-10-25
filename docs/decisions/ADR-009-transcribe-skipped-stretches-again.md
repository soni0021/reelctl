# ADR-009: Transcribe stretches whisper skipped a second time

## Status
Accepted

## Date
2026-10-25

## Context
whisper.cpp 1.8.3 decodes 30 s windows in turn and can skip speech without reporting it: it drops a window it judges to hold no speech, and skips the rest of a window whose output ends on a single timestamp. With the Apex model this lost the first 30.4 s of an 18-minute video and 9.9–30.9 s of a 60 s iPhone clip. Whether it happens depends on the input in ways small changes flip: the same opening transcribed fully as a 30 s or 60 s cut. Details are in the [spec](../specs/whisper-coverage.md).

## Decision
- After the first pass, find holes: stretches of more than 3 s between word starts, or between an end of the audio and the nearest word, with at least 1.5 s outside `silence_gaps`.
- Transcribe each hole again with the same loaded model, with 1 s of silence before and after it, in the language the first pass detected. Words found inside the hole replace the first pass's words there. One round only.
- The first pass is not padded and runs exactly as before, so a run without holes gives identical words.
- Report every re-run, with its words found, in `asr_note`. No schema change.

## Alternatives Considered

### Tune whisper's thresholds
- Turning off the no-speech check (`no_speech_thold` 1.0) changed nothing on the failing inputs, and the single-timestamp skip has no parameter.

### Pad the first pass as well
- Padding the whole recording by 1 s changed the text of 4 of 5 hole-free 60 s cuts (punctuation, a few spellings, starts within 60 ms), which breaks identical output on clean runs.

### Re-run holes without padding, or with padding only after
- On the iPhone clip, no padding recovered 0 words in 9.9–15.6 s, 0.5 s before recovered 7, and 1 s before and after recovered 10, bringing the clip to 183 words.

### A voice activity detection model, or splitting the whole recording in advance
- A new model download and a different first pass for every run, for a problem that a check on the output already catches.

## Consequences
- Lost speech comes back in most measured holes. Some holes stay empty, and `asr_note` says which.
- Each hole costs a 30 s whisper window, so `asr_ms` grows with the number of holes: on the iPhone clip from 20.8 s to 33.1 s for two holes.
- Stretches of music or tone are re-run too. On a 25 s tone the second pass found nothing.
- The whisper.cpp DTW abort (PR #4096) is not addressed here; the spec's Task 3 moves whisper into a child process for that.
