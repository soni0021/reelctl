# Tasks: whisper coverage

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/whisper-coverage.md](../docs/specs/whisper-coverage.md). Tick each box in the commit that completes it.

## Task 1: Holes and merging

**Description:** New `crates/decompile/src/coverage.rs` with pure functions: `find_holes(starts, gaps, duration_ms)`, `merge(first, reruns)`, and dropping words that start at or after the end of the audio. Nothing calls them yet.

**Acceptance criteria:**
- [x] A 25 s stretch with sound and no word start is a hole; a 4 s stretch that is 3 s of silence gap is not; stretches before the first word and after the last count
- [x] Re-run words are shifted by the hole's start, kept only if they start inside the hole, replace the first pass's words there (an empty re-run changes nothing), and the merged list stays sorted
- [x] Words that start at or after the end of the audio are dropped; words inside silence gaps are kept

**Verification:**
- [x] `cargo test -p decompile coverage` (red first, then green: 7 of 7). Nothing calls the module yet, so it warns as dead code until Task 2

**Dependencies:** None

**Files likely touched:** `crates/decompile/src/coverage.rs` (new), `crates/decompile/src/lib.rs`

**Estimated scope:** Small

## Task 2: Re-transcribe holes in-process

**Description:** Refactor `whisper.rs` around a `Transcriber` that loads the model once and runs padded passes over slices of the 16 kHz audio. The first pass covers the whole audio. Holes found in its output are re-run, merged, and word ends computed once. `asr_note` reports the re-runs, the words they added, and any holes that stayed empty.

**Acceptance criteria:**
- [x] Before the change, save `words` from `speech12` (both models) and the five clean cuts; after it, they are identical (spec criterion 4). They are, once the first pass is left unpadded; padding it changed 4 of 5 cuts
- [x] iPhone clip, first 60 s with copied audio: at least 180 words, with words in 9.9–15.6 and 16.0–30.9 s (criterion 2): 183 words, 10 and 52
- [x] No word starts at or after the end of the audio (criterion 3)
- [x] A synthetic clip that forces a hole is tried; if one works, it becomes an ignored model test; if not, the attempts are recorded in the spec. Neither attempt forced a speech loss; the tone-then-speech clip became an ignored test of the hole path
- [x] Docs: `docs/edit-dna.md` (`words`, `asr_note`), CHANGELOG, ADR-009

**Verification:**
- [x] `cargo test` (52 pass, 1 ignored); the whisper model tests with both models (3 of 3 each)
- [x] Release-build runs on the iPhone clip and the saved cuts

**Dependencies:** Task 1

**Files likely touched:** `crates/decompile/src/whisper.rs`, `crates/decompile/src/asr.rs`, `crates/reelctl/tests/whisper_fixture.rs` (if a forcing clip is found), plus the docs above

**Estimated scope:** Medium

## Checkpoint A

- [x] `cargo test` passes
- [x] NEET video, full: at least 80 words before 30.44 s, words in each of the three short holes, and extra `asr_ms` under 10% of 401.6 s (criteria 1 and 5). Criterion 1 passed: 86 words before 30.44 s, and 27, 18, and 17 in the short holes. Criterion 5 was not measurable here: total `asr_ms` was 763.0 s under a load average near 6, which two separate runs cannot attribute; `asr_first_ms` and `asr_second_ms` were added and Task 4 measured it within one run: re-runs 44.1 s, 8.4% of the 528.0 s first pass

## Task 3: Run whisper in a child process

**Description:** A hidden `reelctl whisper-worker` subcommand runs whisper passes for the parent. `decompile` gets `WhisperRunner::{InProcess, Worker}`, and `reelctl` uses `Worker` with its own executable. Audio goes through a temporary file, requests through stdin as JSON, and results come back as one JSON line per pass. If the worker dies from a signal, the parent re-runs the unfinished passes with DTW off and says so in `asr_note`.

**Acceptance criteria:**
- [x] A unit test with a fake worker that aborts (`/bin/sh -c 'kill -ABRT $$'`) gets the abort reported and the retry attempted; when the retry also fails, the error names the abort and exit code 1 follows. A mutation that keeps DTW on the retry makes the retry test fail
- [x] With the real worker, `speech12` gives the same `words` as in-process with both models, and `--help` does not list the worker subcommand
- [x] The temporary audio file is removed after the run, on success and on failure
- [x] Docs: `docs/cli.md` (worker process, temporary file location), CHANGELOG

**Verification:**
- [x] `cargo test` (57 pass, 2 ignored); the whisper model tests with both models; the iPhone clip gives the same 183 words through the worker as in-process

**Dependencies:** Task 2

**Files likely touched:** `crates/decompile/src/whisper.rs`, `crates/decompile/src/worker.rs` (new), `crates/decompile/src/pipeline.rs`, `crates/reelctl/src/main.rs`, `docs/cli.md`

**Estimated scope:** Medium

## Task 4: Measure and close out

**Description:** Run the release build on the NEET video and the iPhone clip through the worker, and record the results.

**Acceptance criteria:**
- [x] `docs/status.md`: words recovered per hole, `asr_ms` before and after, the worker's start-up cost, and the known issue updated. Through the worker the NEET video gave the same 4,040 words as in-process
- [x] Spec status updated; these boxes ticked

**Verification:**
- [x] Release-build runs; the docs link check passes

**Dependencies:** Task 3

**Files likely touched:** `docs/status.md`, `docs/specs/whisper-coverage.md`, `tasks/plan.md`, `tasks/todo.md`

**Estimated scope:** Small

## Checkpoint B: Done

- [x] All seven spec success criteria verified (criterion 5 on the within-run measure)
- [x] `cargo test` passes, and the whisper model test passes with both models
- [ ] Reviewed with the user, then `feature/whisper-coverage` merged into `main`, and this plan moved to `tasks/archive/whisper-coverage/`
