# ADR-006: Test only with synthetic clips generated offline

## Status
Accepted

## Date
2026-10-18. Recorded on 2026-10-20 from [fixtures/generate.sh](../../fixtures/generate.sh), [p0_fixture.rs](../../crates/reelctl/tests/p0_fixture.rs), and [AGENTS.md](../../AGENTS.md).

## Context
AGENTS.md requires synthetic FFmpeg clips, no scraped Reels, and no network for the P0 test run. The tests also need inputs whose correct answers are known exactly.

## Decision
`fixtures/generate.sh` builds clips from FFmpeg's `lavfi` sources (solid colors and a 440 Hz sine), encoded with x264 at the `ultrafast` preset without B-frames, with 16-bit PCM audio:

| Clip | Content | Planted signals |
|---|---|---|
| `draft12` | 12 s, 640×360: red until 6 s, then white; audio muted from 2.0 to 2.5 s | one cut at 6 s, a 500 ms silence, no payoff cue |
| `outlier12` | 12 s, 640×360: red until 3 s, white until 9 s, then green; continuous tone | cuts at 3 s and 9 s, a payoff cue at 9 s |
| `bench60` | 60 s, 1920×1080: red until 30 s, then white; audio muted from 10.0 to 10.5 s | timing benchmark |

The end-to-end test generates `draft12` and `outlier12` into a temporary directory on every run. Generated clips and outputs are gitignored.

## Alternatives Considered

### Real or scraped reels
- Not allowed by AGENTS.md.

## Consequences
- Tests are deterministic, run offline, and keep media out of git.
- The tests show that the pipeline finds planted signals. They say nothing about accuracy or speed on real footage, with its texture, motion, B-frames, music, and transitions. Check real reels by hand, keep them out of the repo, and record the results in [status](../status.md).
- Running the tests needs `bash` and the `ffmpeg` command with libx264.
