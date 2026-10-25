# Spec: Silence relative to each recording's noise floor

Status: approved 2026-10-25, with the recommended answer to each open question (see Resolved Questions). Branch: `feature/relative-silence`. Plan: [tasks/plan.md](../../tasks/plan.md).

## Objective

`silence_gaps` counts a 10 ms window as silent when its level is below a fixed −40 dBFS. Real recordings have room noise above that, so pauses go undetected. On the first 60 s of the iPhone recording in [docs/status.md](../status.md#first-real-footage-2026-10-20), −40 dBFS found one gap of 180 ms or more and none of 300 ms or more. As a result:

- `reelctl diff` reports almost no dead air on real speech, which defeats its main check.
- whisper word ends stop at silence gaps ([ADR-007](../decisions/ADR-007-whisper-cpp-word-timing.md)), so on that recording 9 of 183 words end inside a real pause.

This change sets the silence threshold per recording, from the recording's own level distribution, so pauses under room noise become gaps. Clean recordings and every existing synthetic clip keep exactly the gaps they have today.

Behavior:

- **Threshold:** `max(−40, min(p10 + 6, p90 − 12))` dB, where p10 and p90 are the 10th and 90th percentiles of the 10 ms window levels over the whole recording. p10 estimates the noise floor; 6 dB above it is still noise. The `p90 − 12` cap keeps the threshold at least 12 dB under the loud part of the recording, so a recording with little or no quiet audio, such as a constant tone, is not marked silent. The `−40` floor means a clean recording keeps today's threshold.
- **Short sounds inside a gap:** a gap ends only after at least 30 ms (3 windows) of sound. Shorter sounds, such as noise spikes, stay inside the gap. Today one 10 ms window above the threshold ends a gap. A gap still needs at least 80 ms of silent windows.
- **The burst estimator** (the no-model word path in `asr.rs`) uses the same threshold, so bursts and gaps agree. Its 40 ms split rule does not change.
- `signal_ms` includes the threshold calculation, because silence detection is part of the structural path. Sorting about 6,000 window levels for a 60 s clip costs well under a millisecond.

Out of scope: voice activity detection models, spectral noise estimation, per-section thresholds that change over a recording, a CLI override flag (see Resolved Questions), and any change to the 180 ms dead-air limit in `diff`.

## Evidence

All measured 2026-10-25 on an Apple M3 with a Python model of `silence.rs` (10 ms mean-square windows, 80 ms minimum gap). The model was fed audio mixed the way `reelctl` mixes it, by averaging the channels; at −40 dB it reproduced `reelctl`'s 24 gaps exactly. FFmpeg's `-ac 1` downmix comes out about 3 dB louder than that average, so thresholds read from FFmpeg tools are not comparable.

**Real recording, first 60 s, `reelctl`'s mono mix:** window levels p5 −42.7, p10 −41.5, p50 −31.3, p90 −22.6 dB. A gap that contains a word start (from Apex DTW, more than 100 ms inside the gap) means speech was called silence.

| Threshold (dB) | Gaps ≥ 180 ms | Gaps ≥ 300 ms | Time silent | Word starts inside a gap |
|---|---|---|---|---|
| −40 (today) | 1 | 0 | 4.7% | 0 |
| −38 | 18 | 7 | 16.1% | 0 |
| −36 | 32 | 16 | 26.8% | 0 |
| −34 | 33 | 21 | 33.2% | 0 |
| −33 | 35 | 22 | 36.1% | 1 |
| −30 | 49 | 31 | 48.6% | 5 |

The rule gives −35.5 dB here, 2.5 dB below where speech starts to count as silence. With the 30 ms rule, it finds 31 gaps of 180 ms or more and 24 of 300 ms or more, and no word starts inside any gap. Requiring 40 ms of sound instead let one word start fall inside a gap, so 30 ms is the limit.

**Synthetic clips:** the rule changes no existing clip's gaps.

| Clip | p10 | p90 | Threshold | Gaps ≥ 180 ms, today and with the rule |
|---|---|---|---|---|
| `draft12` (tone, planted 0.5 s mute) | −21.2 | −21.0 | −33.0 | 2,010–2,510 ms |
| `outlier12` (tone, no mute) | −21.2 | −21.0 | −33.0 | none |
| `bench60` (tone, planted 0.5 s mute) | −21.2 | −21.0 | −33.0 | 10,010–10,510 ms |
| `speech12` (`say`, digital silence) | −120 | −14.4 | −40.0 | 0–500, 1,280–2,500, 4,860–12,000 ms |

**Noise under a pause:** `draft12`'s tone with white noise mixed in (FFmpeg `anoisesrc`, amplitude 0.02, fixed seed) puts every pause window between −39.2 and −38.4 dB. −40 dB finds no gap. The rule's threshold is −32.9 dB and finds 2,010–2,510 ms, the planted pause. With pink noise, single 10 ms spikes split the pause in two until the 30 ms rule was added.

## Tech Stack

No new dependencies. Rust in `crates/decompile`, test clips from FFmpeg's `lavfi` sources (`sine`, `anoisesrc` with a fixed seed), so they are reproducible and offline ([ADR-006](../decisions/ADR-006-synthetic-offline-fixtures.md)).

## Commands

```bash
cargo check                    # after every edit
cargo test                     # default suite, includes the new noisy-clip test
REELCTL_WHISPER_MODEL=~/models/whisper/ggml-apex-hinglish-q8_0.bin \
  cargo test -p reelctl --test whisper_fixture -- --include-ignored
bash fixtures/generate.sh noisy12 /tmp/noisy12.mp4
cargo build --release
target/release/reelctl decompile --input clip.mp4 --out clip.edit_dna.json
```

## Project Structure

| Path | Change |
|---|---|
| `crates/decompile/src/silence.rs` | `threshold_db(levels)`; gaps end only after 30 ms of sound; `detect_gaps` takes the threshold |
| `crates/decompile/src/asr.rs` | The burst estimator takes the same threshold |
| `crates/decompile/src/pipeline.rs` | Compute window levels and the threshold once, pass them to silence and ASR, record them in the output |
| `crates/edit-dna/src/lib.rs`, schema | `silence_threshold_db` and `noise_floor_db`; `SCHEMA_VERSION` 2 |
| `fixtures/generate.sh` | New `noisy12` clip: `draft12`'s video and tone, plus white noise that continues through the planted mute |
| `crates/reelctl/tests/p0_fixture.rs` | Decompile `noisy12`; check the planted pause is a gap and `diff` flags it as dead air |
| Docs | `docs/edit-dna.md`, `docs/cli.md` if output changes, `docs/status.md`, README commands, AGENTS.md (the fixed −40 dBFS fact), CHANGELOG, ADR-008, and the ADR-003 and ADR-007 consequences that mention −40 dBFS |

## Code Style

Match `silence.rs`: free functions over `&[f32]`, constants with their reason in the module comment, no `unwrap` outside tests.

```rust
const FLOOR_DB: f64 = -40.0;
const NOISE_MARGIN_DB: f64 = 6.0;
const HEADROOM_DB: f64 = 12.0;

pub fn threshold_db(levels: &[f64]) -> f64 {
    if levels.is_empty() {
        return FLOOR_DB;
    }
    let noise = percentile(levels, 10);
    let loud = percentile(levels, 90);
    (noise + NOISE_MARGIN_DB).min(loud - HEADROOM_DB).max(FLOOR_DB)
}
```

## Testing Strategy

Test-first with `cargo test`. All default tests are synthetic and offline.

- **Unit tests in `silence.rs`:**
  - The threshold is −40 for digital silence plus speech-like levels, follows `p10 + 6` for a noise floor above −46 dB, and is capped at `p90 − 12` for a constant level.
  - A pause of noise at −38 dB inside a −20 dB tone is a gap with the computed threshold, and not at −40.
  - A 10 or 20 ms sound inside a gap does not split it; 30 ms does.
  - The existing tests keep passing unchanged.
- **End-to-end, default suite:** `p0_fixture.rs` decompiles `noisy12`. Its gaps of 180 ms or more are exactly the planted pause (±20 ms), and `diff` flags it as dead air. The existing `draft12`/`outlier12` assertions stay as they are.
- **Model test, ignored by default:** `whisper_fixture.rs` keeps passing with both development models.
- **Manual, not committed:** rerun the first 60 s of the real recording and record the gap counts and word-end results in `docs/status.md`.

## Boundaries

- **Always:**
  - Keep every existing synthetic clip's gaps identical.
  - Keep the threshold calculation inside `signal_ms`.
  - Update the docs in the same commit.
- **Ask first:**
  - Changing the `edit_dna.json` schema beyond the two fields in Resolved Question 1.
  - Changing the 180 ms dead-air limit or the 80 ms minimum gap.
  - Adding a dependency.
- **Never:**
  - Commit real recordings or their audio.
  - Use the network in tests.

## Success Criteria

1. On `noisy12`, `silence_gaps` contains the planted 2.0–2.5 s pause (±20 ms) and no other gap of 180 ms or more, and `diff` flags it as dead air. At −40 dB the same clip has no gap.
2. `draft12`, `outlier12`, `bench60`, and `speech12` give the same `silence_gaps` as today.
3. On the first 60 s of the real recording, `reelctl` finds at least 20 gaps of 300 ms or more, and no Apex word start lies more than 100 ms inside a gap.
4. On that recording, fewer whisper word ends run more than 200 ms into a pause than the 9 of 183 today.
5. The 60 s benchmark's `signal_ms` stays within its current range (242–261 ms).
6. `cargo test` passes, and the whisper model test passes with both development models.
7. The docs listed under Project Structure are updated.

## Resolved Questions

Answered 2026-10-25 with the recommended options:

1. **Record the threshold in `edit_dna.json`?** Yes. Add `silence_threshold_db` and `noise_floor_db` (the p10 level), set `SCHEMA_VERSION` to 2, and give both fields `#[serde(default)]`, so `diff` still reads outlier files written before this change. A reader then knows why a pause was or was not a gap. The alternative is no schema change, with the threshold printed only in `asr_note`-style text, which tools cannot read.
2. **Should the burst estimator share the threshold?** Yes, as written above. Otherwise, without a whisper model, bursts and gaps disagree on what is silence.
3. **A `--silence-threshold-db` flag to override the rule?** No for now. Nothing needs it yet, and it can be added later without changing the rule.
