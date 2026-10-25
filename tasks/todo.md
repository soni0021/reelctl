# Tasks: whisper coverage

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/whisper-coverage.md](../docs/specs/whisper-coverage.md). Tick each box in the commit that completes it.

## Task 1: Holes and merging

**Description:** New `crates/decompile/src/coverage.rs` with pure functions: `find_holes(starts, gaps, duration_ms)`, `merge(first, reruns)`, and dropping words that start at or after the end of the audio. Nothing calls them yet.

**Acceptance criteria:**
- [ ] A 25 s stretch with sound and no word start is a hole; a 4 s stretch that is 3 s of silence gap is not; stretches before the first word and after the last count
- [ ] Re-run words are shifted by the hole's start, kept only if they start inside the hole, and the merged list stays sorted
- [ ] Words that start at or after the end of the audio are dropped; words inside silence gaps are kept

**Verification:**
- [ ] `cargo test -p decompile coverage` (red first, then green)

**Dependencies:** None

**Files likely touched:** `crates/decompile/src/coverage.rs` (new), `crates/decompile/src/lib.rs`

**Estimated scope:** Small

## Task 2: Re-transcribe holes in-process

**Description:** Refactor `whisper.rs` around a `Transcriber` that loads the model once and runs padded passes over slices of the 16 kHz audio. The first pass covers the whole audio. Holes found in its output are re-run, merged, and word ends computed once. `asr_note` reports the re-runs, the words they added, and any holes that stayed empty.

**Acceptance criteria:**
- [ ] Before the change, save `words` from `speech12` (both models) and the five clean cuts; after it, they are identical (spec criterion 4)
- [ ] iPhone clip, first 60 s with copied audio: at least 180 words, with words in 9.9–15.6 and 16.0–30.9 s (criterion 2)
- [ ] No word starts at or after the end of the audio (criterion 3)
- [ ] A synthetic clip that forces a hole is tried; if one works, it becomes an ignored model test; if not, the attempts are recorded in the spec
- [ ] Docs: `docs/edit-dna.md` (`words`, `asr_note`), CHANGELOG, ADR-009

**Verification:**
- [ ] `cargo test`; the whisper model test with both models
- [ ] Release-build runs on the iPhone clip and the saved cuts

**Dependencies:** Task 1

**Files likely touched:** `crates/decompile/src/whisper.rs`, `crates/decompile/src/asr.rs`, `crates/reelctl/tests/whisper_fixture.rs` (if a forcing clip is found), plus the docs above

**Estimated scope:** Medium

## Checkpoint A

- [ ] `cargo test` passes
- [ ] NEET video, full: at least 80 words before 30.44 s, words in each of the three short holes, and extra `asr_ms` under 10% of 401.6 s (criteria 1 and 5)

## Task 3: Run whisper in a child process

**Description:** A hidden `reelctl whisper-worker` subcommand runs whisper passes for the parent. `decompile` gets `WhisperRunner::{InProcess, Worker}`, and `reelctl` uses `Worker` with its own executable. Audio goes through a temporary file, requests through stdin as JSON, and results come back as one JSON line per pass. If the worker dies from a signal, the parent re-runs the unfinished passes with DTW off and says so in `asr_note`.

**Acceptance criteria:**
- [ ] A unit test with a fake worker that aborts (`/bin/sh -c 'kill -ABRT $$'`) gets the abort reported and the retry attempted; when the retry also fails, the error names the abort and exit code 1 follows
- [ ] With the real worker, `speech12` gives the same `words` as in-process with both models, and `--help` does not list the worker subcommand
- [ ] The temporary audio file is removed after the run, on success and on failure
- [ ] Docs: `docs/cli.md` (worker process, temporary file location), CHANGELOG

**Verification:**
- [ ] `cargo test`; the whisper model test with both models

**Dependencies:** Task 2

**Files likely touched:** `crates/decompile/src/whisper.rs`, `crates/decompile/src/worker.rs` (new), `crates/decompile/src/pipeline.rs`, `crates/reelctl/src/main.rs`, `docs/cli.md`

**Estimated scope:** Medium

## Task 4: Measure and close out

**Description:** Run the release build on the NEET video and the iPhone clip through the worker, and record the results.

**Acceptance criteria:**
- [ ] `docs/status.md`: words recovered per hole, `asr_ms` before and after, the worker's start-up cost, and the known issue updated
- [ ] Spec status updated; these boxes ticked

**Verification:**
- [ ] Release-build runs; the docs link check passes

**Dependencies:** Task 3

**Files likely touched:** `docs/status.md`, `docs/specs/whisper-coverage.md`, `tasks/plan.md`, `tasks/todo.md`

**Estimated scope:** Small

## Checkpoint B: Done

- [ ] All seven spec success criteria verified
- [ ] `cargo test` passes, and the whisper model test passes with both models
- [ ] Reviewed with the user, then `feature/whisper-coverage` merged into `main`, and this plan moved to `tasks/archive/whisper-coverage/`
