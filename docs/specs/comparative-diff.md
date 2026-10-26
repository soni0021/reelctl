# Spec: Comparative diff against one to three outliers

Status: approved 2026-10-26, with the five answers recommended at the end of this spec; built on `feature/comparative-diff` and merged into `main` 2026-10-26. All six success criteria verified (evidence in [status.md](../status.md#comparative-diff-2026-10-26)). Plan: [tasks/archive/comparative-diff/plan.md](../../tasks/archive/comparative-diff/plan.md). Branch: `feature/comparative-diff`. Module `comparative-diff` in the [P0 capability map](p0-capability-map.md). Follows [planer.md](../../planer.md) Pass 2 ("aligns a draft against 1–3 competitor outlier blueprints") and closes the main gap in [status.md](../status.md#gaps).

## Objective

`reelctl diff` takes one outlier and uses it for one thing: the missing-payoff flag. Dead air and static windows are properties of the draft alone, so swapping the outlier changes nothing else. Run on two real reels (a 15 s near-silent clip against a 38.5 s talking explainer, and the reverse), the reference barely mattered. A creator comparing a draft with a high-performing reel wants to know how the draft's rhythm differs from it, not only whether it breaks an absolute rule.

This module makes `diff` compare. It accepts one to three outliers and reports, side by side with the draft:

- **Cut rhythm, second by second.** Cuts in each 5 s window of the draft and of every outlier, over the draft's length. Where most of the outliers change the picture and the draft does not, a finding says so.
- **Pause density.** Dead-air gaps per minute and the share of time that is silent, for the draft and each outlier.
- **Speaking rate and loudness.** Words per minute and integrated LUFS, side by side.
- **Shot length.** Mean and longest shot, side by side.
- **The existing `flags`**, byte-for-byte as today: dead air over 180 ms, static windows over 2.5 s, and the missing payoff cue, which with several outliers is raised when more than half of them have a cue and the draft does not (with one outlier this is today's rule).

Output stays JSON on stdout, now with a `comparison` object next to `flags` (the relative findings live in `comparison.flags`, so `flags` and every script that reads it do not change), and gains `--format text` for a readable report. Nothing is decoded again for outliers: they are `edit_dna.json` files from earlier `decompile` runs.

## Evidence

Measured 2026-10-26 with the merged build on two real reels kept outside the repo (neither is committed):

| | NEET explainer | Milk clip |
|---|---|---|
| Length, frame | 38.5 s, 1080×1920 | 15.1 s, 720×1280 |
| Cuts, mean shot, longest shot | 1 (at 8.47 s), 19.2 s, 30.0 s | 0, 15.1 s, 15.1 s |
| Dead-air gaps over 180 ms | 18 (28 per minute), 28% of the time silent | 2 (8 per minute), 9% silent |
| Words per minute (silence bursts, no whisper model) | 144.9 | 19.9 |
| Integrated loudness | −22.4 LUFS | −48.6 LUFS |

- **Expected output on this pair** (what success criterion 5 checks):
  - *Milk clip as draft, NEET as outlier.* `flags` is what `diff` prints today (two dead-air, one static window, and the missing payoff cue). `comparison.flags` has one `rhythm_gap` for 5,000–10,000 ms: the outlier's cut at 8,466 ms falls there and the draft has none. No `pause_density`: the draft's 8 pauses per minute are under the outlier's 28.
  - *NEET as draft, milk clip as outlier.* `flags` is today's 18 dead-air lines and 2 static windows. `comparison.flags` has no `rhythm_gap` (the outlier has no cuts, and after 15.1 s it has ended and does not vote) and one `pause_density`: 28.1 per minute against 8.0 passes both the 1.5× and the 3-per-minute tests. That flag is expected and is a known limit, not a finding about the reel: the milk clip's few pauses come from near-silent audio. No further threshold is added to suppress it.
- With two slow reels there is little to compare, so this pair is a smoke test; a fast-cut outlier is the real case.
- The NEET reel's "cut" at 8.47 s is a text box appearing under a still face (scene score 0.303 against the 0.30 threshold). A cut-count comparison inherits that detector's limits; `visual-motion` is the module that names such events.
- The pauses flagged on the talking reel are mostly natural speech pauses of 0.2–0.6 s. The absolute 180 ms limit is strict for talking video, which is why pause density is compared with the outlier's instead of judged alone (see Resolved Questions).

## Tech Stack

Rust workspace as today. No new dependencies. The comparison is pure code over `EditDna` values in `crates/diff`, with the argument and output handling in `crates/reelctl`.

## Commands

- `reelctl diff --draft <draft.mp4> --outlier <a.edit_dna.json> [--outlier <b.edit_dna.json> [--outlier <c.edit_dna.json>]] [--format json|text]`
- `cargo test -p diff`, `cargo test -p reelctl --test p0_fixture`, then `cargo test --workspace` once at the end.

## Project Structure

- `crates/diff/src/lib.rs` (or a new `compare.rs` beside it): metrics per reel, the rhythm windows, the new flags, the text report.
- `crates/reelctl/src/main.rs`: `--outlier` repeatable (one to three), `--format`.
- `crates/reelctl/tests/p0_fixture.rs`: end-to-end with fixtures.
- Docs: ADR-014, `docs/cli.md`, README, `docs/status.md`, CHANGELOG, `docs/edit-dna.md` (only if its text changes; the `diff` JSON is documented in `cli.md`), this spec, `tasks/`.

## Behavior

**Arguments.** `--outlier` may be given one to three times. Zero is an error as today; four or more exits with code 2 and a message. Each outlier file is read as today (schema version 1 or 2).

**Metrics per reel** (draft and each outlier): `duration_ms`, `cut_count`, `mean_shot_ms`, `longest_shot_ms`, `dead_air_count` and `dead_air_per_minute` (gaps over 180 ms), `silent_share` (silence gap time divided by duration), `words_per_minute` with `words_source` (`whisper_cpp` or `silence_bursts`, from `asr_backend`), and `integrated_lufs`.

**Cut rhythm.** Windows of 5,000 ms from 0 to the draft's duration (the last one shorter). For each window: the draft's cut count and each outlier's cut count. A cut belongs to the window containing its `time_ms`. An outlier that has ended before a window starts has no count there (`null`), and does not vote on it. The window list is part of `comparison`.

**Relative findings**, in `comparison.flags` (never in `flags`), in this order:
- `rhythm_gap { window_start_ms, window_end_ms, outliers_with_cut, outliers_counted }`: a window where the draft has no cut and more than half of the outliers still running have at least one. This is the same "more than half" rule as the payoff cue. Adjacent flagged windows are reported as one span, with the counts of the window in it that has the fewest outliers cutting (the first on a tie).
- `pause_density { draft_per_minute, outlier_per_minute }`: raised once when the draft's dead-air gaps per minute exceed the median outlier's by more than 50% and by at least 3 per minute. The 3 per minute stops 1 against 0.6 from counting. With two outliers the median is their mean.

**Payoff.** With one outlier, unchanged. With two or three, the flag is raised when more than half of the outliers have a cue in 8–10 s and the draft has none.

**Statements the output does not make.** It does not rate a reel as good or bad, and `words_per_minute` from silence bursts is an estimate; its `words_source` says so. No flag is raised for speaking rate or loudness: they are listed, not judged, because one pair of reels says nothing about what is "too fast" or "too quiet".

**Text format.** `--format text` prints the comparison as a table (one column per reel; names cut to 8–20 characters and rows wrapped so no line passes 80 characters), the rhythm windows as a row of counts, and the flags as lines like `NO CUT 5.0-10.0 s: 1 of 1 outliers cut here, the draft does not`. JSON stays the default, so scripts and tests keep working.

**Compatibility.** `flags` is exactly what `diff` prints today, for one outlier and for several (apart from the payoff rule with several); `comparison` is a new key after it. The existing fixture tests, the `cli.md` example, and `Reels/compare.sh` (which prints any unknown kind in `flags` as a payoff warning) keep working. Once `--format text` exists, `Reels/compare.sh` switches to it.

Out of scope: aligning by content or by words (that needs transcripts), judging speech or loudness, per-second micro-timing of pauses, naming what kind of event a cut is (`visual-motion`), reading `.otio` drafts, comparing an outlier against another outlier, and any change to `decompile` or `edit_dna.json`.

## Code Style

Match `crates/diff`: plain structs with serde derives, small functions, constants named for their limits (`RHYTHM_WINDOW_MS`, `PAUSE_DENSITY_RATIO`, `PAUSE_DENSITY_MIN_EXTRA_PER_MINUTE`), no `unwrap` in library code.

## Testing Strategy

Test-first.

- **Unit tests in `crates/diff`** with hand-built `EditDna` values:
  - Metrics: mean and longest shot with and without cuts; dead-air density; silent share; zero-length and one-frame reels never divide by zero.
  - Rhythm windows: counts per window for the draft and for each outlier; an outlier that has ended gives `null` and does not vote; the last short window.
  - `rhythm_gap`: a draft with no cut where more than half of the running outliers have one is flagged (2 of 3 yes, 1 of 2 no, 1 of 1 yes); adjacent windows merge; a draft that cuts as often as the outliers raises none; a window after every outlier has ended raises none.
  - `pause_density` thresholds: below 1.5× or under 3 extra per minute raises nothing; above both raises one flag.
  - Payoff with one, two (needs both), and three outliers (needs two).
  - The existing diff tests pass unchanged.
- **End-to-end** (`p0_fixture.rs`, fixtures only, no network): `draft12` against `outlier12` gives exactly its four `flags` as today, plus `comparison` (its cut at 6 s against the outlier's at 3 s and 9 s gives a `rhythm_gap` for 0–5 s); `draft12` against `outlier12` and `textured_cuts12` gives per-outlier metrics, the window rows, and `rhythm_gap` only where both outliers cut and the draft does not; four outliers exit with code 2; `--format text` contains the table header and the flags; `--format json` parses.
- **Manual, not committed:** the two real reels in both directions, recorded in `docs/status.md`.

## Boundaries

- **Always:** keep `flags`' existing entries, shape, and order; keep `decompile` untouched; update the docs in the same commit.
- **Ask first:** changing the 180 ms or 2.5 s limits; adding a field to `edit_dna.json`; adding a dependency; judging speech rate or loudness.
- **Never:** commit the real reels or their analyses; use the network in tests; weaken CONSTRAINTS.md.

## Success Criteria

1. `diff` accepts one to three `--outlier` files; four exits with code 2.
2. `flags` is identical to today's for every existing fixture pair and for the two real reels; the relative findings are only in `comparison.flags`.
3. The output has per-reel metrics and the cut-rhythm windows for the draft and each outlier, in JSON and in `--format text`.
4. `rhythm_gap` and `pause_density` fire on the fixture pairs built to trigger them, and not on pairs built not to; the payoff rule follows the "more than half" rule.
5. On the two real reels, in both directions, the output matches the expected output listed in Evidence, recorded in `status.md`.
6. ADR-014 records the decision, and the docs match the code.

## Resolved Questions (the recommended answers, approved 2026-10-26)

1. **Which thresholds?** 5 s windows, the 1.5× and 3-per-minute rule for pause density, and "more than half" for payoff are choices made here, from one pair of slow reels. Recommended: accept them as named constants, say so in `status.md`, and retune on the first fast-cut outlier you download.
2. **Should the dead-air limit change for talking video?** Recommended: no, not in this module. It is "ask first" and the absolute flags keep working; `pause_density` already says whether the draft's pauses are unusual compared with the outlier's. Revisit with real whisper word timing.
3. **Should speaking rate and loudness raise flags?** Recommended: no, list them only (see Behavior).
4. **Is `--format text` in scope?** Recommended: yes. planer.md asks for a machine- and human-readable diff, and the text report replaces the helper script in `Reels/`.
5. **First test pair.** Recommended: use the two reels you have for a smoke test, and a fast-cut reference when you download one; the fixtures carry the automated tests.
