# Spec: Quiet recordings are not all silence

Status: draft, awaiting approval. Branch: `feature/quiet-audio` (started from `chore/remove-unreachable-arms`, which is not yet merged into `main`). Follows [ADR-008](../decisions/ADR-008-silence-relative-to-the-noise-floor.md) and the [relative-silence spec](relative-silence.md).

## Objective

The silence threshold is `max(−40, min(p10 + 6, p90 − 12))` dB. The −40 floor is applied last, so when a recording's loud part is under about −28 dB, the floor pushes the threshold above the loud part and the whole recording becomes one silence gap.

Found on the first reel downloaded for the comparison workflow (`Reels/`, a 15 s Instagram download with almost no audio). Window levels, measured with the same 10 ms mono mix `reelctl` uses: p10 −52.0, p50 −46.4, p90 −42.5, maximum −31.3 dB. The threshold came out at −40, only 36 of 1,509 windows reach it, and `silence_gaps` was one gap from 0 to 15,092 ms. `reelctl diff` then reports "dead air, 15 s", which tells the creator to trim a pause that is not there. The same happens for any quiet speaker or low-gain microphone: the file is not silent, it is quiet.

This change applies the loud-part cap last, so the threshold can never sit above `p90 − 12`:

`threshold = min(max(−40, min(p10 + 6, p90 − 12)), p90 − 12)`, which simplifies to `min(max(−40, p10 + 6), p90 − 12)`.

For every recording whose p90 is −28 dB or higher the result is unchanged, because there `p90 − 12` is at or above −40. That includes every synthetic fixture and the real iPhone recording (p90 −22.6). For quieter recordings the threshold drops below −40 to stay 12 dB under the loud part, so pauses are measured against the sound that is there.

On the Reels file the threshold becomes −54.5 dB. A Python model of `silence.rs` (10 ms windows, 80 ms minimum gap, 30 ms sound rule; to be confirmed with the real binary) gives 3 short gaps of 470, 80, and 110 ms in the first 0.9 s, 660 ms in all, instead of a gap over the whole clip. `diff` would flag none of them as dead air (the limit is 180 ms, and the 470 ms one at the start is the quiet opening of the file). The file has no pause to trim; its audio is just very quiet.

Out of scope: a new `edit_dna.json` field for "audio too quiet" (see Resolved Questions), voice activity detection, loudness normalization, and any change to the 180 ms dead-air limit, the 80 ms minimum gap, or the 30 ms sound rule.

## Evidence to collect before coding

- The Reels file: window levels, threshold, and gap count before and after, from the real binary (recorded in `docs/status.md`; the file itself is never committed).
- The real iPhone recording and every synthetic fixture: gaps identical before and after.

## Commands

- `cargo test -p decompile silence`, then `cargo test --workspace` once at the end.
- Manual: `reelctl decompile` on the Reels file, reading `silence_gaps` and `silence_threshold_db` from the JSON.

## Project Structure

- `crates/decompile/src/silence.rs`: `threshold_db` and its unit tests.
- `crates/reelctl/tests/p0_fixture.rs` and `fixtures/generate.sh`: a `quiet12` fixture.
- Docs: ADR-013 (supersedes the floor-last order in ADR-008), `docs/edit-dna.md` (`silence_threshold_db`), `docs/status.md`, CHANGELOG, this spec, `tasks/plan.md` and `tasks/todo.md`.

## Testing Strategy

Test-first.

- **Unit tests in `silence.rs`** (written failing first):
  - Levels like the Reels file (p10 −52, p90 −42.5) give a threshold of −54.5, not −40.
  - A quiet recording with a planted pause 20 dB under its speech keeps the pause as a gap and does not make the speech a gap.
  - The existing threshold tests pass unchanged: clean recording −40, noise floor at p10 + 6, constant level capped at p90 − 12, no audio −40.
- **End-to-end:** `quiet12`, 12 s of a tone at −46 dBFS with a planted 0.5 s mute. Today it is one gap over the whole clip; after the change its gaps of 180 ms or more are exactly the planted pause (±20 ms), and `diff` flags that pause and nothing else about silence. The existing fixtures' gaps do not change.
- **Manual:** the Reels file and the real iPhone recording, recorded in `docs/status.md`.

## Boundaries

- **Always:** keep every existing synthetic clip's gaps identical; keep the calculation inside `signal_ms`; update the docs in the same commit.
- **Ask first:** adding a field to `edit_dna.json`; changing 180 ms, 80 ms, or the 30 ms rule.
- **Never:** commit the real reel or its audio; use the network in tests; weaken CONSTRAINTS.md.

## Success Criteria

1. The Reels file gives no whole-clip silence gap, and `diff` raises no dead-air flag for anything but, at most, the opening 470 ms.
2. `quiet12` gives the planted pause only, as a gap, and the rest of the clip is not silence.
3. All existing fixtures and the real iPhone recording give identical `silence_gaps` and `silence_threshold_db`.
4. The existing silence unit tests pass unchanged; new tests failed before the change.
5. ADR-013 supersedes the order in ADR-008, and the docs match the code.

## Resolved Questions (recommended answers; please confirm)

1. **Should `edit_dna.json` say "audio too quiet to judge"?** Recommended: no. The threshold fix removes the false dead air, and a new field is a schema change (version 3). If the audio is almost absent, `integrated_lufs` already shows it (−48.6 here). Revisit if `comparative-diff` needs it.
2. **Should the threshold ever go below the noise floor?** On this file it does (−54.5 against p10 −52.0), which means almost nothing counts as silence (the model finds 660 ms of it). Recommended: accept it; a recording with no quiet stretch should have no gaps, which is the same reasoning as the existing cap.
