# ADR-007: Time words with whisper.cpp and DTW when a model is given

## Status
Accepted. Supersedes [ADR-003](ADR-003-silence-burst-word-estimator.md).

## Date
2026-10-21

## Context
ADR-003 estimated words from voiced bursts because whisper.cpp could not be built. CMake is now installed, and `whisper-rs` 0.16.0 builds whisper.cpp 1.8.3 on an Apple M3 with no extra flags. Two findings shaped the design ([spec](../specs/whisper-asr.md#revision-2026-10-21)):

- whisper.cpp's segment timestamps stretched words across pauses: 21 of 183 words on 60 s of real speech ran more than 200 ms into a pause. Starting each word at its DTW timestamp put none there, at no extra cost with a large-v3-turbo model.
- The target speech is Hinglish. `base.en` invented text; Oriserve Whisper-Hindi2Hinglish-Apex, a large-v3-turbo fine-tune, transcribed it. Its file name matches no whisper.cpp model name.

## Decision
- With `--whisper-model` or `REELCTL_WHISPER_MODEL`, `asr::transcribe` runs whisper.cpp on the CPU (Accelerate; Metal off) with greedy sampling and one segment per word, and sets `asr_backend` to `"whisper_cpp"`. Without a model, the ADR-003 estimator runs.
- A word starts at the DTW timestamp of its first text token. It ends at the next word's start or the next measured silence gap, whichever is earlier.
- The DTW alignment-head preset comes from the model header's shape (`n_vocab`, `n_audio_state`, `n_audio_layer`, `n_text_layer`), not its file name. An unknown shape fails.
- Multilingual models detect the language; English-only models use English.
- A model path that is missing or unusable exits with code 1 ([ADR-004](ADR-004-report-missing-passes-in-the-output.md)). `reelctl` never picks a model itself.
- Resampling to 16 kHz and transcription count as `asr_ms`, outside `signal_ms` ([ADR-005](ADR-005-signal-ms-measures-the-structural-path.md)).

## Alternatives Considered

### Plain token timestamps
- Simpler, and no preset needed, but words smeared across pauses, which breaks the dead-air checks that word timing is meant to support.

### Choose the DTW preset from the file name
- Fails on renamed and fine-tuned files such as the Apex model.

### DTW over the top text layers (`DtwMode::TopMost`)
- Needs no preset, but uses heads that were not chosen for alignment. Not measured; the preset already worked on real speech.

### A Hinglish model in another runtime (Python, ONNX)
- Would add a second runtime to a Rust CLI. The Apex fine-tune runs in whisper.cpp unchanged.

## Consequences
- Every build needs CMake and compiles whisper.cpp once.
- Word timing is far slower than the estimator: about 19 s for 60 s of speech with the Apex model. It stays outside the 500 ms target.
- Word ends are only as good as `silence_gaps`. Under room noise the −40 dBFS threshold misses pauses, and a word's end can run into one. [ADR-008](ADR-008-silence-relative-to-the-noise-floor.md) (2026-10-25) sets the threshold per recording to address this.
- Models fine-tuned from a standard shape reuse that shape's alignment heads. That worked for Apex; other fine-tunes are unmeasured.
