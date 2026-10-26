# Tasks: fast decode

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/fast-decode.md](../docs/specs/fast-decode.md). Tick each box in the commit that completes it.

## Task 1: Baselines, the `textured_cuts12` fixture, and the corpus tooling

**Description:** Save the baseline output of every existing fixture from the current release build. Add `textured_cuts12` (12 s, 640×360, hard cuts between textured sources, B-frames, scene-cut detection off) to `fixtures/generate.sh`, with an end-to-end test that its planted cuts are found by the software path. Keep the corpus script and its four encodings outside the repo, and save the software outputs for them.

**Acceptance criteria:**
- [x] `textured_cuts12` finds each planted cut within one frame, in the default suite
- [x] The existing fixtures' baselines are saved, and the corpus has four software `edit_dna.json` files
- [x] `generate.sh all` includes the new fixture and every clip keeps its codec (checked with `ffprobe`)
- [x] Docs: README fixture list and status.md (CHANGELOG not needed: no user-visible change)

**Verification:**
- [x] `cargo test` (new test red first, against a wrong expected cut)
- [x] `ffprobe` on `generate.sh all`

**Dependencies:** None

**Files likely touched:** `fixtures/generate.sh`, `crates/reelctl/tests/p0_fixture.rs`, `README.md`

**Estimated scope:** Small

## Task 2: `p010` planes

**Description:** `Plane` gets a `High16` variant that reads the high byte of a 16-bit sample, which is the top 8 bits of a `p010` 10-bit value. `yuv_view` accepts `p010le` (luma plus interleaved chroma, range from the frame's metadata), and the unsupported-format message lists it. The decoder choice and the `decode_note` sentences for hardware, fallback, and forced software moved to Task 3: they have no caller before it, and the project allows no `#[allow(dead_code)]`.

**Acceptance criteria:**
- [x] Unit tests: `p010` reads the high byte of each sample, with strides wider than the row; every 10-bit value reads the same from `yuv420p10le` and `p010`
- [x] Unit tests: `p010` pixels convert like the same 8-bit values; a real `p010le` frame is read, follows its range metadata, and counts as 10-bit
- [x] Every fixture matches its baseline apart from timings
- [x] Docs: `docs/cli.md` input requirements

**Verification:**
- [x] `cargo test` (new tests red first)
- [x] Baseline comparison with a release build

**Dependencies:** Task 1

**Files likely touched:** `crates/decompile/src/decode.rs`, `docs/cli.md`

**Estimated scope:** Small

## Task 3: VideoToolbox decode with fallback and `--decoder`

**Description:** A pure function chooses hardware or software from the codec and the `--decoder` setting, and `decode_note` gets a sentence for hardware, for each fallback reason, and for forced software (moved here from Task 2). `hwdecode.rs` creates the VideoToolbox device, sets the decoder's `get_format` callback, and transfers each frame to a software frame. `decode_file` uses it for HEVC under `auto`, and falls back to software on a missing device, an open error, or an error on the first frame. `decompile --decoder <auto|software>` is added and documented.

**Acceptance criteria:**
- [x] Unit tests: HEVC tries hardware under `auto`; H.264 and other codecs use software; `software` forces software; `decode_note` text for each case, and unchanged for 8-bit software
- [x] `draft12_10bit` and `textured_cuts12` (as HEVC) give the same `edit_dna.json` with hardware and with `--decoder software`, apart from timings and `decode_note`; on macOS the test asserts that the note says the media engine ran
- [x] With hardware failing (forced in a test), the output equals software's and `decode_note` gives the reason
- [x] Decoding a 2 s 1280×720 HEVC clip 50 times in one process does not grow memory beyond 20 MiB, and an HDR10 clip does not pile up side data on the reused frame
- [x] Docs: `docs/cli.md` (`--decoder`), `docs/edit-dna.md` (`decode_note`), ADR-012, CHANGELOG

**Verification:**
- [x] `cargo test` (new tests red first)
- [x] Corpus: all four encodings, hardware against the baseline software outputs
- [x] The iPhone file: hardware against software, three alternating runs

**Dependencies:** Task 2

**Files likely touched:** `crates/decompile/src/hwdecode.rs`, `crates/decompile/src/decode.rs`, `crates/decompile/src/pipeline.rs`, `crates/reelctl/src/main.rs`, `docs/cli.md`, `docs/edit-dna.md`, ADR-012, `CHANGELOG.md`

**Estimated scope:** Medium

## Checkpoint A

- [x] `cargo test` passes
- [x] All fixtures unchanged apart from timings
- [x] Corpus and the iPhone file: same cuts, scores within 0.005, equal colours and thumbnails with hardware and software

## Task 4: Loop filter off for all frames, if cuts stay identical

**Outcome (done inside Task 3):** the experiment ran the other way. Today's skip on non-reference frames already moved HEVC scene scores by up to 0.022 against the hardware decoder, so software now filters every frame; nothing is skipped. Evidence in [status.md](../docs/status.md#fast-decode-2026-10-26).

**Description:** Measure software decoding with the loop filter skipped on every frame against today's setting, on the corpus, the fixtures, and the iPhone file's 8-bit conversion. Adopt it only if every cut and scene score (within 0.005) is unchanged.

**Acceptance criteria:**
- [x] Either the change is adopted with the corpus evidence recorded, or it is not and the status page says why
- [x] The 60 s H.264 benchmark's median `signal_ms` is unchanged (243.0 before, 242.5 after) and all fixtures give identical output apart from `decode_note`

**Verification:**
- [x] Corpus comparison; alternating benchmark runs

**Dependencies:** Checkpoint A

**Files likely touched:** `crates/decompile/src/decode.rs`, `docs/status.md`

**Estimated scope:** Small

## Task 5: Measurements and docs

**Description:** Re-measure the real iPhone file (hardware against software, alternating) and the benchmark. Record the per-class decode cost and the unmet 500 ms in the status page. Write ADR-012 and mark ADR-001 as partly superseded.

**Acceptance criteria:**
- [x] `docs/status.md`: the measured cost per 60 s by input class, 500 ms recorded as unmet on real footage with numbers, the capability map and spec updated, boxes ticked
- [x] The iPhone file's `signal_ms` is at least 40% lower with hardware, in three alternating runs
- [x] The docs link check passes

**Verification:**
- [x] Release-build runs; the `audit` subagent on the final commit

**Dependencies:** Tasks 3 and 4

**Files likely touched:** `docs/status.md`, `docs/specs/fast-decode.md`, `docs/specs/p0-capability-map.md`, `docs/decisions/`, `tasks/`

**Estimated scope:** Small

## Checkpoint B: Done

- [ ] All six spec success criteria verified
- [ ] `cargo test` passes
- [ ] The `audit` subagent has run on the final commit
- [ ] Reviewed with the user, then `feature/fast-decode` merged into `main`, and this plan moved to `tasks/archive/fast-decode/`
