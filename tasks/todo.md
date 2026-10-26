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

## Task 2: `p010` planes, the decoder choice, and `decode_note`

**Description:** `Plane::Ten` takes the bit shift (2 for `yuv420p10le`, 8 for `p010`). `yuv_view` accepts `p010` (and `nv12` hardware frames, already supported). A pure function chooses hardware or software from the codec and the `--decoder` setting. `decode_note` gets the sentence for each case. No FFI yet.

**Acceptance criteria:**
- [ ] Unit tests: `p010` reads the high byte of each sample, with strides wider than the row; 10-bit `yuv420p10le` is unchanged
- [ ] Unit tests: HEVC tries hardware under `auto`; H.264 and other codecs use software; `software` forces software
- [ ] Unit tests: `decode_note` for software by choice, by fallback (each reason), and for hardware
- [ ] Every fixture matches its baseline apart from timings

**Verification:**
- [ ] `cargo test` (new tests red first)
- [ ] Baseline comparison with a release build

**Dependencies:** Task 1

**Files likely touched:** `crates/decompile/src/decode.rs`

**Estimated scope:** Small

## Task 3: VideoToolbox decode with fallback and `--decoder`

**Description:** `hwdecode.rs` creates the VideoToolbox device, sets the decoder's `get_format` callback, and transfers each frame to a software frame. `decode_file` uses it for HEVC under `auto`, and falls back to software on a missing device, an open error, or an error on the first frame. `decompile --decoder <auto|software>` is added and documented.

**Acceptance criteria:**
- [ ] `draft12_10bit` and `textured_cuts12` (as HEVC) give the same `edit_dna.json` with hardware and with `--decoder software`, apart from timings and `decode_note`; the test skips itself, printing why, when there is no VideoToolbox
- [ ] With hardware failing (forced in a test), the output equals software's and `decode_note` gives the reason
- [ ] Decoding the 12 s HEVC fixture 50 times in one process does not grow memory beyond a stated bound
- [ ] Docs: `docs/cli.md` (`--decoder`), `docs/edit-dna.md` (`decode_note`), ADR-012, CHANGELOG

**Verification:**
- [ ] `cargo test` (new tests red first)
- [ ] Corpus: all four encodings, hardware against the baseline software outputs
- [ ] The iPhone file: hardware against software, three alternating runs

**Dependencies:** Task 2

**Files likely touched:** `crates/decompile/src/hwdecode.rs`, `crates/decompile/src/decode.rs`, `crates/decompile/src/pipeline.rs`, `crates/reelctl/src/main.rs`, `docs/cli.md`, `docs/edit-dna.md`, ADR-012, `CHANGELOG.md`

**Estimated scope:** Medium

## Checkpoint A

- [ ] `cargo test` passes
- [ ] All fixtures unchanged apart from timings
- [ ] Corpus and the iPhone file: same cuts, scores within 0.005, equal colours and thumbnails with hardware and software

## Task 4: Loop filter off for all frames, if cuts stay identical

**Description:** Measure software decoding with the loop filter skipped on every frame against today's setting, on the corpus, the fixtures, and the iPhone file's 8-bit conversion. Adopt it only if every cut and scene score (within 0.005) is unchanged.

**Acceptance criteria:**
- [ ] Either the change is adopted with the corpus evidence recorded, or it is not and the status page says why
- [ ] If adopted: the 60 s H.264 benchmark's median `signal_ms` stays at or below today's, and all fixtures give identical output

**Verification:**
- [ ] Corpus comparison; alternating benchmark runs

**Dependencies:** Checkpoint A

**Files likely touched:** `crates/decompile/src/decode.rs`, `docs/status.md`

**Estimated scope:** Small

## Task 5: Measurements and docs

**Description:** Re-measure the real iPhone file (hardware against software, alternating) and the benchmark. Record the per-class decode cost and the unmet 500 ms in the status page. Write ADR-012 and mark ADR-001 as partly superseded.

**Acceptance criteria:**
- [ ] `docs/status.md`: the measured cost per 60 s by input class, 500 ms recorded as unmet on real footage with numbers, the capability map and spec updated, boxes ticked
- [ ] The iPhone file's `signal_ms` is at least 40% lower with hardware, in three alternating runs
- [ ] The docs link check passes

**Verification:**
- [ ] Release-build runs; the `audit` subagent on the final commit

**Dependencies:** Tasks 3 and 4

**Files likely touched:** `docs/status.md`, `docs/specs/fast-decode.md`, `docs/specs/p0-capability-map.md`, `docs/decisions/`, `tasks/`

**Estimated scope:** Small

## Checkpoint B: Done

- [ ] All six spec success criteria verified
- [ ] `cargo test` passes
- [ ] The `audit` subagent has run on the final commit
- [ ] Reviewed with the user, then `feature/fast-decode` merged into `main`, and this plan moved to `tasks/archive/fast-decode/`
