# Status

Last updated 2026-10-25. Update this page in the same commit as any change that affects it.

## Summary

P0, the Rust decompiler and reel-diff CLI, is built and its tests pass. It meets the 500 ms target on the synthetic 60-second benchmark clip but misses it on real footage: 60 s of an iPhone recording, converted to 8-bit H.264, took 2.1–2.8 s ([First real footage](#first-real-footage-2026-10-20)). Given a whisper model, `decompile` now produces real words timed with DTW, including Hinglish (merged into `main` on 2026-10-21). Silence is now detected relative to each recording's noise floor, so `diff` finds dead air under room noise (branch `feature/relative-silence`, awaiting review). `diff` does not yet compare pacing against the outlier. P1–P3 in [planer.md](../planer.md) have not started.

## Verified 2026-10-20

Apple M3 (8 cores, 16 GB), macOS 26.5, rustc 1.98.1, FFmpeg 8.1.

| Check | Result |
|---|---|
| `cargo check --workspace --all-targets` | Passes with 23 warnings (see Known issues) |
| `cargo test --workspace` | 2026-10-21, on `main`: 31 of 31 pass, plus the whisper model test, which is ignored by default and passes with both models. 2026-10-25, on `feature/relative-silence`: 43 of 43, and the model test passes with both models |
| 60 s benchmark, release build | `signal_ms` 310, 316, 284 over three runs; the target is under 500 |
| 12 s fixtures (640×360), release build | `signal_ms` 11–21 |

The benchmark clip (`bash fixtures/generate.sh bench60`) is 1920×1080 at 30 fps: 30 s of solid red, then 30 s of solid white, over a sine tone, encoded as H.264 Constrained Baseline without B-frames. Flat color with simple encoding is about as cheap to decode as video gets. Real footage has texture and motion and is usually encoded with B-frames, so its `signal_ms` is not known yet.

`signal_ms` is not wall-clock time. It leaves out process start, word timing, and PNG writing ([ADR-005](decisions/ADR-005-signal-ms-measures-the-structural-path.md)). The whole benchmark command took 0.30–0.80 s.

Reproduce the benchmark:

```bash
cargo build --release
bash fixtures/generate.sh bench60 /tmp/bench60.mp4
REELCTL_PROFILE=1 target/release/reelctl decompile \
  --input /tmp/bench60.mp4 --out /tmp/bench60.edit_dna.json
```

## Word timing, 2026-10-21

Same machine, release build. whisper.cpp 1.8.3 on the CPU with 4 threads (whisper-rs's default). Three runs each.

| Check | Result |
|---|---|
| First build of whisper.cpp | 41 s for the test profile, 24 s for release; no extra link flags |
| 60 s benchmark `signal_ms`, no model | 261, 242, 245; earlier 284–316. ASR is not part of it |
| `speech12` (12 s, `say` speech), `base.en` | `asr_ms` 398, 365, 361. All 12 words; "air." ends at 1,280 ms, where the planted pause starts |
| `speech12`, Apex | `asr_ms` 10,284, 11,859, 10,115. All 12 words, same pause boundary. whisper.cpp pads audio to 30 s windows, so 12 s costs about half of 60 s |
| 60 s real Hinglish, Apex | `asr_ms` 21,826, 21,673, 22,271 |
| Threads, Apex on 60 s, standalone probe | 4: 14.2 and 15.1 s; 6: 13.5 and 15.1 s; 8: 14.3 and 14.3 s. No difference beyond run-to-run noise, so the default stays |
| Language detection, Apex on 60 s, standalone probe | Auto-detect: 20.3 and 23.1 s. Forced English: 15.2 and 16.5 s. Identical text both ways; whisper.cpp reports `en`, since the model writes Roman script. Detection accounts for most of the gap between `reelctl` and the probe |
| stderr during transcription | Empty |

## Silence relative to the noise floor, 2026-10-25

Same machine, release builds of `main` and `feature/relative-silence` on the same inputs ([spec](specs/relative-silence.md), [ADR-008](decisions/ADR-008-silence-relative-to-the-noise-floor.md)).

| Check | Result |
|---|---|
| `draft12`, `outlier12`, `bench60`, `speech12` | `silence_gaps` and burst words identical to `main` |
| `noisy12` (`draft12` with white noise under the planted pause) | `main`: no gap of 180 ms or more and no `dead_air` flag. Branch: threshold −32.85 dB, and the planted pause, 2,010–2,510 ms, is the only long gap and the only `dead_air` flag |
| Real recording, first 60 s | Threshold −35.47 dB, noise floor −41.47 dB. Gaps of 180 ms or more: 1 on `main`, 31 on the branch; of 300 ms or more: 0 and 24. No Apex word start lies more than 100 ms inside a gap |
| Word ends on that recording, Apex, against the 19 pauses ffmpeg `silencedetect` finds at −25 dB and 0.3 s | Words running more than 200 ms into a pause: 8 of 125 on `main`, 2 of 125 on the branch ("mile." and "aaj."). No word starts more than 200 ms inside one, on either build |
| 60 s benchmark `signal_ms`, five interleaved runs each | `main` 242–248 ms, branch 243–250 ms; the silence stage takes 1 ms on both |

The real clip was re-cut from the original recording on 2026-10-25 with its audio copied, not re-encoded, so its word count (125) differs from the 2026-10-21 runs (183); see Known issues.

## First real footage, 2026-10-20

The input was an 87 s iPhone recording: HEVC Main 10 with HDR (HLG), 1080×1920 at 24 fps with B-frames, AAC stereo, and a 4-channel spatial-audio track. It ran on the same machine as above. The file stays outside the repo.

| Check | Result |
|---|---|
| `decompile` on the original file | Stops with `decode video frame: unsupported pixel format`, because the video is 10-bit |
| First 60 s converted to 8-bit H.264 High (`-vf format=yuv420p -c:v libx264 -preset fast -crf 20 -profile:v high`), release build | `signal_ms` 2828, 2205, 2087 over three runs, against a target of 500. Decoding was over 99% of it |
| FFmpeg decoding every frame of that file in software | 2.12 s (680 fps) |
| FFmpeg decoding with VideoToolbox | 5.71 s |
| FFmpeg decoding keyframes only | 0.44 s, but only 62 of 1,440 frames, about one per second |
| Silence gaps at −40 dBFS | 26 gaps, the longest 190 ms, so no pause of 0.3 s or more. ffmpeg `silencedetect` at −25 dB finds 19 pauses of 0.3 s or more. Correction, 2026-10-25: −25 dB is ffmpeg's sample-peak measure, not the room-noise level. Measured the way `reelctl` measures it, the 10th-percentile window level is −41.5 dB, so the noise sits just above −40 ([Silence relative to the noise floor](#silence-relative-to-the-noise-floor-2026-10-25)) |
| whisper `base.en`, on the `feature/whisper-asr` branch | One invented English sentence, repeated. The speech is Hinglish, which this English-only model cannot read |
| Oriserve Whisper-Hindi2Hinglish-Apex (Q8_0, large-v3-turbo fine-tune), 2026-10-21 | Real Roman-script Hinglish, 183 words in 60 s, in 14.5 s. With plain timestamps, 21 of 183 words stretched more than 200 ms into one of the 19 pauses found at −25 dB; with DTW timestamps, none did, at the same speed |
| `reelctl decompile --whisper-model` with Apex, release build, 2026-10-21 | `asr_backend` `whisper_cpp`, 183 words, 183 words per minute, `asr_ms` 19,182, no whisper.cpp output on stderr. whisper.cpp reported the language as `en`, since the model writes Roman script. No word starts inside one of the 19 pauses, but 9 words end more than 200 ms into one, because reelctl's −40 dBFS silence gaps, where word ends stop, miss those pauses |

What this means:

- Decoding every frame at 1080p cannot meet 500 ms on this machine, because FFmpeg alone takes about 2.1 s. Meeting the target needs a different decode strategy, or a different target.
- A fixed −40 dBFS threshold misses pauses when there is room noise, so `diff` would report no dead air in a recording like this. Addressed on 2026-10-25 by a per-recording threshold.

## What works

- `reelctl decompile` writes `edit_dna.json` with cuts, silence gaps (with a threshold set from each recording's noise floor), integrated loudness, per-shot RGB histograms, and the 8–10 s payoff cue, plus one `timeline_view` PNG per cut.
- `reelctl diff` flags dead air over 180 ms, more than 2.5 s without a cut, and a missing payoff cue when the outlier has one.
- The JSON Schema is generated from the Rust types and matches them.

## Gaps

Already listed in [AGENTS.md](../AGENTS.md) as P0 gaps:

| Gap | Effect now | To close it |
|---|---|---|
| Word timing needs a model | Without `--whisper-model`, `words` are voiced bursts named `burst-N` and `words_per_minute` counts bursts | Closed 2026-10-21, merged into `main`: with a model, whisper.cpp gives real words timed with DTW ([ADR-007](decisions/ADR-007-whisper-cpp-word-timing.md)). `reelctl` never downloads a model |
| TransNetV2 not linked | Every cut comes from the scene-score detector; `--cut-detector transnet` exits with an error | TransNetV2 ONNX weights and an `ort` dependency |
| Zoom curves and `.cube` LUT | Reported as `"status": "unimplemented"` | Pass 1b work |

Not yet listed in AGENTS.md:

- **`diff` barely uses the outlier.** Dead air and static windows are properties of the draft alone; the outlier only feeds the payoff check. `diff` takes one outlier, where planer.md asks for one to three and a second-by-second pacing comparison. This is the main gap against the P0 success metric.
- **Static windows count only cuts.** planer.md defines a static window as more than 2.5 s without B-roll, zoom, or a graphic callout. Nothing detects zooms or graphics yet, so a punch-in without a cut is still flagged.
- **The 500 ms target fails on real footage.** Only one real recording has been run so far ([First real footage](#first-real-footage-2026-10-20)). The tests still use only synthetic clips ([ADR-006](decisions/ADR-006-synthetic-offline-fixtures.md)).
- **Silence on noisy recordings** (closed on `feature/relative-silence`, awaiting merge). The fixed −40 dBFS threshold found no pause of 0.3 s or more on the real recording; the per-recording threshold finds 24. Its constants were tuned on that one recording, so music beds, loud backgrounds, and very quiet speakers are unmeasured.
- **10-bit video is rejected.** Decoding stops with `unsupported pixel format` on anything other than 8-bit 4:2:0. That rules out iPhone HDR recordings until they are converted, which the real-footage run confirmed.
- **`--draft` takes an MP4.** planer.md specifies a `.otio` timeline; OpenTimelineIO is P2 work.

## Known issues

- `cargo check` reports 22 warnings in library and binary code (`unreachable_patterns`, `unreachable_code`), plus 1 in a `diff` unit test. The 22 come from 11 `other => { ... }` arms placed after `match` arms that already cover every case. They do not change behavior.
- In a `timeline_view` PNG, the thumbnails and the waveform use different time scales. The 8 thumbnails cover about 0.27 s at 30 fps; the waveform and word ticks cover 1.5 s either side of the cut, across the same width.
- The JSON value for TransNetV2 is `"trans_net_v2"`, but `CutDetectorKind::as_str()` returns `"transnet_v2"`. Nothing emits either until TransNetV2 is linked.
- Colors are converted from YUV with BT.601 coefficients, while most HD video uses BT.709, so absolute RGB values are slightly off.
- whisper with Apex skipped 15.96–30.86 s of the re-cut real clip (copied audio), on both `main` and the branch, although that stretch is no quieter than the rest. The earlier cut of the same recording, with re-encoded audio, did not lose it. Not investigated yet.
- Thresholds read from FFmpeg tools are not comparable with `reelctl`'s: FFmpeg's `-ac 1` downmix reads about 3 dB louder than `reelctl`'s channel average, and `silencedetect` compares sample peaks rather than 10 ms mean-square levels.
- The quality interview in [CONSTRAINTS.md](../CONSTRAINTS.md) has not been run, so only the floor rules plus `cargo check` and `cargo test` are enforced.

## Next steps

Done: real word timing with whisper.cpp ([spec](specs/whisper-asr.md)), merged 2026-10-21. A possible follow-up, not yet specified: a `--whisper-language` flag, which would skip language detection and save about 6 s per run with the Apex model.

Then, to finish P0 against its own success metric before starting P1:

1. Find a decode strategy that meets 500 ms on real footage, or revise the target.
2. Read 10-bit video, such as iPhone HDR recordings.
3. Detect silence relative to each recording's noise floor instead of at a fixed −40 dBFS. Done on `feature/relative-silence` ([spec](specs/relative-silence.md)), awaiting review and merge.
4. Make `diff` comparative: accept one to three outliers and compare cut rhythm second by second.
5. Link TransNetV2. It needs ONNX weights, a download that needs an explicit go-ahead.
