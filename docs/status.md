# Status

Last updated 2026-10-26. Update this page in the same commit as any change that affects it.

## Summary

P0, the Rust decompiler and reel-diff CLI, is built and its tests pass. It meets the 500 ms target on the synthetic 60-second benchmark clip but misses it on real footage: 60 s of an iPhone recording, converted to 8-bit H.264, took 2.1–2.8 s ([First real footage](#first-real-footage-2026-10-20)). Given a whisper model, `decompile` now produces real words timed with DTW, including Hinglish (merged into `main` on 2026-10-21). Silence is now detected relative to each recording's noise floor, so `diff` finds dead air under room noise (merged into `main` on 2026-10-25). Stretches whisper skips are transcribed again, and whisper runs in a child process so a whisper.cpp abort cannot end `reelctl` (merged into `main` on 2026-10-26). Since the merge of `feature/decode-inputs` on 2026-10-26, `decompile` reads 10-bit 4:2:0 video, such as iPhone HEVC HDR recordings (the original 87 s iPhone recording decompiles with the same cut and silence gaps as an 8-bit conversion) ([10-bit video](#10-bit-video-and-441-khz-audio-2026-10-26)). `diff` does not yet compare pacing against the outlier. P1–P3 in [planer.md](../planer.md) have not started.

## Verified 2026-10-20

Apple M3 (8 cores, 16 GB), macOS 26.5, rustc 1.98.1, FFmpeg 8.1.

| Check | Result |
|---|---|
| `cargo check --workspace --all-targets` | 2026-10-20: passes with 23 warnings. 2026-10-26, on `main` after the decode-inputs merge: 21. 2026-10-26, after removing the unreachable `match` arms: 0 |
| `cargo test --workspace` | 2026-10-21, on `main`: 31 of 31 pass, plus the whisper model test, which is ignored by default and passes with both models. 2026-10-26, on `main` after the whisper-coverage merge: 57 of 57, and the 2 whisper model tests pass with both models. 2026-10-26, on `main` after the decode-inputs merge: 66 of 66 after 10-bit reading, 76 of 76 after the stream resampler and the audit's extra tests, and the 2 ignored whisper model tests pass with both `base.en` (4.2 s for the file's 3 tests) and Apex (31.3 s) 2026-10-26, on `feature/quiet-audio`: 103 pass and 2 whisper model tests are ignored by default (run, with the Apex model, they pass). |
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

## Visual motion, 2026-10-28

Apple M3 MacBook Air, release build of `feature/visual-motion` ([spec](specs/visual-motion.md)). Real footage is kept outside the repo.

| Check | Result |
|---|---|
| `motion12` (540×960 portrait, still texture, planted punch-in 1.18 at frame 102, zoom-out at frame 150, box covering 12.0% at frame 192) | Exactly three events at frames 102, 150 and 192, no cut: `punch_in` scale 1.1794 (planted 1.18, centre 0.5, 0.5), `zoom_out` scale 0.8479 (0.8475), `overlay_change` area 0.1162 (0.1204) |
| `motion12_wide` (640×360 landscape copy) | The same three events at the same frames, scales 1.1794 and 0.8479, area 0.1161 |
| `still12`, `draft12` (solid colour, one planted cut) | No event; the cut stays a cut |
| `textured_cuts12` (probe the spec asked for: one shot zooms by itself over 300 frames, one has moving elements) | Cuts 3, 6, 9 s as before, no event: the slow source zoom is not a move that completes within 12 frames, which is the rule |
| `signal_ms` with the module against without it (release, median of 10 runs, 5 for the real files) | 60 s solid-colour `bench60`: 247 against 252 ms. `bench60_textured` (a punch-in every 5 s, where the scale fit runs): 397 against 370 ms (+7%). `motion12`: 160 against 158. The 180 s lecture proxy: 104 against 98. The 38.5 s NEET reel: 490 against 489 |
| The 38.5 s NEET reel | The cut at 8.47 s as before and one `overlay_change` at 14.1 s covering 25% of the frame: checked by eye, the question card is swapped and the face shrinks above it |
| The milk clip (15 s) | No event |
| The 28.6-minute lecture at 1280×720 | 20 events in 28.6 minutes, all `overlay_change` (areas 11–33%), `signal_ms` 10,311 (not measured without the module on this file; the 500 ms target already misses on real footage). Eight checked by eye against frames: a pop-up card at 50 s, a pen panel going at 741 s and board text appearing or changing at 468, 580 and 1,444 s are right by pixels (a board change cannot be told from an overlay); three (185, 310 and 997 s) are the speaker walking across the board, which the detector takes for an overlay. About five of eight are what the name says, and the lecture's own 614 s pop-up cut is not repeated as an event. The low-resolution proxy of the same lecture gives none |
| Mutation checks on the detector (agent that built it) | Many threshold changes survived the first run; tests were added until each constant was pinned, and a margin check no test shape could reach was removed |
| Mutation check on the wiring | Pointing `visual_events` at an empty list fails the planted-event and the `diff` tests |
| `cargo test --workspace` | 436 pass, 2 ignored (the whisper model tests); the detector adds 51 unit tests, `p0_fixture.rs` three end-to-end tests, and `edit-dna` four and `diff` five |

Known limits: the thresholds are tuned on the planted clips and these four recordings and are untested on fast-cut footage; a speaker walking across a static background can be read as an overlay; a board that is written on reads as `overlay_change`, not as a slide change; a move slower than about 12 to 16 frames (a run over 600 ms) is not an event; `punch_in` is reported once at the frame where the move completes, so its start is not recorded.

## Long-to-short clipper, 2026-10-28

Merged into `main` 2026-10-28 from `feature/long-to-short-clipper` ([spec](specs/long-to-short-clipper.md), [ADR-015](decisions/ADR-015-combine-measured-time-with-a-model-that-answers-by-segment-id.md); plan and checklist in `tasks/archive/long-to-short-clipper/`). The first live run shows the pipeline works and the rules do their job; the 9B model does not respect the 30–60 s length, so a prompt or model change is the next lever (Muse 1.2 was not run). Billing check from the same run: 87 sheets plus the transcript billed 65,852 input tokens against 139,077 estimated, so the bound overestimates by about 2× and does not underestimate sheets.

| Check | Result |
|---|---|
| `decompile::decode_frames_at`, release build, Apple M3 | 344 frames (every 5 s) from the 28.6-min 1080p lecture, scaled to 448×252 RGB: 10.8 s. The last requested time (1,715,000 ms) got the frame at 1,715,013 ms. 20 frames from a 31 MB proxy: 58 ms (decoding stops once the last time is served) |
| `understand` crate, offline | 204 unit tests (segmenter 21, clip rules 66, frame plan and request 61, assembly 33, OpenRouter 21, schema 2; 382 in the whole workspace, 2 ignored, `cargo test --workspace` 2026-10-28) and 12 end-to-end tests of `reelctl understand` through `--replay` and `--catalog`; mutation checks by the agents that built each part (25 on the clip rules, 7 on assembly, 14 on frames and request, 3 on the segmenter) and by me on the key handling (5, all caught) |
| Segmenter on the real 28.6-min lecture (whisper words) | 535 segments, median 1,720 ms, longest 27,990 ms, 2 over 15 s: the same counts as the prototype |
| `--dry-run` on the real lecture, no key, public price listing only | 346 frames in 87 sheets of 2×2, 535 segments, about 139,000 input tokens, cost upper bound $0.0276 on the first prompt and $0.0277 on the final one (default model, cap $0.50) |
| Clip cut points on the real lecture's 535 segments, one clip per segment | The start lands in a measured pause for 380 of 535 and falls back to 100 ms before the first word for 155; the end lands in a pause for 56 and falls back to 150 ms after the last word for 479. The fallbacks never cut into a word, and the rule has no slack on whisper's word times, so most ends are not in a pause. Whether that leaves audible dead air at the edges is for the live run |
| Live run, `qwen/qwen3.5-9b` on the 28.6-min lecture, 2026-10-28, user-approved, release build | 83.4 s for the model call, 95 s in all. Provider Parasail (bf16). 65,852 input and 6,502 output tokens; cost $0.0082 against a bound of $0.0276. 7 beats, 17 visual events, 1 cut (unlabelled). 3 clips and 1 rejected pick. **No clip met every rule**: both long picks were 2–4 minutes (tight 174 s and 97 s against a 60 s maximum) with the payoff 51–53 s in; the third was 25.8 s and its payoff segment lay outside it. The rejected pick had its cutoff before its hook. The rules flagged or rejected each one and changed nothing silently. Hooks and last lines are exact whisper text, because the model answers by segment ID |

Prompt tuning, 2026-10-28 ([ADR-016](decisions/ADR-016-state-the-clip-length-in-segments-not-seconds.md)), same lecture, frames and transcript, tight length 30–60 s as the pass line:

Each cell is one run (model output varies). Cells give clips within 30–60 s tight, of the clips returned.

| Prompt | Qwen3.5-9B | Muse 1.2 |
|---|---|---|
| Length in seconds (first) | 0 of 3; 0 meet every rule; $0.0082 | not run with this prompt |
| "Hard limit" wording | 0 of 1 (two picks rejected: start after end); $0.0090 | not run |
| Fixed counts, "about 3 s per segment" | 1 of 1 (three picks rejected: segment IDs that do not exist); $0.0075 | 2 of 5; 1 meets every rule; $0.1289 |
| Counts from this video's mean segment length (this change) | 4 of 5; 1 meets every rule; $0.0088 | 4 of 4; 2 meet every rule; $0.1271 |

Seven paid runs, about $0.30 in all. After the audit's fixes to the prompt arithmetic (a span counted with both ends, a short-video rule, no clamp on long segments; [ADR-016](decisions/ADR-016-state-the-clip-length-in-segments-not-seconds.md)) one more Qwen run gave 1 of 3 clips within 30–60 s tight, one meeting every rule, one pick rejected (cutoff before hook), $0.0079. That is worse than the 4 of 5 of the run above, with a prompt that differs by a few words, so the run-to-run spread on this model is large and neither number is a rate. The `--dry-run` for the final prompt is `input_tokens_estimate=139315 cost_upper_bound_usd=0.0277`.
The remaining flags are the payoff time (3.7–15 s in against 6–12 s) and one start 13 s into its sentence. In both runs the lecture's one cut stayed unlabelled. Muse reads the 346 frames one by one (`sheet=single`, bound $0.71, cap raised to $0.75 for the run).

Audit of the branch (a clean checkout of f31f5f8: fmt, check and 348 tests green, 18 mutation checks on cost, key and exit-code gates, 15 caught): 18 findings, fixed on the branch. A time past the last decodable frame returned a stale earlier frame (now the last frame, with a test that fails if the gate's `>` becomes `>=`, checked); the 10-bit hardware test now asserts the media engine ran; the written `understanding.json` is validated against its schema; `--every-s 0`, an empty transcript and a zero duration exit 2 before any network access, each with a test; every backend error is scrubbed of the key in one function, with loopback tests for a key echoed in a stream error and a malformed reply; model ids with `..` or empty segments are rejected; a word near `u64::MAX` no longer overflows; the docs for the clip end rule, the sample output and the cost figures were corrected. After the fixes: 364 tests pass, 2 ignored; the whole `understand` command through `--replay` on the lecture (decode, tiling and JPEG encoding, no model) takes 10.6 and 11.0 s, the same as the 10.8 s decode alone before the stale-frame fix, and the `--dry-run` plan is unchanged.

Second audit, of the fixes (fmt, check and 364 tests green on a clean checkout): the frame copy added for the stale-frame fix let a frame's HDR side data grow without end when copied again and again (measured 4 to 595 entries over 269 skipped frames on a 10-bit HDR10 clip); the copy now goes through `hwdecode::copy_frame`, which frees the old entries first, with a test that shows both the growth and the fix. The kept copy costs about 50 ms on a 60 s 1080p H.264 clip (about 20%, the audit's measure). The key-leak test reads the key on a run that is checked for output, with every proxy variable pointed at a closed local port so that nothing can leave the machine even if the cost check were lost (with the check removed the test fails locally with exit 1, checked). Two scrub tests that passed without the scrub were removed. After this: 364 tests pass, 2 ignored.

Known limits found by the audit of this branch, not changed: `--sheet auto` falls back to 3×3 without checking that it fits the context, so a video over about 1 h 50 min at `--every-s 5` is sent over the limit and fails at the provider (outside the 20–60 min scope); `decode_frames_at` keeps every sampled frame in memory, about 339 KB each, so a 2-hour video at 5 s needs about 490 MB before JPEG encoding; the quantization recorded in `model.quantization` is taken from the first endpoint of the provider that answered, which matched (`bf16`, Parasail) in the live run but is unchecked for providers with several endpoints.

## Comparative diff, 2026-10-26

Apple M3 MacBook Air, merged into `main` 2026-10-26; release build of `feature/comparative-diff` ([spec](specs/comparative-diff.md), [ADR-014](decisions/ADR-014-compare-a-draft-with-several-outliers-in-a-separate-key.md)). The two real reels are downloads kept outside the repo.

| Check | Result |
|---|---|
| `flags` against the build before the change | 40 draft and outlier pairs (8 drafts × 5 outliers: five fixtures, a 10-bit clip, and both real reels): `flags` identical in all, and `comparison` present after it |
| Milk clip (draft) against the NEET explainer (outlier) | `flags` as before (two dead-air, one static window, the missing payoff cue). `comparison.flags`: one `rhythm_gap` for 5,000–10,000 ms (the outlier's cut at 8,466 ms). No `pause_density` (8 against 28 per minute) |
| NEET explainer (draft) against the milk clip (outlier) | `flags` as before. `comparison.flags`: one `pause_density`, 28.1 against 8.0 per minute. Expected and a known limit: the milk clip's few pauses come from near-silent audio |
| The metrics for the two reels | NEET: 38.5 s, 1 cut, mean shot 19.2 s, longest 30.0 s, 18 pauses over 180 ms (28/min), 28% silent, 144.9 words/min (estimate), −22.4 LUFS. Milk: 15.1 s, 0 cuts, 2 pauses (8/min), 9% silent, 19.9 words/min (estimate), −48.6 LUFS |
| `draft12` against `outlier12` | `flags`: dead air 2.01–2.51 s, two static windows, missing payoff. `comparison.flags`: `rhythm_gap` 0–5 s and `pause_density` 5.0 against 0.0 per minute |
| Argument checks | One to three `--outlier` files run; four exit with code 2 and a message before the draft is decoded; none exits with code 2 |
| `cargo test --workspace` | 144 pass, 2 whisper model tests ignored by default |

The 5 s window, the 1.5 ratio, and the 3 per minute were chosen on these two slow reels and are untested on a fast-cut outlier. The NEET reel's one "cut" is a caption box appearing under a still face (score 0.303 against 0.30), so a cut count inherits the detector's limits; naming such events is `visual-motion`. A final window can be a sliver: the 15.1 s clip has one from 15.0 to 15.1 s. Real reel titles are long, so the text report cuts names to between 8 and 20 characters and wraps the window rows so that no line passes 80 characters (checked with four reels and a 60 s draft); the first version let long titles push the columns together, found on the real files and fixed with tests.

## Quiet audio, 2026-10-26

Apple M3 MacBook Air, merged into `main` 2026-10-26; release build of `feature/quiet-audio` against the build before it, on the same 17 clips ([spec](specs/quiet-audio.md), [ADR-013](decisions/ADR-013-the-silence-floor-follows-a-quiet-recording.md)). The Reels file is a 15 s Instagram download kept outside the repo.

| Check | Result |
|---|---|
| Reels file (720×1280, AAC, mean −48.2 dB, peak −27.3 dB, `integrated_lufs` −48.59) | Before: threshold −40.0, one gap 0–15,092 ms. After: threshold −53.92, three gaps (0–990, 1,370–1,640, 1,760–1,850 ms). A Python model of `silence.rs` with `reelctl`'s mix predicted exactly these gaps. Burst "words" 1 → 5 |
| `quiet12` (tone at −46 dB peak, about −49 dB RMS, 0.5 s mute at 5.0–5.5 s) | Before: one gap over the whole 12 s. After: threshold −61.0 and one gap 5,020–5,500 ms; `diff` flags it as dead air |
| The other 16 clips, including the iPhone recording, every synthetic fixture, and the HEVC resize and depth clips | `edit_dna.json` identical apart from timings and `decode_note`; `timeline_view` PNGs identical |
| 60 s benchmark, `signal_ms`, 10 runs | 238–250 ms, median 242.5 ms (limit 242–262) |
| Whisper tests with the Apex model | 3 passed |
| Unit tests for the rule | The Reels-shaped levels give −54; a −50 tone gives −62; 95 windows at −120 plus 5 at −14 and 92 at −60 plus 8 at −20 stay at −40; 995 at −120 plus 5 at −14 gives −70, with the silence a gap |

The rule changes only recordings whose loudest 1% of windows are quieter than −28 dBFS. Its 12 dB distance was tuned on one recording, so a second quiet recording is the time to retune it.

## Fast decode, 2026-10-26

Same machine, release builds, `feature/fast-decode`, merged into `main` 2026-10-26 ([spec](specs/fast-decode.md), [ADR-012](decisions/ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md)). The direction for this module was quality over time, so nothing skips frames.

| Check | Result |
|---|---|
| Cut detection from keyframes and packet sizes (rejected), on a 60 s corpus of 10 planted hard cuts, 1080×1920, B-frames, keyframe every 24 frames | Today's full decode finds 10 of 10 in all four encodings (x264 and x265, scene-cut keyframes on and off). A keyframe sits at 7 of 10 cuts with scene-cut keyframes on. With them off, the cut frame's packet size ranks 6th to 537th among non-keyframes (x264) and 38th to 354th (x265). The one cut in the iPhone recording is neither a keyframe nor a size outlier (84 KB against a median of 83 KB) |
| Software decode: threads | 7 of 8 cores busy (62 CPU-seconds in 8.9 s); one thread takes 42.7 s. More parallelism cannot reach 500 ms |
| Hardware against software, iPhone recording (87 s, HEVC Main 10, 1080×1920), `reelctl` `signal_ms`, alternating runs | Hardware 5,052, 5,327, 4,021, 4,599 ms; software 8,416, 8,618, 8,478, 8,507 ms. About 43% lower at the median (4,825.5 against 8,492.5 ms). CPU time: 5.7 s against 62 s |
| Found by the audit of this branch, fixed before merge | The media engine's frame copy reused its buffers, so a stream that grew from 320×180 to 640×360 came back cropped at 320×180 with no error and different colors; and full-range 8-bit HEVC (read as limited, where software reads it as `yuvj420p`, full) differed in shot colors (the first shot's mean red was 179.1 against 172.3, mean blue 172.3 against 164.8). Both are now tests (`resize12_hevc`, `textured_cuts12_hevc_full`) that fail without the fix. The copy now starts from an empty frame when the size changes and checks the size afterwards. A second pass found that properties were copied into a reused frame that kept the last frame's side data, so on HDR10 video the count grew by about 2 entries per frame (4, then 29, 55, 80, 106, 131 at frames 1, 13, 25, …; 103 at the end of a 48-frame clip); the old frame's entries are now freed first, and a test with an HDR10 clip fails without it. A change from 8-bit to 10-bit (`depth12_hevc`) was refused by the copy and fell back to software; the copy is now remade in the new pixel format, the media engine decodes it, and the output equals software's. The scene-score grid is now placed by relative position, so streams that grow or shrink are scored across the whole picture (`resize12_hevc`, `shrink12_hevc`) |
| Same file as 8-bit H.264 (1080p reference) | Hardware 11.2 s, software 3.0 s, so H.264 stays in software |
| Hardware has a fixed cost | `draft12_10bit` (12 s, 640×360): 183 ms with hardware, 23 ms in software. About 160 ms of start-up; at 1080p hardware wins from about 3–4 s of video |
| Same results | Corpus HEVC encodings (x265, scene-cut on and off) and the iPhone recording: `decode_note` says the media engine ran, and cuts (frame, time, score), silence gaps, shot colors, loudness, and every `timeline_view` PNG are equal to a software decode. `textured_cuts12_hevc` and `_hevc8` and `draft12_10bit`: scores equal to 0.0001 in the default test suite |
| The old software shortcut | With the loop filter skipped on non-reference frames, software and hardware scene scores differed by 0.006–0.022 on frames just after a cut (HEVC fixtures). With every frame filtered they are equal to four decimals. So the shortcut was removed; it had saved 6% of decode time |
| Exact software decode, existing fixtures | `edit_dna.json` equal to the baseline apart from `decode_note`; PNGs byte-identical. 60 s H.264 benchmark, ten alternating runs: median 243.0 ms before, 242.5 ms after |
| Fallback | A motion JPEG clip (no media engine path): hardware is tried, fails, and the file decodes in software with the same frames; `decode_note` gives the reason. In the default test suite |
| Memory | 50 hardware decodes of one clip in one process: growth of 0.25–3.2 MiB between runs 5 and 50 (four runs) on a 1280×720 clip, against a 20 MiB limit (a deliberate leak of one 1280×720 frame per decode shows 121–122 MiB, and a per-frame leak shows 2,298 MiB, so the test can fail) |
| Exact decode against the 500 ms target | Not met on real footage: the iPhone file takes 2.8–3.7 s per 60 s of video with the media engine (4.0–5.3 s for its 87 s, against 8.5 s in software), and 1080p H.264 is about 2.1 s per 60 s. `planer.md` is unchanged |

## 10-bit video and 44.1 kHz audio, 2026-10-26

Same machine, release builds of `main` (before the merge) and `feature/decode-inputs` on the same inputs ([spec](specs/decode-inputs.md), [ADR-011](decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)). The real iPhone recording is the last two rows. It is the 87 s file from [First real footage](#first-real-footage-2026-10-20), kept outside the repo.

| Check | Result |
|---|---|
| `draft12`, `outlier12`, `noisy12`, `bench60`, `speech12`, `tonespeech` | `edit_dna.json` identical to `main` apart from `signal_ms` and `asr_ms`, including `decode_note`; `timeline_view` PNGs byte-identical |
| `draft12_10bit` (`draft12` as 10-bit HEVC Main 10, tagged BT.2020 and HLG) | `main` stops with `unsupported pixel format`. Branch: same cut frame (180), silence gaps, threshold, loudness, and payoff result as `draft12`; per-shot mean RGB (254.67, 0, 0) and (255, 255, 255) against (255, 0, 0) and (255, 255, 255). `decode_note` adds the 10-bit and HLG sentences |
| 60 s benchmark `signal_ms`, ten alternating runs each | `main` 244–263 ms, median 248; branch 246–254 ms, median 248.5 |
| `textured_cuts12` (added for `fast-decode`: 12 s, 640×360, B-frames, a keyframe every 25 frames, scene-cut detection off) | The software path finds all 3 planted cuts (frames 72, 144, 216), none on a keyframe; a test checks the keyframe positions from the file's own packets |
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

- **`diff` compares with one to three outliers** (since the comparative-diff merge, 2026-10-26), in 5 s windows and per-reel metrics. Dead air and static windows are still properties of the draft alone, and the comparison is only as good as the cut detector: it counts a caption box as a cut and does not name events (`visual-motion`).
- **Static windows count only cuts.** planer.md defines a static window as more than 2.5 s without B-roll, zoom, or a graphic callout. Nothing detects zooms or graphics yet, so a punch-in without a cut is still flagged.
- **The 500 ms target fails on real footage.** Only one real recording has been run so far ([First real footage](#first-real-footage-2026-10-20)). The tests still use only synthetic clips ([ADR-006](decisions/ADR-006-synthetic-offline-fixtures.md)).
- **Silence on noisy recordings** (closed 2026-10-25, merged into `main`). The fixed −40 dBFS threshold found no pause of 0.3 s or more on the real recording; the per-recording threshold finds 24. Its constants were tuned on that one recording, so music beds and loud backgrounds are unmeasured. Very quiet recordings are handled since 2026-10-26 ([ADR-013](decisions/ADR-013-the-silence-floor-follows-a-quiet-recording.md)), tested on one downloaded reel.
- **10-bit video** (closed 2026-10-26, merged into `main`). Decoding stopped with `unsupported pixel format` on anything other than 8-bit 4:2:0, which ruled out iPhone HDR recordings until they were converted. 10-bit 4:2:0 is now read from the top 8 bits of each sample ([ADR-011](decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)), tested on a synthetic clip and on the real recording (see [10-bit video and 44.1 kHz audio](#10-bit-video-and-441-khz-audio-2026-10-26)); it decodes 2.8 times slower than 8-bit H.264. 4:2:2, 4:4:4, and 12-bit video still stop with an error, which now names the format (only 4:2:2 is covered by a test).
- **`--draft` takes an MP4.** planer.md specifies a `.otio` timeline; OpenTimelineIO is P2 work.

## Known issues

- In a `timeline_view` PNG, the thumbnails and the waveform use different time scales. The 8 thumbnails cover about 0.27 s at 30 fps; the waveform and word ticks cover 1.5 s either side of the cut, across the same width.
- The JSON value for TransNetV2 is `"trans_net_v2"`, but `CutDetectorKind::as_str()` returns `"transnet_v2"`. Nothing emits either until TransNetV2 is linked.
- Colors are converted from YUV with BT.601 coefficients, while most HD video uses BT.709, so absolute RGB values are slightly off.
- Audio sampled above 48 kHz (88.2 or 96 kHz) can alias, because the resampler is linear interpolation with no low-pass filter. Unmeasured; the rates seen so far (44.1 and 48 kHz) are not affected.
- The extra detection on the frame after a cut: in an earlier version of `textured_cuts12_hevc` (sources that change drastically between frames, since replaced), both decoders reported a second cut one frame after a real one. The detector, not the decoder, does that, so it is a property of `scene_score` on footage whose first frame in a shot is coded coarsely. Not investigated further.
- Range follows one rule for every format: a frame is full range if it is `yuvj420p` or its metadata says full range, and limited otherwise (also when the metadata says nothing). Before 2026-10-26 only `yuvj420p` and 10-bit frames followed it, so a software `yuv420p` frame flagged full range, as VP9 and AV1 files can be, came out with its colors stretched as if limited. The media engine returns every 8-bit HEVC frame as `nv12`, which has no full-range variant, so it relies on the metadata ([ADR-012](decisions/ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md)).
- A stream that changes frame size part-way decodes in both decoders. The scene-score grid keeps the first frame's number of points and places them by position in the picture, so it spans every frame whatever its size and compares a resized frame with the last one by content. (Before 2026-10-26 a shrinking stream stopped with `luma sample walked past the plane`, and a growing one was scored over only the top-left part of the picture.) Colors and thumbnails use each frame's own size. A resolution change without a change of picture scores low; a splice to different content scores as a cut. A change from 8-bit to 10-bit is decoded on the media engine too, with the same output.
- HDR video (HLG or PQ transfer, such as iPhone recordings) is not tone-mapped, so its per-shot colors and thumbnails are flatter and less saturated than the picture a viewer sees. `decode_note` says so ([ADR-011](decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)).
- whisper can silently drop up to a 30 s window of speech. Since 2026-10-26 such stretches are found and transcribed again, which recovered every measured loss ([Whisper coverage](#whisper-coverage-2026-10-25)). A stretch can still stay empty, and `asr_note` then lists it. Which of whisper.cpp's two skip rules fired in each case is not known.
- With the Apex model, 29 s of digital silence before English speech (`say`) produced 6 invented words at 0.1 s and a garbled first sentence. `base.en` did not do this. Found 2026-10-25; not addressed.
- Thresholds read from FFmpeg tools are not comparable with `reelctl`'s: FFmpeg's `-ac 1` downmix reads about 3 dB louder than `reelctl`'s channel average, and `silencedetect` compares sample peaks rather than 10 ms mean-square levels.
- The quality interview in [CONSTRAINTS.md](../CONSTRAINTS.md) has not been run, so only the floor rules plus `cargo check` and `cargo test` are enforced.

## Next steps

Done: real word timing with whisper.cpp ([spec](specs/whisper-asr.md)), merged 2026-10-21. A possible follow-up, not yet specified: a `--whisper-language` flag, which would skip language detection and save about 6 s per run with the Apex model.

Then, to finish P0 against its own success metric before starting P1. The [P0 capability map](specs/p0-capability-map.md) (approved 2026-10-25) orders these into modules: `whisper-coverage`, `decode-inputs`, `fast-decode`, `comparative-diff`, `visual-motion`. The list below predates it:

1. Find a decode strategy that meets 500 ms on real footage, or revise the target.
2. Read 10-bit video, such as iPhone HDR recordings. Done, merged 2026-10-26 ([spec](specs/decode-inputs.md)): 10-bit 4:2:0 is read, audio is resampled as one stream, and the real recording decompiles.
3. Detect silence relative to each recording's noise floor instead of at a fixed −40 dBFS. Done ([spec](specs/relative-silence.md)), merged 2026-10-25.
4. Make `diff` comparative: accept one to three outliers and compare cut rhythm second by second. Done, merged 2026-10-26 ([spec](specs/comparative-diff.md)); thresholds untested on a fast-cut outlier.
5. Link TransNetV2. It needs ONNX weights, a download that needs an explicit go-ahead.
