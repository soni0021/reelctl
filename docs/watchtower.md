# watchtower

`watchtower` reads posts and follower snapshots for a niche, finds the creators who are surging, decompiles their breakout Reels with `reelctl decompile`, and writes a daily digest ("Why They Grew + Exact Edit Blueprint"). Every number in the digest is measured. The spec is [specs/watchtower.md](specs/watchtower.md).

It is a Go module in `watchtower/`, separate from the Cargo workspace. It uses only the standard library and calls `reelctl` as a process.

## Run it

```
cd watchtower
go build -o watchtower ./cmd/watchtower
./watchtower run --data <dir> --out <dir> [--reelctl <path>] [--min-score 10] [--min-velocity 0.02] [--reelctl-timeout 5m] [--now <rfc3339>]
```

The `watchtower` binary that `go build` leaves in `watchtower/` is ignored by `watchtower/.gitignore`.

| Flag | Default | Meaning |
|---|---|---|
| `--data <dir>` | required | Directory with `niche.json` |
| `--out <dir>` | required | Where `digest.json`, `digest.md` and `dna/` go; created if missing |
| `--reelctl <path>` | `reelctl` | The `reelctl` binary, a path or a name on `PATH` |
| `--min-score <n>` | `10` | Outlier multiplier at which a post is a breakout. Must be a finite number above 0 (`NaN` and `Inf` are refused) |
| `--min-velocity <n>` | `0.02` | Follower growth per day, as a share of followers, above which a creator surges. Must be a finite number, not negative |
| `--reelctl-timeout <d>` | `5m` | The longest one `reelctl decompile` may run, a Go duration such as `90s`. A run past it is killed and listed in `failures` with cause `timed_out`. Must be above 0 |
| `--now <rfc3339>` | the clock | The `generated_at` value. Fix it to get byte-identical output |

Exit codes: 0 for a digest, also when some decompiles failed; 2 for a bad argument, an unreadable data directory, or a `niche.json` that is invalid; 1 for an internal error such as an output directory that cannot be made.

A run keeps at most the first 2,000 bytes of `reelctl`'s stderr; the rest is read and dropped, so a chatty `reelctl` cannot fill memory.

On success it prints one line: `watchtower: 4 creators, 2 surging, 0 failures; wrote digest.json and digest.md`.

The tests need neither `reelctl` nor a network: `cd watchtower && go test ./...`. They run a stub `reelctl` shell script against the recorded data in `watchtower/testdata/`.

## Input: `niche.json`

```json
{"niche": "neet-prep", "creators": [
  {"handle": "a",
   "snapshots": [{"date": "2026-10-21", "followers": 100000, "total_views": 5000000}],
   "posts": [{"id": "p1", "url": "https://…", "posted_at": "2026-10-20T10:00:00Z", "views": 250000, "media_path": "media/p1.mp4"}]}
]}
```

| Field | Required | Notes |
|---|---|---|
| `niche` | yes | Name shown in the digest |
| `creators[].handle` | yes | |
| `snapshots[].date` | yes | `YYYY-MM-DD` |
| `snapshots[].followers`, `total_views` | yes | Integers, not negative |
| `posts[].id` | yes | Not empty; only letters, digits, `.`, `_` and `-`; no `..` and not only dots. Unique across the whole file, ignoring case (a case-insensitive disk would give `P1` and `p1` one file). It names the file `dna/<id>.edit_dna.json`, so anything else is refused, not rewritten |
| `posts[].posted_at` | yes | RFC 3339 |
| `posts[].views` | yes | Integer, not negative |
| `posts[].url` | no | Copied to the digest |
| `posts[].media_path` | no | Relative to the data directory, and it must stay inside it: no absolute path, no `..` element left after cleaning, no leading `-`, and the nearest existing part of the path must not resolve outside the directory through a symlink. A post without one cannot be decompiled. `reelctl` is always given the absolute path `<data dir>/<cleaned media_path>` |

Unknown fields are ignored. A missing required field, or a value that is refused, is an error that names the creator, the post and the field, for example `creator "a": post "p1": missing field views` or `creator "b": post "p1": duplicate id (ignoring case), already used by creator "a"`. The file is not used at all then: exit 2, no digest.

The data comes through a `Source` interface (`internal/niche`). The only implementation is `FileSource`.

## What it calculates

All of it is deterministic. No model is called.

