# Status

Last updated 2026-10-26. Update this page in the same commit as any change that affects it.

## Summary

P0, the Rust decompiler and reel-diff CLI, is built and its tests pass. It meets the 500 ms target on the synthetic 60-second benchmark clip but misses it on real footage: 60 s of an iPhone recording, converted to 8-bit H.264, took 2.1–2.8 s ([First real footage](#first-real-footage-2026-10-20)). Given a whisper model, `decompile` now produces real words timed with DTW, including Hinglish (merged into `main` on 2026-10-21). Silence is now detected relative to each recording's noise floor, so `diff` finds dead air under room noise (merged into `main` on 2026-10-25). Stretches whisper skips are transcribed again, and whisper runs in a child process so a whisper.cpp abort cannot end `reelctl` (merged into `main` on 2026-10-26). On branch `feature/decode-inputs`, not yet merged, `decompile` reads 10-bit 4:2:0 video, such as iPhone HEVC HDR recordings (the original 87 s iPhone recording decompiles with the same cut and silence gaps as an 8-bit conversion) ([10-bit video](#10-bit-video-and-441-khz-audio-2026-10-26)). `diff` does not yet compare pacing against the outlier. P1–P3 in [planer.md](../planer.md) have not started.

## Verified 2026-10-20

Apple M3 (8 cores, 16 GB), macOS 26.5, rustc 1.98.1, FFmpeg 8.1.

| Check | Result |
|---|---|
| `cargo check --workspace --all-targets` | 2026-10-20: passes with 23 warnings. 2026-10-26, on `feature/decode-inputs`: 21 (see Known issues) |
| `cargo test --workspace` | 2026-10-21, on `main`: 31 of 31 pass, plus the whisper model test, which is ignored by default and passes with both models. 2026-10-26, on `main` after the whisper-coverage merge: 57 of 57, and the 2 whisper model tests pass with both models. 2026-10-26, on `feature/decode-inputs` after 10-bit reading: 66 of 66; after the stream resampler and the audit's extra tests: 76 of 76, and the 2 ignored whisper model tests pass with both `base.en` (4.2 s for the file's 3 tests) and Apex (31.3 s) |
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

## 10-bit video and 44.1 kHz audio, 2026-10-26

Same machine, release builds of `main` and `feature/decode-inputs` on the same inputs ([spec](specs/decode-inputs.md), [ADR-011](decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)). The real iPhone recording is the last two rows. It is the 87 s file from [First real footage](#first-real-footage-2026-10-20), kept outside the repo.

| Check | Result |
|---|---|
| `draft12`, `outlier12`, `noisy12`, `bench60`, `speech12`, `tonespeech` | `edit_dna.json` identical to `main` apart from `signal_ms` and `asr_ms`, including `decode_note`; `timeline_view` PNGs byte-identical |
| `draft12_10bit` (`draft12` as 10-bit HEVC Main 10, tagged BT.2020 and HLG) | `main` stops with `unsupported pixel format`. Branch: same cut frame (180), silence gaps, threshold, loudness, and payoff result as `draft12`; per-shot mean RGB (254.67, 0, 0) and (255, 255, 255) against (255, 0, 0) and (255, 255, 255). `decode_note` adds the 10-bit and HLG sentences |
| 60 s benchmark `signal_ms`, ten alternating runs each | `main` 244–263 ms, median 248; branch 246–254 ms, median 248.5 |
| `generate.sh all` | Found by the audit subagent: it encoded `outlier12`, `noisy12`, and `bench60` as 10-bit HEVC, because `draft12_10bit` changed a shared setting. Fixed and checked by hand: `all` now gives the same bytes as the single-kind runs. No automated test covers `all` |
| `draft12_44k` (60 s, 44.1 kHz AAC, mute switched on at 1,024-sample frames: 50.0159–50.5034 s in the source) | Per-frame resampling (`main`): pause at 50,000–50,470 ms, 16 and 33 ms early. One stream: 50,020–50,500 ms. The same clip as 44.1 kHz PCM (a temporary file, not in the repo) gives the same gap, so AAC is not the cause of the remaining 4 ms |
| After the resampler: the six existing fixtures, ten more alternating benchmark runs | `edit_dna.json` and PNGs identical to the 10-bit build; `signal_ms` 245–279 ms, median 247.5, against 246–252 ms, median 247.5 |
| Final build against the two earlier builds, ten interleaved benchmark runs each, quiet machine (load 2.5) | `signal_ms` medians 244.5 (10-bit build), 243.5 (resampler build), 247.0 (final); ranges 239–264, 239–252, 239–262 |
| Original `IMG_1435.MOV` (HEVC Main 10, HLG, 1080×1920, 87.3 s, AAC stereo plus a 4-channel spatial track) | `main` stopped with `unsupported pixel format`. Branch: exit 0, 1 cut at frame 1726 (71.5 s), 106 silence gaps, threshold and noise floor as for the reference below. `decode_note` says 10-bit and HLG, not tone-mapped |
| Same file as an 8-bit H.264 reference (`-noautorotate -vf format=yuv420p -fps_mode passthrough -c:v libx264 -preset fast -crf 20`, audio copied, whole file) | Same cut frame (1726). Its time is 71,548 ms in the original and 71,541 ms in the reference, and `ffprobe` shows the same 6.7 ms in the containers' own timestamps for that frame (71.548333 s and 71.541667 s), so the difference is not `reelctl`'s; silence gaps, threshold, noise floor, loudness, frame count, and frame size equal; per-shot mean RGB within 1.4. The stereo AAC track was used, not the 4-channel one. A first reference made without `-noautorotate -fps_mode passthrough` was rotated and retimed to constant 24 fps, which put the cut 35 ms and 8 frame indexes away; it was not a valid reference |
| Decode time of that file, three alternating runs | Original: `signal_ms` 8,356, 8,504, 8,606, over 99% of it decoding. 8-bit reference: 3,171, 2,893, 2,960. 10-bit HEVC costs about 2.8 times the 8-bit H.264 decode; both miss the 500 ms target, which is module `fast-decode` |

## Whisper coverage, 2026-10-25

Same machine, release build of `feature/whisper-coverage` (merged 2026-10-26), Apex model unless noted ([spec](specs/whisper-coverage.md), [ADR-009](decisions/ADR-009-transcribe-skipped-stretches-again.md), [ADR-010](decisions/ADR-010-run-whisper-in-a-child-process.md)). The load average stayed between 4 and 6 throughout, so single `asr_ms` values vary by tens of percent between runs.

| Check | Result |
|---|---|
| iPhone clip, first 60 s with copied audio | 125 → 183 words. Re-runs of 9.9–15.6 s and 16.0–30.9 s found 9 and 51 words. `asr_ms` 20.8 s → 33.1 s in-process; through the worker in a later run, 55.4 s (first pass 39.4 s, re-runs 15.9 s) |
| NEET video, 18 min | 3,894 → 4,040 words. Five holes re-run: 0–30.4 s found 86 words ("Hello, kya haal chaal? Welcome to my channel…"), 355.8, 505.5, and 985.9 s found 27, 18, and 17, and 96.7–100.5 s found "in 672853.", which looks invented. Through the worker: first pass 528.0 s, re-runs 44.1 s, which is 8.4% of the first pass in the same run, or 11.0% of the 401.6 s first pass measured earlier under lighter load |
| Runs without holes | Identical words: `speech12` with `base.en` and Apex, and five 30–63 s NEET cuts |
| Worker against in-process | Identical words on `speech12` (both models), the iPhone clip, and the NEET video. Start-up cost not measurable above noise: three alternating `speech12` runs took 9.7–12.5 s through the worker and 13.0–18.2 s in-process on `main` |
| Temporary audio | Removed after every run; none left in `$TMPDIR` |
| whisper.cpp abort | Not reproduced through `reelctl`. The retry path is covered by unit tests with a stand-in worker that aborts |

## Long-form YouTube video, 2026-10-25

"20000 Questions for NEET 2027 FREE" by Dr. Anand Mani (youtu.be/4taOl64cb6U), 18 min 9 s, downloaded with `yt-dlp` for a manual run and kept outside the repo. Only the 640×360 H.264 stream with HE-AAC 44.1 kHz audio would download; 1080p requests got HTTP 403 with `yt-dlp` 2026.3.17. Same machine, release build of `main`.

| Check | Result |
|---|---|
| Structural pass, no model | 3.0 s wall clock, 250 MB peak memory; `signal_ms` 2,975, of which decode 2,795 |
| Audio | Noise floor −68.2 dB, so the threshold stays at −40 dB; −18.93 LUFS |
| Silence and `diff` | 782 gaps (21% of the runtime); 445 `dead_air` flags totalling 194 s, 99 of them 0.5 s or longer and 17 over 1 s, about 24 a minute throughout |
| Cuts | 0. One continuous camera setup with app screens composited beside the presenter, full-screen title cards, and what look like punch-in zooms. FFmpeg's scene score finds nothing above 0.3 and 9 frames above 0.08, all overlay changes or possible jump cuts. `diff` therefore flags the whole video as one static window |
| Words, Apex | 3,894 words, 214.51 words per minute (196–257 in each full minute), `asr_ms` 401,586, nothing on stderr |
| First 30 s | No words before 30.44 s although only 4.7 s of it is silent. Cuts of 0–30 s and 0–60 s transcribe it ("Hello, kya haal chaal? Welcome to my channel…"), with re-encoded or copied audio. Appending the loudest 3 s of the video to the 60 s cut did not reproduce the loss |

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
- **Silence on noisy recordings** (closed 2026-10-25, merged into `main`). The fixed −40 dBFS threshold found no pause of 0.3 s or more on the real recording; the per-recording threshold finds 24. Its constants were tuned on that one recording, so music beds, loud backgrounds, and very quiet speakers are unmeasured.
- **10-bit video** (closed 2026-10-26 on `feature/decode-inputs`, not yet merged). Decoding stopped with `unsupported pixel format` on anything other than 8-bit 4:2:0, which ruled out iPhone HDR recordings until they were converted. 10-bit 4:2:0 is now read from the top 8 bits of each sample ([ADR-011](decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)), tested on a synthetic clip and on the real recording (see [10-bit video and 44.1 kHz audio](#10-bit-video-and-441-khz-audio-2026-10-26)); it decodes 2.8 times slower than 8-bit H.264. 4:2:2, 4:4:4, and 12-bit video still stop with an error, which now names the format (only 4:2:2 is covered by a test).
- **`--draft` takes an MP4.** planer.md specifies a `.otio` timeline; OpenTimelineIO is P2 work.

## Known issues

- `cargo check` reports 20 warnings in library and binary code (`unreachable_patterns`, `unreachable_code`), plus 1 in a `diff` unit test. The 20 come from 10 `other => { ... }` arms placed after `match` arms that already cover every case. They do not change behavior.
- In a `timeline_view` PNG, the thumbnails and the waveform use different time scales. The 8 thumbnails cover about 0.27 s at 30 fps; the waveform and word ticks cover 1.5 s either side of the cut, across the same width.
- The JSON value for TransNetV2 is `"trans_net_v2"`, but `CutDetectorKind::as_str()` returns `"transnet_v2"`. Nothing emits either until TransNetV2 is linked.
- Colors are converted from YUV with BT.601 coefficients, while most HD video uses BT.709, so absolute RGB values are slightly off.
- Audio sampled above 48 kHz (88.2 or 96 kHz) can alias, because the resampler is linear interpolation with no low-pass filter. Unmeasured; the rates seen so far (44.1 and 48 kHz) are not affected.
- `nv12` frames are always read as limited range, whatever their metadata says. FFmpeg's software decoders never output `nv12`, so no measured input is affected; 8-bit `yuv420p` with full-range metadata is also read as limited, as before. 10-bit follows the metadata.
- HDR video (HLG or PQ transfer, such as iPhone recordings) is not tone-mapped, so its per-shot colors and thumbnails are flatter and less saturated than the picture a viewer sees. `decode_note` says so ([ADR-011](decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)).
- whisper can silently drop up to a 30 s window of speech. Since 2026-10-26 such stretches are found and transcribed again, which recovered every measured loss ([Whisper coverage](#whisper-coverage-2026-10-25)). A stretch can still stay empty, and `asr_note` then lists it. Which of whisper.cpp's two skip rules fired in each case is not known.
- With the Apex model, 29 s of digital silence before English speech (`say`) produced 6 invented words at 0.1 s and a garbled first sentence. `base.en` did not do this. Found 2026-10-25; not addressed.
- Thresholds read from FFmpeg tools are not comparable with `reelctl`'s: FFmpeg's `-ac 1` downmix reads about 3 dB louder than `reelctl`'s channel average, and `silencedetect` compares sample peaks rather than 10 ms mean-square levels.
- The quality interview in [CONSTRAINTS.md](../CONSTRAINTS.md) has not been run, so only the floor rules plus `cargo check` and `cargo test` are enforced.

## Next steps

Done: real word timing with whisper.cpp ([spec](specs/whisper-asr.md)), merged 2026-10-21. A possible follow-up, not yet specified: a `--whisper-language` flag, which would skip language detection and save about 6 s per run with the Apex model.

Then, to finish P0 against its own success metric before starting P1. The [P0 capability map](specs/p0-capability-map.md) (approved 2026-10-25) orders these into modules: `whisper-coverage`, `decode-inputs`, `fast-decode`, `comparative-diff`, `visual-motion`. The list below predates it:

1. Find a decode strategy that meets 500 ms on real footage, or revise the target.
2. Read 10-bit video, such as iPhone HDR recordings. Done on `feature/decode-inputs` ([spec](specs/decode-inputs.md)), awaiting merge: 10-bit 4:2:0 is read, audio is resampled as one stream, and the real recording decompiles.
3. Detect silence relative to each recording's noise floor instead of at a fixed −40 dBFS. Done ([spec](specs/relative-silence.md)), merged 2026-10-25.
4. Make `diff` comparative: accept one to three outliers and compare cut rhythm second by second.
5. Link TransNetV2. It needs ONNX weights, a download that needs an explicit go-ahead.
