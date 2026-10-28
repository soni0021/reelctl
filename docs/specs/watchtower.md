# Spec: Outlier and growth watchtower (`watchtower`, P1)

Status: built (watchtower/); written 2026-10-28 under the user's "p1-p3" and the answer "build against recorded fixtures" to the Virlo question; the user has not reviewed it line by line. Branch: `feature/watchtower`. Follows [planer.md](../../planer.md) Component 1 and the P1 row of section 7. Depends on `reelctl decompile` (done).

## Objective

A creator wants to know which accounts in their niche are surging and why, without scrolling. The watchtower reads posts and follower snapshots for a niche, ranks creators by outlier multiplier and growth velocity, decompiles the breakout Reels with `reelctl decompile`, compares each surging creator's recent breakouts with their older baseline posts, and writes a daily digest ("Why They Grew + Exact Edit Blueprint") whose every number is measured.

Written in Go, as a separate module in `watchtower/`, so it does not join the Cargo workspace. It calls the `reelctl` binary as a process.

**Source of data.** Virlo's API (planer.md: `POST /v1/agents`, `POST /v1/tracking/creators`) cannot be built or tested here: there is no key in `.env` and its response format is not in the repo. The watchtower therefore reads through a `Source` interface and ships one implementation, `FileSource`, which reads recorded JSON files in a documented format. A `VirloSource` is out of this slice; it is added when the user supplies a key and the API's response format, and its first test is a recording of a real response. This is the one place the spec is knowingly less than P1's text.

## Evidence

- `edit_dna.json` already carries what the digest needs: cut times and shot lengths, silence gaps and pauses per minute, words per minute, integrated loudness, and `visual_events` (punch-ins, overlays) once `visual-motion` is merged. `reelctl diff` already compares two such files.
- planer.md's example finding ("Cut interval dropped from 3.9 s to 1.4 s; added 2-word karaoke captions + split-screen diagram in first 2.0 s") is a before/after comparison of measured fields. Captions and split-screens are not measured by `reelctl`, so the digest does not claim them; it reports what the fields say.

## Tech Stack

Go (go 1.27 is installed). Standard library only for the first slice (`encoding/json`, `os/exec`, `sort`, `text/template`). `reelctl` as an external process, path from `--reelctl` or `PATH`.

## Contract

### Input format (`FileSource`)

A directory with `niche.json`:

```json
{"niche": "neet-prep", "creators": [
  {"handle": "a", "snapshots": [{"date": "2026-10-21", "followers": 100000, "total_views": 5000000}],
   "posts": [{"id": "p1", "url": "...", "posted_at": "2026-10-20T10:00:00Z", "views": 250000, "media_path": "media/p1.mp4"}]}]}
```

`media_path` is relative to the directory and optional; a post without media cannot be decompiled and is listed as such. Unknown fields are ignored; a missing required field is an error naming the creator and the field.

### Calculations (all deterministic, no model)

- **Outlier multiplier** of a post: `views / median(views of the 30 posts before it, by date)`. Fewer than 5 earlier posts: no multiplier, and the post is listed as `not_enough_history` with its count.
- **Growth velocity** of a creator: the least-squares slope of followers and of total views over the last 7 days of snapshots, per day and as a share of the first value. Fewer than 3 snapshots in 7 days: not computed, with the count.
- **Surging creator:** has a post with multiplier at least `--min-score` (default 10), or velocity above `--min-velocity` (default 2% of followers per day).

### Actions

- For each post at or above `--min-score` with media: run `reelctl decompile --input <media> --out <out>/dna/<id>.edit_dna.json`; the exit code and stderr are kept in the digest on failure, and one failure does not stop the run.
- For each surging creator with at least 2 decompiled breakouts and at least 2 decompiled baseline posts (posts under 2× the median, oldest first): compare the **last 5 breakouts** with the **baseline posts** on measured fields: mean and median shot length, cuts per minute, pauses per minute, silent share, words per minute, integrated LUFS, visual events per minute by kind. Each metric appears as `before → after` with its change, and a sentence is written only for a change of at least 25% (a fixed constant, reported with the numbers): "Mean shot length dropped from 3.9 s to 1.4 s (−64%) across 5 breakouts against 4 baseline posts."

### Output

`<out>/digest.json` and `<out>/digest.md`. The JSON has `generated_at` (from `--now`, default the clock; tests pass a fixed value), `niche`, `creators[]` (handle, multiplier of best post, velocity, surging, breakouts, baseline count, `blueprint` metrics and sentences, `skipped` reasons), and `failures[]`. The Markdown is the human version of the same data. Both are byte-identical for identical inputs and `--now`.

### Command

```
watchtower run --data <dir> --out <dir> [--reelctl <path>] [--min-score 10] [--min-velocity 0.02] [--now <rfc3339>]
```

Exit 0 with a digest even when some decompiles fail; exit 2 for a bad argument or an unreadable data directory; exit 1 for an internal error.

## Boundaries

- **Always:** report only measured numbers, with the counts they rest on; say what could not be computed and why.
- **Ask first:** adding a dependency outside the standard library; a network client.
- **Never:** call the network in tests; guess a Virlo response format; scrape; claim captions or layouts that `edit_dna.json` does not measure.

## Testing Strategy

- **Unit tests (Go, offline):** the median and multiplier on hand-made series (ties, even counts, fewer than 5 posts); slope on a known line and with noise; the surging rule at its boundaries; the 25% sentence threshold at 24% and 25%; the format parser (missing field names the creator and field).
- **Integration:** a test writes a temporary `reelctl` stub (a shell script that writes a given `edit_dna.json` to `--out`, one per media file name) and runs `watchtower run` on a recorded data directory in `watchtower/testdata/`: the digest equals a golden file; a failing stub for one post is listed and the rest proceed; `--now` makes it byte-identical on a rerun.
- **Real `reelctl`:** one run with the real binary on `fixtures/generated` clips (made by `fixtures/generate.sh`) when the binary exists, recorded in `status.md`; the Go test suite does not depend on it.

## Success Criteria

1. `go test ./...` passes with no network and no `reelctl` binary.
2. The recorded data directory produces a digest whose surging creators, multipliers and blueprint sentences match hand-computed values.
3. One run through the real `reelctl` on fixture clips produces a digest, recorded in `status.md`.
4. README, `docs/status.md`, a new `docs/watchtower.md` (input format, flags, digest fields), the capability map and CHANGELOG match; the Go module is listed in AGENTS.md.
5. The `VirloSource` gap is stated in `status.md`.

## Resolved Questions

1. **File source first; Virlo later, from a real recording.**
2. **Separate Go module, `reelctl` as a process.** No FFI, no shared types.
3. **Only measured claims in the blueprint.**
