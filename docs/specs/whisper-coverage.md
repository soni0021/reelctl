# Spec: whisper coverage

Status: approved 2026-10-25, with the recommended answer to each open question (see Resolved Questions). Plan: [tasks/plan.md](../../tasks/plan.md). Module `whisper-coverage` in the [P0 capability map](p0-capability-map.md). Branch: `feature/whisper-coverage`.

## Objective

With a whisper model, `reelctl decompile` can silently leave out stretches of speech, and in rare cases whisper.cpp can kill the whole process. Both break the words and speaking rate that `diff` and agents rely on. A Reel's hook is in its first seconds, and that is exactly where one of the losses happened.

Observed on 2026-10-25 with the Apex model ([status.md](../status.md#long-form-youtube-video-2026-10-25)):

- **The NEET video (18 min):** no words before 30.44 s, although only 4.7 s of that stretch is silent. Shorter stretches were also lost at 355.8–360.6, 505.5–510.7, and 985.9–990.8 s; re-transcribed on their own they give 25, 18, and 17 words.
- **The first 60 s of the iPhone recording, with copied audio:** no words in 9.9–15.6 and 16.0–30.9 s. Re-transcribed on their own they give 13 and 49 words, which brings the clip to 187 words; the earlier cut of the same recording gave 183.
- **The abort:** a probe with the same whisper settings, run on the NEET opening resampled by FFmpeg, exited with SIGABRT from `WHISPER_ASSERT(filter_width < a->ne[2])` in whisper.cpp's DTW median filter. `reelctl` discards whisper.cpp's logs, so it would exit with no message.

Why, from the whisper.cpp 1.8.3 source and upstream history:

- whisper.cpp decodes 30 s windows in turn. A window judged to contain no speech is dropped (`is_no_speech`, `whisper.cpp` around line 7585). A window whose output ends with a single timestamp makes it skip the rest of that window ("single timestamp ending - skip entire chunk", line 7729, from PR #2629). Both drops end at a window boundary, as both observed losses do. Which of the two fired in each case is not known, because the logs are discarded.
- The abort happens when, with DTW on, "a 30-second window advances only a few frames", so the window has fewer audio tokens than the filter's width of 7 ([PR #4096](https://github.com/ggml-org/whisper.cpp/pull/4096)). Upstream fixed it on 2026-10-19 in commit `a46fadb36a` by narrowing the filter instead of asserting. That is after the latest release (v1.9.5, 2026-10-06), and whisper-rs 0.16.0, the newest release and the one we use, bundles whisper.cpp 1.8.3.

The losses depend on the whole input in ways small changes flip. The same NEET opening transcribed fully as a 30 s or 60 s cut, re-encoded or copied, gave 0 words as a 30.4 s cut, and gave 92 words once 1 s of silence was appended. Turning off the no-speech check (`no_speech_thold` 1.0) made no difference on that cut. So the fix does not try to tune whisper. It checks whisper's output against the audio and transcribes again where they disagree.

Behavior:

- **Find holes.** After the first pass, a hole is a stretch between consecutive word starts, or between the start or end of the audio and the nearest word, that lasts more than 3 s and has at least 1.5 s outside `silence_gaps`. On the seven transcripts measured, this flags every known loss and nothing in the five runs that had none.
- **Fill holes.** Each hole's audio, plus 1 s of silence after it, is transcribed again with the same loaded model and settings. Its words are shifted by the hole's start and kept if they start inside the hole. One round only: a hole that comes back empty stays empty.
- **Pad the first pass** with 1 s of silence after the audio, as cheap insurance against a short last window.
- **Drop words from the padding.** A word that starts at or after the end of the real audio is removed. Apex wrote "nan" for 20 s of digital silence and for 20 s of white noise, so padding can produce invented words, and they can only start in the padding.
- **No general silence filter.** On the NEET video, 3 of 3,894 real words have DTW starts 110–200 ms inside a silence gap ("let **say.**" at the start of a 1.41 s gap, "death **se.**", "saari **chiz** se"), so dropping words that start inside gaps would delete real speech.
- **Survive an abort.** whisper runs in a child `reelctl` process (Resolved Question 1).
- `asr_note` says how many holes were re-transcribed, how many words that added, and which holes stayed empty, with their times.
- Without holes the output is unchanged, because whisper with greedy sampling is deterministic. Re-transcription counts toward `asr_ms` and stays outside `signal_ms`.

Out of scope: voice activity detection models, changing whisper's thresholds, beam search, chunking the whole recording in advance, the `--whisper-language` flag, and the 44.1 kHz resampling drift (module `decode-inputs`).

## Tech Stack

No new dependencies. `whisper-rs` 0.16.0 (whisper.cpp 1.8.3), as now. Re-transcription reuses the loaded `WhisperContext` and runs `WhisperState::full` again on a slice of the 16 kHz audio.

## Commands

```bash
cargo check
cargo test
REELCTL_WHISPER_MODEL=~/models/whisper/ggml-apex-hinglish-q8_0.bin \
  cargo test -p reelctl --test whisper_fixture -- --include-ignored
cargo build --release
target/release/reelctl decompile --input video.mp4 --out video.edit_dna.json \
  --whisper-model ~/models/whisper/ggml-apex-hinglish-q8_0.bin
```

## Project Structure

| Path | Change |
|---|---|
| `crates/decompile/src/whisper.rs` | Run passes on one loaded model; padding; hole re-runs; merging |
| `crates/decompile/src/coverage.rs` (new) | Pure functions: find holes from words, gaps, and duration; merge re-run words; drop words from the padding |
| `crates/decompile/src/asr.rs` | `asr_note` text |
| `crates/reelctl/src/main.rs` | A hidden worker subcommand that runs whisper passes for the parent process |
| `crates/reelctl/tests/whisper_fixture.rs` | A model test where a hole is forced, if one can be made synthetically (see Testing Strategy) |
| Docs | `docs/edit-dna.md` (`words`, `asr_note`), `docs/cli.md`, `docs/status.md`, CHANGELOG, ADR-009 |

## Code Style

```rust
/// A stretch of sound with no word start: more than `MIN_HOLE_MS` long and at
/// least `MIN_SOUND_MS` of it outside the silence gaps.
pub(crate) fn find_holes(words: &[Word], gaps: &[SilenceGap], duration_ms: u64) -> Vec<Hole> {
    // ...
}
```

Free functions over plain data, named constants with the evidence in the module comment, as in `silence.rs`.

## Testing Strategy

Test-first.

- **Unit tests, no model, in `coverage.rs`:**
  - Holes: a 25 s stretch with sound and no words is a hole; a 4 s stretch that is mostly a silence gap is not; leading and trailing stretches count.
  - Merging: re-run words are shifted and kept only inside the hole, and the result stays sorted.
  - Words that start at or after the end of the real audio are dropped; words inside silence gaps are kept.
- **Model test, ignored by default:** `speech12` keeps passing unchanged with both development models, so the no-hole path changes nothing.
- **A forced hole:** a synthetic clip that reliably makes whisper drop a window is not known. The plan's first task tries a few, such as speech after a long pure tone. If none works, the hole path is covered by the unit tests plus the manual runs below, and the spec records that.
- **Manual, real footage outside the repo:** the NEET video and the iPhone clip, before and after.

## Boundaries

- **Always:** keep the output identical when there are no holes; keep re-transcription in `asr_ms`; update the docs in the same commit.
- **Ask first:** adding fields to `edit_dna.json`; changing whisper's thresholds or sampling; upgrading or patching `whisper-rs`.
- **Never:** commit real recordings or their transcripts; use the network in tests.

## Success Criteria

1. NEET video: at least 80 words before 30.44 s (a 30 s cut alone gave 84), and words in each of the three short holes.
2. iPhone clip, first 60 s with copied audio: at least 180 words, and words in both 9.9–15.6 and 16.0–30.9 s.
3. No word starts at or after the end of the audio, on either recording or `speech12`.
4. Runs without holes give identical `words`: `speech12` with both models, and the five NEET and iPhone cuts that had no holes.
5. Extra `asr_ms` for the NEET video is under 10% of its first pass (401.6 s).
6. A whisper.cpp abort in the child process does not end `reelctl`: the pass is retried without DTW, `asr_note` says so, and a test covers it.
7. `cargo test` passes, and the docs listed under Project Structure are updated.

## Resolved Questions

Answered 2026-10-25 with the recommended options:

1. **The DTW abort.** It ends the whole process in whisper.cpp 1.8.3, and no released whisper-rs has the fix.
   - **A (chosen):** run whisper in a child `reelctl` process, so an abort becomes an error the parent handles. The parent re-runs the failed pass without DTW, falling back to segment timestamps, and says so in `asr_note`. Costs: the 16 kHz audio goes to the child through a temporary file, and the model loads once per child (about 1 s for Apex).
   - **B:** accept the abort for now, document it, and upgrade when whisper-rs ships a whisper.cpp with PR #4096.
   - **C:** point `whisper-rs-sys` at a whisper.cpp revision that has the fix, with a Cargo `[patch]` to a git dependency. Fixes it at the source, but leaves the released crates.
2. **Where to report re-transcribed holes.** In `asr_note` text, with no schema change. The alternative is a new `asr_reruns` field and schema version 3, which tools could read.
3. **Holes over music or noise.** A hole is any stretch with sound and no words, so a music bed or a noisy stretch also triggers a re-run, and whisper may invent words there. Accepted; every re-run is reported in `asr_note` so it can be checked. The alternative is to skip holes whose level stays near-constant, which needs its own measurements.
