# ADR-014: Compare a draft with several outliers, in a separate key

## Status
Accepted

## Date
2026-10-26

## Context
`reelctl diff` took one outlier and used it for one thing, the missing-payoff flag. Dead air and static windows depend on the draft alone, so on two real reels (a 15 s near-silent clip and a 38.5 s talking explainer) swapping the outlier changed almost nothing. planer.md asks for a draft aligned against one to three outlier blueprints. Existing readers of `flags` (the tests, the `cli.md` example, a helper script that prints unknown kinds as a payoff warning) must not break. The requirements are in the [spec](../specs/comparative-diff.md).

## Decision
- **`--outlier` repeats, one to three times.** A fourth is an argument error with exit code 2, raised through clap's own error path because `main` turns every other error into exit code 1.
- **`flags` is unchanged.** The comparison is a second key, `comparison`, after it, and the relative findings live in `comparison.flags`, so no reader of `flags` can meet an unknown kind. With several outliers only the payoff flag's rule changes: a cue in more than half of them.
- **Metrics from `edit_dna.json` alone** (cuts, silence gaps, speaking rate with its source, loudness), so outliers are never decoded again and older files work.
- **Cut rhythm in 5 s windows,** with a vote by "more than half of the outliers still running". An outlier that has ended has no count and does not vote.
- **Two relative findings:** `rhythm_gap` (the draft has no cut where most outliers do) and `pause_density` (more than 1.5 times the outliers' median pauses per minute and at least 3 more). Speaking rate and loudness are listed, not judged.
- **`--format text`** prints a table and one line per flag, from the same report, so the two formats cannot disagree.

## Alternatives Considered

### Add the new flags to `flags`
- Breaks the promise that `flags` is the same for one outlier: the first fixture pair would get a new flag, and a script that treats unknown kinds as payoff warnings would mislabel it.

### A new subcommand, `reelctl compare`
- Leaves `diff` doing the same job with one outlier. A key in `diff`'s output costs less and keeps one entry point.

### Align by content or by words
- Needs transcripts for every reel. The windows work from cuts, which every `edit_dna.json` has.

### Flag speaking rate and loudness
- One pair of reels says nothing about what is too fast or too quiet, and a silence-burst rate is an estimate.

## Consequences
- The thresholds (5 s, 1.5, 3 per minute) come from two slow reels and have not been tried against a fast-cut outlier. They are named constants to retune on the first one.
- `pause_density` compares counts, so a near-silent outlier (the milk clip) makes a talking draft look pause-heavy. That is a known limit, not a finding about the draft.
- The last window can be a sliver (the 15.1 s clip has a 92 ms one from 15 to 15.1 s). It is a row of counts, and it can raise a `rhythm_gap` only if the outliers cut inside it.
- A cut is whatever the detector found, so a caption box appearing counts (the NEET reel's only cut). Naming the event is `visual-motion`.
