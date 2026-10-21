# Tasks: Real word timing with whisper.cpp

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/whisper-asr.md](../docs/specs/whisper-asr.md). Tick each box in the commit that completes it.

## Task 1: `whisper-rs` builds and links

**Description:** Add `whisper-rs` 0.16.0 as a dependency of `decompile` and prove that whisper.cpp compiles and links on this Mac before anything depends on it.

**Acceptance criteria:**
- [x] `cargo build --release` compiles whisper.cpp 1.8.3 with CMake 4.4.4
- [x] A unit test calls into whisper.cpp (`whisper_rs::get_whisper_version()`) and gets `1.8.3`
- [x] README prerequisites and CHANGELOG say that building needs CMake

**Verification:**
- [x] `cargo check`, then `cargo test` (18 of 18 pass)
- [x] First-build time noted for Task 6: 41 s for the test profile, 24 s for release; no extra link flags were needed

**Dependencies:** None

**Files likely touched:** `Cargo.toml`, `crates/decompile/Cargo.toml`, `crates/decompile/src/whisper.rs` (new), `crates/decompile/src/lib.rs`, `README.md`, `CHANGELOG.md`, and `.cargo/config.toml` only if linking needs it

**Estimated scope:** Small

## Task 2: Resample 48 kHz to 16 kHz

**Description:** A pure function that converts the decoded mono PCM to whisper's input rate: a windowed-sinc low-pass filter, then every third sample. Other input rates return an error.

**Acceptance criteria:**
- [x] 48,000 samples in, 16,000 out
- [x] A 1 kHz tone keeps its RMS level within 5%; a 12 kHz tone drops by at least 30 dB
- [x] Input that is not 48 kHz returns an error

**Verification:**
- [x] `cargo test -p decompile resample` (4 of 4 pass). The function shows `dead_code` warnings until Task 4 calls it.

**Dependencies:** None

**Files likely touched:** `crates/decompile/src/resample.rs` (new), `crates/decompile/src/lib.rs`

**Estimated scope:** Small

## Task 3: whisper module

**Description:** `words_from_segments` (pure) and `whisper::words(model, pcm, sample_rate)`. The latter checks the path, installs the logging hooks, loads the model, and sets the spec's parameters: greedy sampling, token timestamps, `max_len` 1, split on word, English unless the model is multilingual, non-speech tokens suppressed, and no printing. It then runs the model and maps the segments.

**Acceptance criteria:**
- [x] `words_from_segments` converts centiseconds to milliseconds, trims text, and drops empty, punctuation-only, and bracketed segments such as `[BLANK_AUDIO]`
- [x] A missing model path returns `DecompileError::Whisper` naming the path
- [x] A manual run on a `say` clip returns the spoken words: all 13 words of "Cut the dead air. Show the proof before the viewer scrolls away." Timing problem found, see Risks in [plan.md](plan.md)

**Verification:**
- [x] `cargo test -p decompile whisper` (5 of 5 pass)
- [x] Manual run result recorded in the commit message

**Dependencies:** Tasks 1 and 2

**Files likely touched:** `crates/decompile/src/whisper.rs`, `crates/decompile/src/error.rs`

**Estimated scope:** Small to medium

## Checkpoint A

- [x] `cargo test` passes (26 of 26)
- [x] `cargo build --release` is clean

## Task 4: Use whisper for words when a model is given

**Description:** `asr::transcribe` sends audio to whisper (DTW word starts, ends at the next word or silence gap, preset from the model header, per the 2026-10-21 spec revision) when a model is given and to the burst estimator (with the new note) when not, and returns a `Result` that the pipeline propagates. `p0_fixture.rs` checks the new note and spawns `reelctl` without `REELCTL_*` variables. The user-facing docs and ADR-007 land in the same commit.

**Acceptance criteria:**
- [x] `--whisper-model <model>` gives `asr_backend: "whisper_cpp"` and an `asr_note` naming the model file and whisper.cpp version
- [x] Without a model, output is unchanged except `asr_note` ("No whisper model was given ...")
- [x] `--whisper-model /missing.bin` exits with code 1 and an error naming the path
- [x] Docs updated: `docs/cli.md`, `docs/edit-dna.md`, README usage, CHANGELOG, AGENTS.md P0 gaps, and ADR-007, with ADR-003 marked superseded

**Verification:**
- [x] `cargo test` (30 of 30 pass)
- [x] Manual runs of the three cases above, on the first 60 s of the real recording: Apex gave 183 words with `asr_backend` `whisper_cpp`; no model gave the new note; `/missing.bin` exited with 1 and `error: decompile: whisper: model not found at /missing.bin`

**Dependencies:** Task 3

**Files likely touched:** `crates/decompile/src/asr.rs`, `crates/decompile/src/pipeline.rs`, `crates/reelctl/tests/p0_fixture.rs`, plus the docs above

**Estimated scope:** Medium

## Task 5: `speech12` fixture and end-to-end tests

**Description:** `fixtures/generate.sh speech12` builds 12 s of solid color with `say -v Samantha` speech and a planted 1-second pause. `crates/reelctl/tests/whisper_fixture.rs` holds two tests: (a) a missing model path exits with code 1, which runs in the default suite; and (b) the model test for spec success criteria 1 and 2, ignored by default.

**Acceptance criteria:**
- [ ] `bash fixtures/generate.sh speech12 out.mp4` works offline
- [ ] Default `cargo test` runs (a) and it passes; (b) is listed as ignored with its reason
- [ ] With `REELCTL_WHISPER_MODEL` set and `--include-ignored`, (b) passes: the six key words in order, times increasing and inside the clip, and nothing more than 200 ms inside the pause
- [ ] README commands and the AGENTS.md Verify section document the model test

**Verification:**
- [ ] `cargo test`
- [ ] `REELCTL_WHISPER_MODEL=~/models/whisper/ggml-apex-hinglish-q8_0.bin cargo test -- --include-ignored`, and the same with `ggml-base.en.bin`

**Dependencies:** Task 4

**Files likely touched:** `fixtures/generate.sh`, `crates/reelctl/tests/whisper_fixture.rs` (new), `README.md`, `AGENTS.md`

**Estimated scope:** Small to medium

## Task 6: Measure and close out

**Description:** Choose the thread count by comparing 4 and 8 threads on a 60 s speech clip. Record `asr_ms` for 12 s and 60 s speech, the first-build time, and the 60 s benchmark's `signal_ms` in `docs/status.md`. Mark the spec implemented.

**Acceptance criteria:**
- [ ] `docs/status.md` shows the whisper gap closed, with measured numbers and the date
- [ ] The benchmark's `signal_ms` stays within its earlier range (284–316 ms)
- [ ] The spec's status is updated and these boxes are ticked

**Verification:**
- [ ] Release-build runs; the docs link check passes

**Dependencies:** Task 5

**Files likely touched:** `crates/decompile/src/whisper.rs` (only if the thread count changes), `docs/status.md`, `docs/specs/whisper-asr.md`, `tasks/todo.md`

**Estimated scope:** Small

## Checkpoint B: Done

- [ ] All seven spec success criteria verified
- [ ] `cargo test` passes, and the model test passes
- [ ] Reviewed with the user, then `feature/whisper-asr` merged into `main`