- **Outlier multiplier** of a post: its views divided by the median views of the (up to) 30 posts before it, ordered by `posted_at` then `id`. With fewer than 5 earlier posts there is no multiplier, and the post is listed as `not_enough_history` with its count. A median of zero is listed as `zero_median`.
- **Growth velocity:** the least-squares slope of followers, and of total views, over the snapshots in the 7 days that end on the creator's latest snapshot. A snapshot exactly 7 days before the latest is out. The slope is per day, and also a share of the first value in the window. With fewer than 3 snapshots in the window, velocity is not computed (`velocity_not_computed`, with the count); the same when all snapshots in the window share one date, which the detail says in those words (there is no time span to fit). The two growths are fitted on their own: a zero first value in `total_views` leaves `views_per_day` and `views_share_per_day` as `null` (with a `velocity_not_computed` skip saying so) and keeps the follower velocity; a zero first follower count leaves `velocity` as `null`, because the surging rule rests on it.
- **Surging:** a post with a multiplier of at least `--min-score`, or follower share per day strictly above `--min-velocity`. `surging_by` lists which rule fired. Total-views growth is reported but does not decide.
- **Breakout:** a post with a multiplier of at least `--min-score`. **Baseline post:** any other post with a multiplier under 2.

For each surging creator it decompiles every breakout and every baseline post that has media, in date order. A failed run is recorded in `failures` and the run goes on. A post without media is listed as `no_media`.

A creator gets a blueprint when at least 2 breakouts and at least 2 baseline posts were decompiled. It compares the last 5 decompiled breakouts (by `posted_at`) with the decompiled baseline posts. Otherwise `blueprint_not_computed` says how many there were.

### Choices the spec left open

- Baseline posts are decompiled too, for surging creators. The spec asks for at least 2 decompiled baseline posts, which cannot happen otherwise.
- A group's value for a metric is the mean of the per-reel values, so every reel counts once whatever its length. "Median shot length" is the mean of each reel's median.
- Change is `(after - before) / |before|`. With a baseline of zero there is no percentage, and a sentence is written when the breakout value is not zero. For loudness the sign of the change is the sign of the LUFS number, not "louder": −16.0 to −10.0 LUFS is "rose", +38%.
- Floats in the digest are rounded to four decimals, so the output does not depend on the last bits of a sum. The 25% test uses the unrounded values.

### Blueprint metrics

Read from each `edit_dna.json`: `schema_version`, `duration_ms`, `cuts[].time_ms`, `silence_gaps[].duration_ms`, `words_per_minute`, `asr_backend`, `integrated_lufs`, and `visual_events[].kind`. Nothing else is read.

A file the blueprint cannot trust is a failure for that post (stage `read_edit_dna`), never a zero: `schema_version` must be 1, 2, 3 or 4 (4 is written now: 3 added `visual_events`, 4 added `zoom_curves`, which the watchtower does not read; absent or any other value is refused); `duration_ms` must be above 0; and `duration_ms`, `cuts`, `silence_gaps`, `words_per_minute`, `integrated_lufs`, every `cuts[].time_ms` and every `silence_gaps[].duration_ms` must be present and not `null`. An absent `visual_events` is still "not measured".

| Metric key | How it is measured |
|---|---|
| `mean_shot_length_ms`, `median_shot_length_ms` | Shots run from 0 to the first cut, between cuts, and from the last cut to `duration_ms`. A cut at 0 or at or after `duration_ms`, and a repeated cut time, is ignored |
| `cuts_per_minute` | Cuts counted that way, over `duration_ms` in minutes |
| `pauses_per_minute` | Silence gaps of at least 180 ms, over minutes |
| `silent_share` | Total of all silence gaps over `duration_ms` |
| `words_per_minute` | As in the file, only when every file in the comparison has `asr_backend` `whisper_cpp`; it is a speaking rate then, and the label is "Words per minute" |
| `voiced_bursts_per_minute` | The same field when every file has `asr_backend` `silence_bursts`. Then it counts bursts of sound, not words (docs/edit-dna.md), so the label is "Voiced bursts per minute". If the files disagree, in either group, or one lacks `asr_backend`, neither metric is compared and a note in `blueprint.notes` says how the files split, for example `asr_backend cannot be compared across the files (2 silence_bursts, 2 whisper_cpp), so the speech rate is not compared: a voiced-burst rate is not a speaking rate.` |
| `integrated_lufs` | As in the file |
| `visual_events_per_minute.<kind>` | Events of that kind over minutes, one metric per kind, kinds sorted |

