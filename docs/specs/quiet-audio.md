# Spec: Quiet recordings are not all silence

Status: implemented 2026-10-26 on `feature/quiet-audio`; criteria 1–4 verified (evidence in [status.md](../status.md#quiet-audio-2026-10-26)). Approved 2026-10-26, revised after review: levels re-measured with `reelctl`'s own mix, the rule uses p99 so mostly-silent recordings keep −40, and a −70 lower bound keeps near-total silence a gap (added during planning and approved with the plan). Branch: `feature/quiet-audio` (started from `chore/remove-unreachable-arms`, which is not yet merged into `main`). Follows [ADR-008](../decisions/ADR-008-silence-relative-to-the-noise-floor.md) and the [relative-silence spec](relative-silence.md).

## Objective

The silence threshold is `max(−40, min(p10 + 6, p90 − 12))` dB. The −40 floor assumes sound is louder than −40 dB. When a recording's sound is entirely quieter than that, the floor puts the threshold above all of it, and the whole recording becomes one silence gap.

Found on the first reel downloaded for the comparison workflow (`Reels/`, a 15 s Instagram download with almost no audio). The real binary reports `noise_floor_db` −55.05, `silence_threshold_db` −40.0, and one gap from 0 to 15,092 ms. A Python model of `silence.rs` with `reelctl`'s channel-average mix reproduces that (p10 −55.02, the same single gap), and gives p50 −49.4, p90 −45.5, p99 −41.9, maximum −34.3 dB: all the sound is under the −40 floor except a handful of windows. `reelctl diff` then reports "dead air, 15 s", which tells the creator to trim a pause that is not there. The same happens for any quiet speaker or low-gain microphone: the file is not silent, it is quiet.

**The rule.** Keep the formula, but let its floor come down when the recording's loudest windows are themselves under about −28 dB:

`threshold = max(max(min(−40, p99 − 12), −70), min(p10 + 6, p90 − 12))`

The floor is `−40` for any recording whose 99th-percentile window level is −28 dB or higher, so the threshold is unchanged for those. For a quieter recording the floor becomes `p99 − 12`: the threshold stays 12 dB under the recording's own loud part. It never goes below `−70` dB (`FLOOR_LIMIT_DB`): a recording with under 1% sound, such as a silent clip with one click, has a `p99` equal to its silence (−120), and without the bound the floor would fall to −132, below the silence, and the silence would stop being a gap. −70 is clear of the Reels file (−53.9), of a −50 dB tone (−62), and of 16-bit dither (about −96). `p99` is used, not `p90`, because a recording that is mostly digital silence or room noise has a `p90` equal to the silence itself; a `p90` cap applied last would push the threshold below the silence and stop real silence from being a gap.

**Who it changes.** Only recordings whose loudest 1% of 10 ms windows are quieter than −28 dBFS: near-silent downloads, very low gain, or a very quiet speaker. Every recording whose p99 is −28 dB or higher keeps today's threshold exactly. That includes every synthetic fixture, the real iPhone recording (p90 −22.6), and recordings that are mostly silence with at least 1% of windows at −20 dB or louder. A recording with under 1% sound (a silent clip with one click) moves from −40 to −70; its gaps change only if some window's level lies between −70 and −40, which digital silence does not have. A recording that is entirely digital silence also gets −70 and is still all one gap.

**On the Reels file** the threshold becomes −53.9 dB. The model gives 3 gaps instead of the whole clip: 0–990 ms, 1,370–1,640 ms, and 1,760–1,850 ms, 1,350 ms in all, all in the first 1.9 s. `diff` would flag the first two as dead air (over 180 ms). They are the quiet opening of the file, not a pause to trim; the audio is very quiet throughout (−48.6 LUFS), and `integrated_lufs` already says so. To be confirmed with the real binary.

Out of scope: a new `edit_dna.json` field for "audio too quiet" (see Resolved Questions), voice activity detection, loudness normalization, and any change to the 180 ms dead-air limit, the 80 ms minimum gap, or the 30 ms sound rule.

## Evidence to collect before coding

- The Reels file: window levels, threshold, and gap count before and after, from the real binary (recorded in `docs/status.md`; the file itself is never committed).
- The real iPhone recording and every synthetic fixture: gaps identical before and after.

## Commands

- `cargo test -p decompile silence`, then `cargo test --workspace` once at the end.
- Manual: `reelctl decompile` on the Reels file, reading `silence_gaps` and `silence_threshold_db` from the JSON. The levels above come from a Python model; the real binary decides.

## Project Structure

- `crates/decompile/src/silence.rs`: `threshold_db` and its unit tests.
- `crates/reelctl/tests/p0_fixture.rs` and `fixtures/generate.sh`: a `quiet12` fixture.
- Docs: ADR-013 (supersedes the threshold formula in ADR-008), `docs/edit-dna.md` (`silence_threshold_db`), `docs/status.md`, CHANGELOG, this spec, `tasks/plan.md` and `tasks/todo.md`.

## Testing Strategy

Test-first.

- **Unit tests in `silence.rs`**, written failing first:
  - Quiet recording: levels shaped like the Reels file (p10 about −55, p90 −45.5, p99 −42) give a threshold below −40, not −40.
  - Quiet recording with a pause: a tone at −46 dB with a planted pause 20 dB lower keeps the pause as a gap, and the tone is not a gap.
  - **Unchanged cases, which fail under a plain "cap last" formula:** 95 windows at −120 plus 5 at −14 stays −40; 92 at −60 plus 8 at −20 stays −40; digital silence with speech at −14 stays −40.
  - **The lower bound:** 995 windows at −120 plus 5 at −14 gives −70, not −132, and its gaps are the silence; a recording of only −120 gives −70 and one gap; a constant −50 tone gives −62.
  - The existing threshold tests pass unchanged: noise floor at p10 + 6 (−34), constant level capped at p90 − 12 (−33), no audio −40.
  - A silent recording (all `−120`) cannot give a `−inf` or NaN threshold.
- **End-to-end:** `quiet12`, 12 s of a tone at −46 dBFS with a planted 0.5 s mute. Today it is one gap over the whole clip; after the change its gaps of 180 ms or more are exactly the planted pause (±20 ms), and `diff` flags that pause. The existing fixtures' gaps and thresholds do not change.
- **Manual:** the Reels file and the real iPhone recording, with the real binary, recorded in `docs/status.md`.

## Boundaries

- **Always:** keep every existing synthetic clip's gaps identical; keep the calculation inside `signal_ms`; update the docs in the same commit.
- **Ask first:** adding a field to `edit_dna.json`; changing 180 ms, 80 ms, or the 30 ms rule.
- **Never:** commit the real reel or its audio; use the network in tests; weaken CONSTRAINTS.md.

## Success Criteria

1. The Reels file, run with the real binary, gives no whole-clip silence gap and a threshold below −40.
2. `quiet12` gives the planted pause as a gap and the rest of the clip is not silence.
3. All existing fixtures and the real iPhone recording give identical `silence_gaps` and `silence_threshold_db`; the mostly-silent unit cases stay at −40, and the under-1% case gives −70 with the silence still a gap.
4. The existing silence unit tests pass unchanged; the new tests failed before the change.
5. ADR-013 supersedes the threshold formula in ADR-008, and the docs match the code.

## Resolved Questions (recommended answers; please confirm)

1. **Should `edit_dna.json` say "audio too quiet to judge"?** Recommended: no. The threshold fix removes the false dead air, and a new field is a schema change (version 3). If the audio is almost absent, `integrated_lufs` already shows it (−48.6 here). Revisit if `comparative-diff` needs it.
2. **Is 12 dB under `p99` the right distance?** It reuses the 12 dB headroom of ADR-008, tuned on one iPhone recording. On the Reels file the other threshold, p10 + 6, is −49 and `p99 − 12` is −53.9, so the floor decides. Recommended: accept 12 dB; there is one quiet recording to test on, and a second one would be the time to retune. Recommended: accept it; a recording with no quiet stretch should have no gaps, which is the same reasoning as the existing cap.
