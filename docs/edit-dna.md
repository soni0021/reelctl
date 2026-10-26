# edit_dna.json reference

`reelctl decompile` writes one JSON object per video. The Rust types are in [crates/edit-dna/src/lib.rs](../crates/edit-dna/src/lib.rs), and the JSON Schema generated from them is [crates/edit-dna/schema/edit_dna.schema.json](../crates/edit-dna/schema/edit_dna.schema.json). After changing the types, regenerate the schema with `cargo run -p edit-dna --example schema`, and bump `SCHEMA_VERSION` when the format changes. Version 2 (2026-10-25) added `silence_threshold_db` and `noise_floor_db`. Version 1 files still parse: a missing `silence_threshold_db` reads as −40, the fixed threshold version 1 used, and a missing `noise_floor_db` reads as `null`. So `diff` accepts outlier files from either version.

Every field is always present. Times are whole milliseconds. Video times count from the video stream's start time and audio times from the first decoded audio sample; these usually line up, but the code does not align them.

## Video

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | integer | Currently `2` |
| `duration_ms` | integer | The longer of the container's duration and the last frame's time plus one frame |
| `fps` | number | The stream's average frame rate, else its base rate, else 30 |
| `frame_size` | object | `width` and `height` of the decoded frames, in pixels |
| `frame_count` | integer | Number of frames decoded |
| `decode_note` | string | How the video was decoded: in software, or on the Apple media engine (HEVC on macOS), or in software after the media engine failed (with the reason), or in software because `--decoder software` asked for it. For 10-bit video it adds that the top 8 bits of each sample were read, and for an HLG or PQ transfer that the input was HDR and colors were not tone-mapped |

## Cuts

`cuts` lists hard cuts in time order:

| Field | Meaning |
|---|---|
| `frame_index` | Index of the first frame of the new shot |
| `time_ms` | That frame's presentation time |
| `interval_ms` | Time since the previous cut, or since 0 for the first cut |
| `scene_score` | Detector score from 0 to 1, rounded to 4 decimals |

The scene-score detector applies the formula behind FFmpeg's `scene` value (from the `select` filter) to a grid of roughly 160×90 luma samples, and records a cut wherever the score is above 0.30. It has been tested only on hard cuts between solid colors; dissolves, whip pans, flashes, and speed ramps are untested. See [ADR-002](decisions/ADR-002-scene-score-cut-detector.md).

`cut_detector` names the detector that produced the list: `"ffmpeg_scene_score"` today, with `"trans_net_v2"` reserved for TransNetV2. `cut_detector_note` says why that detector ran, including the reason for any fallback.

## Audio

Audio is mixed to mono and resampled to 48 kHz first, as one stream, so times do not drift with the input's sample rate.

| Field | Meaning |
|---|---|
| `silence_gaps` | `{start_ms, end_ms, duration_ms}` for each run of 10 ms windows whose mean-square level is below the recording's silence threshold, if the run lasts at least 80 ms. Sounds shorter than 30 ms inside a run do not end it. The threshold is `max(−40, min(p10 + 6, p90 − 12))` dB, where p10 and p90 are the 10th and 90th percentiles of all window levels in the recording: 6 dB above the noise floor, at least 12 dB below the loud part, and never below −40 dBFS. A pause under room noise is a gap; a clean recording keeps −40 dBFS. The 180 ms dead-air limit is applied later, by `diff`. See [ADR-008](decisions/ADR-008-silence-relative-to-the-noise-floor.md). |
| `silence_threshold_db` | The threshold used for this recording, in dB, rounded to 2 decimals. −40 for a clean recording or one with no audio |
| `noise_floor_db` | The 10th-percentile window level, in dB, rounded to 2 decimals: the noise-floor estimate the threshold starts from. −120 means at least a tenth of the recording is digital silence. `null` when there is no audio |
| `integrated_lufs` | Integrated loudness following ITU-R BS.1770 / EBU R128 (K-weighting; 400 ms blocks every 100 ms; −70 LUFS absolute gate; −10 LU relative gate), rounded to 2 decimals. −70 means silence or no audio. It is measured on the mono mix, so it can differ from a stereo meter. |
| `words` | `{text, start_ms, end_ms}` entries: transcribed words with a whisper model, voiced bursts without one; see below. |
| `words_per_minute` | Number of `words` divided by the duration in minutes, rounded to 2 decimals |
| `asr_backend` | `"whisper_cpp"` when `--whisper-model` was given, otherwise `"silence_bursts"` |
| `asr_note` | Which backend ran and how. For whisper.cpp: its version, the model's file name (not its path), the DTW preset, the detected language, and which stretches were transcribed a second time (see below) |