A sentence is written only for a change of at least 25% (a fixed constant, reported as `change_threshold`): "Mean shot length dropped from 3.9 s to 1.4 s (−64%) across 5 breakouts against 4 baseline posts." The digest does not claim captions, layouts, or anything else `edit_dna.json` does not measure.

`visual_events` is absent from older `edit_dna.json` files, which means not measured, and an empty list means measured and none. If any file in the comparison lacks it, visual events are left out and a note says how many files lacked it. The field's shape (`kind`: `punch_in`, `zoom_out` or `overlay_change`) comes from the `visual-motion` branch; the `edit-dna` crate on this branch does not have it yet.

## Output

`digest.json` and `digest.md` hold the same data. In the Markdown, each metric is a row of `baseline → breakouts` with its change. Both are byte-identical for identical inputs and `--now`. They hold no path of the machine that made them, except where a `failures[].detail` quotes a message from `reelctl` or the OS, which may name a path. `dna/<id>.edit_dna.json` holds each decompiled post, and `reelctl` also writes its `timeline_view` PNGs in a directory of its own for each post, `dna/<id>.edit_dna_timeline/`, so two posts never share a PNG name.

Top level of `digest.json`: `generated_at`, `niche`, `min_score`, `min_velocity`, `creators[]`, `failures[]`.

Creators are listed surging first, then by best multiplier, then by follower growth, then by handle.

| `creators[]` field | Meaning |
|---|---|
| `handle` | |
| `surging`, `surging_by` | Whether, and by `multiplier` and/or `velocity` |
| `best_post_id`, `best_multiplier` | The post with the highest multiplier (the earlier post when two tie); both are `null`, not absent, when no multiplier could be computed |
| `velocity` | `snapshots`, `followers_per_day`, `followers_share_per_day`, `views_per_day`, `views_share_per_day`; the whole object is `null` when the follower growth was not computed, and the two `views_*` fields are `null` when only total-views growth was not |
| `breakouts[]` | `id`, `url`, `posted_at`, `views`, `multiplier`, `edit_dna` (path relative to `--out`, absent if not decompiled) |
| `baseline_posts` | How many of the creator's posts have a multiplier under 2×. A post with no multiplier (not enough history, or a zero median) is not counted, and neither is a post between 2× and `--min-score`. This is a count of posts, decompiled or not, and it is filled for creators who are not surging; `blueprint.baseline_posts` is how many of them were decompiled and compared |
| `blueprint` | `breakouts`, `baseline_posts`, `change_threshold`, `metrics[]` (`key`, `label`, `unit`, `before`, `after`, `change`), `sentences[]`, `notes[]`; `null` when not computed. `before` is the baseline posts and `after` the breakouts |
| `skipped[]` | `post` (when about one post), `reason`, `detail`. Reasons: `not_enough_history`, `zero_median`, `velocity_not_computed`, `no_media`, `blueprint_not_computed` |

`failures[]`: `creator`, `post`, `stage` (`decompile` or `read_edit_dna`), `cause`, `exit_code`, `signal`, `detail`.

- `cause` (stage `decompile` only) says how `reelctl` ended: `exited` (it ran and returned a non-zero code; `exit_code` is set), `not_started` (the binary could not be started, or an earlier output file could not be removed), `signaled` (a signal ended it, such as a kill or an out-of-memory kill; `signal` names it, for example `killed`) or `timed_out` (it ran past `--reelctl-timeout` and was killed).
- `exit_code` is present only for `cause` `exited`. It is never `-1`: the old convention made a killed process look like one that never started.
- `detail` is the first 2,000 bytes of stderr, or the reason the file could not be used. For `read_edit_dna` it is `reelctl exited successfully but wrote no edit_dna.json` when `reelctl` returned 0 and left no file. The watchtower deletes `dna/<id>.edit_dna.json` before each run, so a file from an earlier run is never mistaken for this run's output.

## The Virlo gap

Planer.md names Virlo's API (`POST /v1/agents`, `POST /v1/tracking/creators`) as the data source. It is not built. There is no key in `.env` and its response format is not in the repo, and the watchtower does not guess one. A `VirloSource` will be a second implementation of `Source`. Its first test should be a recording of a real response. Until then the watchtower runs on recorded or hand-made `niche.json` files, and nothing here has been checked against live data.
