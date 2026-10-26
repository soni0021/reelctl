# Tasks: decode inputs

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/decode-inputs.md](../docs/specs/decode-inputs.md). Tick each box in the commit that completes it.

## Task 1: Read 10-bit 4:2:0 video

**Description:** Save the baseline output of every existing fixture. Then replace `YuvView`'s byte slices with a `Plane` accessor that reads 8-bit bytes or the top 8 bits of 10-bit little-endian samples, add `yuv420p10le` to `chroma_planes`, and make `decode_note` mention 10-bit and HDR input. Add the `draft12_10bit` fixture and an end-to-end test.

**Acceptance criteria:**
- [x] Unit tests: the accessor returns `v >> 2` for 10-bit and the byte for 8-bit, with strides wider than the row
- [x] `draft12_10bit` gives `draft12`'s cut (± 1 frame), silence gaps, and payoff result, per-shot mean RGB within 3, and a `decode_note` that mentions 10-bit
- [x] Every existing fixture matches its baseline apart from timings
- [x] An unsupported format's error names the format
- [x] Docs: `docs/cli.md` input requirements, `docs/edit-dna.md` (`decode_note`), README fixture list, CHANGELOG, ADR-011

**Verification:**
- [x] `cargo test` (new tests red first)
- [x] Baseline comparison with a release build

**Dependencies:** None

**Files likely touched:** `crates/decompile/src/decode.rs`, `fixtures/generate.sh`, `crates/reelctl/tests/p0_fixture.rs`, plus the docs above

**Estimated scope:** Medium

## Task 2: Resample audio as one stream

**Description:** A resampler struct that keeps its position and last sample across frames replaces the per-frame `resample_linear`. Add the `draft12_44k` fixture and an end-to-end test.

**Acceptance criteria:**
- [x] Unit tests: 60 s of 44.1 kHz audio in 1,024-sample frames gives 2,880,000 samples ± 1; an impulse at 50.0 s comes out at 50.0 s ± 1 sample
- [x] `draft12_44k` finds its planted pause within 10 ms of 50,016–50,503 ms (the mute's real position; see the spec)
- [x] Every existing fixture (all 48 kHz) matches its baseline apart from timings
- [x] Docs: CHANGELOG; status.md known issue updated

**Verification:**
- [x] `cargo test` (new tests red first)

**Dependencies:** None (Task 1 first only to keep the baseline comparison simple)

**Files likely touched:** `crates/decompile/src/decode.rs`, `fixtures/generate.sh`, `crates/reelctl/tests/p0_fixture.rs`, `CHANGELOG.md`, `docs/status.md`

**Estimated scope:** Small

## Checkpoint A

- [x] `cargo test` passes
- [x] All existing fixtures unchanged apart from timings

## Task 3: Real footage, measurements, and docs

**Description:** Decompile the original `IMG_1435.MOV` and an 8-bit conversion of the whole file with the audio copied, and compare. Measure the benchmark and the 10-bit decode time. Record everything.

**Acceptance criteria:**
- [x] `IMG_1435.MOV` exits 0; its cuts and silence gaps equal the 8-bit conversion's; `decode_note` names 10-bit and HLG (spec criteria 1 and 5)
- [x] The benchmark's `signal_ms` stays in 242–262 ms, alternating with the current build
- [x] `docs/status.md`: the 10-bit gap closed, decode times recorded, and the drift known issue closed; spec status updated; boxes ticked

**Verification:**
- [x] Release-build runs; the docs link check passes

**Dependencies:** Tasks 1 and 2

**Files likely touched:** `docs/status.md`, `docs/specs/decode-inputs.md`, `docs/specs/p0-capability-map.md`, `tasks/plan.md`, `tasks/todo.md`

**Estimated scope:** Small

## Checkpoint B: Done

- [ ] All six spec success criteria verified
- [ ] `cargo test` passes
- [ ] Reviewed with the user, then `feature/decode-inputs` merged into `main`, and this plan moved to `tasks/archive/decode-inputs/`