With `"whisper_cpp"`, `text` is the word as whisper.cpp wrote it, with any attached punctuation, such as `air.`. `start_ms` is the word's DTW timestamp. `end_ms` is the next word's `start_ms`, or the start of the next silence gap if that is earlier, so a word never runs into a gap that `silence_gaps` lists; it can run into a pause louder than the recording's silence threshold. Times have 10 ms resolution. `words_per_minute` is a real speaking rate. See [ADR-007](decisions/ADR-007-whisper-cpp-word-timing.md).

whisper.cpp can skip speech without saying so. So after the first pass, every stretch of more than 3 s with no word start and at least 1.5 s of sound (outside `silence_gaps`) is transcribed again on its own, with 1 s of silence before and after it and the language the first pass detected. Words the second pass finds inside the stretch replace the first pass's words there; if it finds none, the first pass stands. `asr_note` lists each such stretch with the number of words found, for example `16.0-30.9 s: 51`. A stretch of music or tone is also re-run, and usually comes back empty. See [ADR-009](decisions/ADR-009-transcribe-skipped-stretches-again.md).

If whisper.cpp aborts during a pass, that pass is run again without DTW ([ADR-010](decisions/ADR-010-run-whisper-in-a-child-process.md)), and `asr_note` names it. Its words then start at whisper's segment timestamps, which can run into pauses.

With `"silence_bursts"`, each word is a stretch of 10 ms windows at or above the recording's silence threshold (the one `silence_gaps` uses) that ends at the first 40 ms below it, kept if it lasts at least 40 ms. `text` is a placeholder: `burst-0`, `burst-1`, and so on. A continuous tone or music bed counts as one long word, so `words_per_minute` is a burst rate, not a speaking rate. See [ADR-003](decisions/ADR-003-silence-burst-word-estimator.md).

## Color

`shot_histograms` has one entry per shot, so it is one longer than `cuts`:

| Field | Meaning |
|---|---|
| `shot_index` | Shot number, from 0 |
| `start_ms`, `end_ms` | Shot boundaries: 0, the cut times, and `duration_ms` |
| `start_frame`, `end_frame` | First frame index, and one past the last |
| `mean_r`, `mean_g`, `mean_b` | Mean 8-bit RGB value over the shot, rounded to 2 decimals |
| `bins_r`, `bins_g`, `bins_b` | 8 bins per channel (0–31, 32–63, …, 224–255); each value is the fraction of samples in that bin, rounded to 4 decimals |

Each frame contributes a grid of roughly 32×18 samples, converted from YUV to RGB with BT.601 coefficients (full range for `yuvj420p` and for frames whose metadata says full range). Most HD video uses BT.709, so absolute colors are slightly off; use the histograms to compare shots rather than as exact color.

## Payoff window

The narrative rubric in planer.md puts the payoff around 9 s. `payoff_window` records whether a cut lands there:

| Field | Meaning |
|---|---|
| `marker_ms` | 9000 |
| `window_start_ms`, `window_end_ms` | 8000 and 10000, both inclusive |
| `has_cue` | `true` if any cut falls inside the window |
| `cue_ms`, `cue_frame_index` | The first such cut, or `null` |

Only cuts count as cues. A zoom, caption, or graphic at 9 s is not detected.

## Timing

| Field | Meaning |
|---|---|
| `signal_ms` | Measured time of the structural work: decode (with cut scoring, color sampling, and thumbnails), cut and histogram analysis, silence, loudness, and writing this file. The 500 ms target applies to this number. |
| `asr_ms` | Measured time of word timing. Not part of the 500 ms target. |

`signal_ms` is the sum of the timed stages, so process start, word timing, PNG writing, and the in-memory copies between stages are not in it. See [ADR-005](decisions/ADR-005-signal-ms-measures-the-structural-path.md).

## Not computed yet

| Field | Value |
|---|---|
| `zoom_curves` | `{"status": "unimplemented", "reason": "..."}`. Optical-flow Bézier zoom curves are Pass 1b. |
| `cube_lut` | `{"status": "unimplemented", "reason": "..."}`. `.cube` LUT synthesis is Pass 1b; the per-shot histograms are recorded instead. |

`"unimplemented"` is the only `status` value today. Check `status` before reading these fields. See [ADR-004](decisions/ADR-004-report-missing-passes-in-the-output.md).
