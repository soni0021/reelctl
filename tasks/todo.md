# Tasks: Silence relative to the noise floor

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/relative-silence.md](../docs/specs/relative-silence.md). Tick each box in the commit that completes it.

## Task 1: Threshold from window levels

**Description:** Pure functions in `silence.rs`: `window_levels(pcm, sample_rate)` (one dB value per 10 ms window, using the existing `window_db`), `percentile(levels, p)` (nearest rank), and `threshold_db(levels)` = `max(−40, min(p10 + 6, p90 − 12))`. Nothing calls them yet.

**Acceptance criteria:**
- [x] Digital silence plus a −14 dB level gives −40
- [x] A −40 dB floor under a −14 dB level gives −34 (`p10 + 6`, below the cap of −26); a constant −21 dB level gives −33 (`p90 − 12`, below `p10 + 6` = −15)
- [x] An empty list gives −40, and `window_levels` returns `ceil(samples / 480)` values at 48 kHz

**Verification:**
- [x] `cargo test -p decompile silence` (new tests red first, then green: 11 of 11). The new functions show `dead_code` warnings until Task 2 calls them

**Dependencies:** None

**Files likely touched:** `crates/decompile/src/silence.rs`

**Estimated scope:** Small

## Task 2: Gaps and bursts use the threshold

**Description:** `silence::analyze` returns gaps, the threshold, and the noise floor (p10). Gap detection takes the threshold and ends a gap only after 30 ms of sound; the 80 ms minimum stays. The pipeline calls `analyze` in the silence stage and passes the result to `asr::transcribe`. The burst estimator takes the threshold instead of its own −40. Docs that describe silence detection change in the same commit.

**Acceptance criteria:**
- [ ] A −38 dB noise pause inside a −20 dB tone is a gap at the computed threshold and not at −40; 10 and 20 ms sounds inside a gap do not split it, 30 ms does; a gap still open at the end of the audio closes there
- [ ] `draft12`, `outlier12`, `bench60`, and `speech12` give exactly the `silence_gaps` they gave on `main` (baseline saved before the change)
- [ ] Docs: `docs/edit-dna.md` (`silence_gaps` and burst text), AGENTS.md (the −40 dBFS workspace fact), CHANGELOG, and ADR-008, with a pointer to it added to ADR-007's consequences

**Verification:**
- [ ] `cargo test`
- [ ] Release build: baseline comparison for the four clips; on the first 60 s of the real recording, at least 20 gaps of 300 ms or more and no Apex word start more than 100 ms inside a gap

**Dependencies:** Task 1

**Files likely touched:** `crates/decompile/src/silence.rs`, `crates/decompile/src/asr.rs`, `crates/decompile/src/pipeline.rs`, plus the docs above

**Estimated scope:** Medium

## Checkpoint A

- [ ] `cargo test` passes
- [ ] The baseline comparison and the real-clip check in Task 2 pass

## Task 3: Record the threshold and noise floor in `edit_dna.json`

**Description:** Add `silence_threshold_db` (a file without it reads as −40) and `noise_floor_db` (`null` with no audio or in older files) to `EditDna`, set `SCHEMA_VERSION` to 2, fill both from `SilenceAnalysis`, and regenerate the schema.

**Acceptance criteria:**
- [ ] A version-1 JSON without the fields deserializes, with −40 and `null`, and `reelctl diff` accepts it as `--outlier`
- [ ] `draft12` reports `silence_threshold_db` −33 and a `noise_floor_db` of about −21.2; a clip with no audio reports −40 and `null`
- [ ] Docs: `docs/edit-dna.md` (both fields and `schema_version` 2), schema regenerated, CHANGELOG

**Verification:**
- [ ] `cargo test`; `cargo run -p edit-dna --example schema` leaves no further diff

**Dependencies:** Task 2

**Files likely touched:** `crates/edit-dna/src/lib.rs`, `crates/edit-dna/schema/edit_dna.schema.json`, `crates/decompile/src/pipeline.rs`, `docs/edit-dna.md`, `CHANGELOG.md`

**Estimated scope:** Medium

## Task 4: `noisy12` fixture and end-to-end test

**Description:** `fixtures/generate.sh noisy12` makes `draft12`'s video and tone, with the tone muted from 2.0 to 2.5 s, plus white noise from `anoisesrc` with a fixed seed throughout. A new test in `p0_fixture.rs` decompiles it and runs `diff` on it.

**Acceptance criteria:**
- [ ] Every 10 ms window in the pause is at least 1 dB above −40 and at least 3 dB below the computed threshold (check while choosing the amplitude)
- [ ] The only gap of 180 ms or more is the planted pause (±20 ms), and `diff` flags it as `dead_air`
- [ ] README commands list `noisy12`

**Verification:**
- [ ] `cargo test`, which includes the new test, on a machine without `say`

**Dependencies:** Task 2, plus Task 3 if the test reads the new fields

**Files likely touched:** `fixtures/generate.sh`, `crates/reelctl/tests/p0_fixture.rs`, `README.md`

**Estimated scope:** Small

## Task 5: Measure and close out

**Description:** Run the release build on the real recording and the benchmark, run the whisper model test with both models, and record the results. Mark the spec implemented.

**Acceptance criteria:**
- [ ] `docs/status.md`: the threshold, noise floor, and gap counts on the real clip; word ends that run more than 200 ms into a pause, against 9 of 183 before (spec criterion 4); benchmark `signal_ms` within 242–261 ms; the gap and next step updated
- [ ] The whisper model test passes with `ggml-base.en.bin` and `ggml-apex-hinglish-q8_0.bin`
- [ ] The spec's status is updated and these boxes are ticked

**Verification:**
- [ ] Release-build runs; the docs link check passes

**Dependencies:** Tasks 3 and 4

**Files likely touched:** `docs/status.md`, `docs/specs/relative-silence.md`, `tasks/plan.md`, `tasks/todo.md`

**Estimated scope:** Small

## Checkpoint B: Done

- [ ] All seven spec success criteria verified
- [ ] `cargo test` passes, and the whisper model test passes with both models
- [ ] Reviewed with the user, then `feature/relative-silence` merged into `main`, and this plan moved to `tasks/archive/relative-silence/`
