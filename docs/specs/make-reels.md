# Spec: One long video in, finished reels out (`make-reels`)

Status: **approved 2026-10-29** by the user ("Approve, start coding"), with two answers that changed the draft: captions get "many options to customize" (see 5), and the reel count is "custom + picked" (see 6). Asked for on 2026-10-29: "if I give one input video, will it cut reels in a famous reel's pattern and give a final output?", with the answers "build it first" (before the studio service and the Mac app) and "follow the presenter" for the 9:16 crop. Branch: `claude/keen-clarke-1a8w5h`. Implements planer.md's P2 success metric (line 272: "a raw talking-head take or 30-min YouTube video + a competitor outlier … a finished 9:16 informational Reel … with a long-form upsell bridge"), apart from manimgx visuals, which stay a manual `overlay add-chart` step.

## Objective

A creator has one long informational video (a 20–60 minute lecture or a raw talking-head take) and one to three outlier reels already decompiled. They want 3–5 finished vertical reels in one command. Each reel should:
- open on a hook
- have no dead air
- be paced like the outliers
- be framed on the presenter
- have word captions
- end on an open loop with a card that sends viewers to the long video

Every step except one already exists as its own command:

| Step | Command | Status |
|---|---|---|
| Measure cuts, pauses, timed words | `decompile` with a whisper model | built |
| Pick hook → payoff → open-loop clips | `understand` (paid; about $0.01 per lecture with Qwen3.5-9B) | built; writes `understanding.json` |
| A project from one clip pick | `timeline from-clip` | **new** |
| Remove dead air and fillers | `timeline cut-silence` | built |
| Punch-ins where the clip stays still too long | `transform auto-punch-in` | **new** (manual `punch-in` is built) |
| 9:16 crop that follows the presenter | `transform reframe` | **new** |
| Upsell card at the end | `funnel attach-bridge --append` | **flag is new** |
| Burned-in word captions | `overlay captions` | **new** |
| Render | `render` | built; gains the moving crop and captions |
| Check each reel against the outliers | `diff` | built |
| All of the above in order | `reelctl make-reels` | **new** |

## Tech Stack

Rust, inside the existing crates:
- `crates/timeline`: `from_clip.rs`, `auto_punch.rs`, `reframe.rs`, `captions.rs`, plus changes to `render.rs` and `bridge.rs`.
- `crates/compositor`: a new alpha-only entry point.
- `crates/reelctl/src`: `make_reels_cmd.rs`, with the new subcommands added to `timeline_cmd.rs` and `overlay_cmd.rs`.

No new dependencies.

## Design

### 1. `timeline from-clip`

```
reelctl timeline from-clip --input <long.mp4> --dna <edit_dna.json> --understanding <understanding.json> --clip <N> --out <clip.otio> [--allow-flagged]
```

- `N` counts from 1, in the order of `understanding.json`'s `clips` array, which is the model's ranking.
- It makes the same project `timeline init` makes, with the video clip's `source_range` set to the clip's `start_ms..end_ms`, snapped outward to whole frames. The DNA reference is recorded as `init` records it, plus `clip: {understanding: <path>, index: N, hook_id, last_id}` under `metadata.reelctl`.
- It refuses, with exit 2:
  - a clip whose `flags` is not empty, unless `--allow-flagged` is given
  - an `N` out of range
  - an understanding file whose `source.path` and duration don't match the DNA
- It runs `ffprobe` on the input, as `init` does.
- `cut-silence`, `punch-in` and the other edit operations must work on a project whose clip starts after 0. **Every edit operation gets a test on such a project.** Today they are tested only on `init` output, which starts at 0.

### 2. `transform auto-punch-in`

```
reelctl transform auto-punch-in --project <clip.otio> --dna <edit_dna.json> --outlier <edit_dna.json>... --out <clip.otio>
```

- **Where**: it finds each static window in the edited timeline. It uses `diff`'s rule from `crates/diff`: more than 2.5 s with no cut and no `visual_events` entry, with cuts and events mapped into timeline time through the project's edits. Each punch-in starts 1.0 s into its window, or at the next word start after that, and lasts until the window ends or 2.0 s, whichever is first. It always leaves at least 12 frames, `punch-in`'s minimum.
- **How much**: the scale is the median `scale` of the outliers' punch-in `visual_events`, clamped to 1.08–1.35. With none, it is 1.15.
- **Easing**: the curve is the median of the outliers' `zoom_curves` y1/y2. With none, it is today's `punch-in` curve.
- It is deterministic, and a second run on its own output finds no windows and writes the same bytes. Overlays are refused, as for `punch-in`.
- stdout: `auto-punch-in: out= windows= punches= scale= curve=measured|default`

### 3. `transform reframe` (follows the presenter)

```
reelctl transform reframe --project <clip.otio> --aspect 9:16 --follow presenter|centre [--weights <rvm.onnx>] [--downsample-ratio R] --out <clip.otio>
```

- **The crop window** is the largest 9:16 rectangle that fits the source, full height on a 16:9 source. Only its x position moves. A source that is already 9:16 or narrower is left unchanged (`unchanged=true`).
- **`--follow presenter`** has two halves (see [ADR-025](../decisions/ADR-025-reframe-on-the-presenter-from-the-rvm-alpha-centroid.md)):
  - **Measure.** It runs RobustVideoMatting through `crates/compositor` over the clip's source frames. It reads every frame at the downsample ratio, 0.25 by default, and keeps only the alpha. No video is written.
  - **Plan.** Per frame it takes the alpha-weighted horizontal centre of mass, ignoring pixels under alpha 0.5. A frame with too little foreground (alpha mass under 1% of the frame) holds the previous position. The path is then smoothed:
    - **dead zone**: no move while the presenter stays within the middle 20% of the crop
    - **maximum pan speed**: 30% of the crop's width per second
    - **ease**: ease-in and ease-out over 250 ms
    - **cuts**: at each cut in the DNA the position jumps to the new shot's settled centre instead of panning
    - **edges**: the window never leaves the frame
  - **Stored.** The result is up to one keyframe every 500 ms plus one at each cut. It is stored under `metadata.reelctl.reframe` as `{aspect, follow, keyframes: [{source_ms, x}], note}`, where `x` is the window's left edge as a fraction of the source width.
- **`--follow centre`** is one keyframe at the centre.
- **No model.** Without `--weights` (or `REELCTL_RVM_WEIGHTS`) or a valid absolute `ORT_DYLIB_PATH`, `--follow presenter` exits 2 and names what is missing, with the same messages and order as `compositor isolate-presenter`. It never silently falls back. The fallback lives in `make-reels`, which records why in its note.
- **ONNX.** The process loads only the RVM model and calls `compositor::release_runtime()` before it exits, as `main` already does.
- **SDR only.** An HDR source is refused (exit 2), as the compositor already does.
- **Render.** `render` turns the keyframes into an ffmpeg `crop` filter whose x position is a piecewise-linear expression in `t`, placed before punch-in scaling and the output-size fit. So `--size 1080x1920` gives a full-height, presenter-centred vertical frame. The Smelter backend refuses a reframed project with exit 2 and a message, for now.
- **Order.** Punch-ins zoom about the crop window's centre. `reframe` can run before or after `auto-punch-in`.
- stdout: `reframe: out= aspect= follow= keyframes= frames_measured= ms_per_frame= unchanged=`

### 4. `funnel attach-bridge --append`

- The clip already ends on the cliffhanger, so `make-reels` must not cut it again. `--append` adds the card after the last frame instead of cutting at `--cutoff-pct`. The two flags can't be used together (exit 2).
- The recorded bridge gets `cutoff_pct: null, appended: true`.
- A project written by the old code (with `cutoff_pct` and no `appended`) still reads.

### 5. `overlay captions`

```
reelctl overlay captions --project <clip.otio> --dna <edit_dna.json> [--style <preset>|<style.json>] [style flags] --out <clip.otio>
```

**Style.** A caption style is a set of fields. It starts from a preset and is then overridden by an optional JSON file (`--style my.json`), then by individual flags, in that order. The resolved style is stored in the caption track, so a render always looks the same as the run that made it.

| Field / flag | Values | Default (`bold` preset) |
|---|---|---|
| `--preset` | `bold`, `clean`, `boxed`, `karaoke`, `minimal` | `bold` |
| `--font` | path to a .ttf or .otf file | system bold sans (below) |
| `--font-size` | percent of output height, 2–12 | 5.5 |
| `--color` | `#rrggbb` | `#ffffff` |
| `--highlight-color` | `#rrggbb`, or `none` to turn the highlight off | `#ffd60a` |
| `--outline-color`, `--outline-width` | `#rrggbb`; 0–12 px at 1080 wide | `#000000`, 6 |
| `--shadow` | 0–12 px offset | 0 |
| `--box-color`, `--box-opacity` | `#rrggbb`; 0–1 (0 is no box) | `#000000`, 0 |
| `--position` | `top`, `middle`, `bottom` | `bottom` |
| `--margin` | percent of output height from that edge, 0–40 | 22 (clear of the app's caption and button area) |
| `--words-per-group` | 1–5 | 3 |
| `--max-chars` | 8–40 per line; a longer group wraps to two lines | 18 |
| `--case` | `as-is`, `upper`, `lower` | `upper` |
| `--min-gap-ms` | a pause that always starts a new group, 100–2000 | 400 |

The presets differ only in these fields:

| Preset | What it changes |
|---|---|
| `clean` | white, no highlight, thin outline, as-is case |
| `boxed` | white on a 60% black box, no outline |
| `karaoke` | one word at a time, large, highlight on |
| `minimal` | small, white, `--position bottom --margin 10` |

An unknown field in the JSON file, or a value out of range, exits 2 with the field's name. `reelctl overlay captions --list-presets` prints each preset's resolved fields as JSON, so the Mac app can show them.

- **Words** come from the DNA's whisper `words`. The DNA must have `asr_backend` `whisper_cpp`, otherwise exit 2, because `burst-N` placeholders are not captions.
- **Mapping.** Each word is mapped from source time to timeline time through the project's edits:
  - a word inside a removed span is dropped
  - a word cut in two by an edit is kept on the side with most of its length
  - words after the bridge cut, or on the card, are dropped
- **Groups.** Groups of `--words-per-group` words are shown from the first word's start to the last word's end. A group never crosses a cut in the timeline or a pause of `--min-gap-ms` or more. Unless the highlight is `none`, the current word is drawn in the highlight colour.
- **Stored.** The groups are a caption track (`kind: reelctl.captions`) with one item per group.
- **Edits after captions.** `cut-silence`, `punch-in`, `auto-punch-in`, `attach-bridge` and `reframe` refuse a project with captions, as they refuse overlays (exit 2, "add captions last"). `overlay remove --captions` takes them out.
- **Render.** `render` draws them with `drawtext`, one filter per group, with `enable='between(t,a,b)'`. They are placed in the lower third of the output frame, which is the reframed vertical frame. Text is escaped for the filter.
- **Font.** No font is bundled or downloaded. The default is the first that exists of `/System/Library/Fonts/Supplemental/Arial Bold.ttf` (macOS) and `/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf` (Linux). `--font` overrides it. If none is found, exit 2 with the paths that were tried.
- **Smelter.** It refuses a captioned project with exit 2, for now.
- stdout: `captions: out= groups= words= dropped= font=`

### 6. `reelctl make-reels`

```
reelctl make-reels --input <long.mp4> --outlier <edit_dna.json>... --target-url <url> --out-dir <dir>
  [--whisper-model <ggml.bin>] [--weights <rvm.onnx>] [--follow presenter|centre]
  [--max-cost-usd 0.50] [--yes] [--reels N] [--clips 1,3,...] [--pick] [--size 1080x1920] [--allow-flagged] [--dna <edit_dna.json>]
  [caption style flags, as for overlay captions]
```

It runs each step as a child `reelctl` process from the same executable. Each ONNX model then gets its own process, and a step's failure is contained to that step. The environment passed to each child holds only what that step needs: `OPENROUTER_API_KEY` reaches `understand` only, and `REELCTL_RVM_WEIGHTS` and `ORT_DYLIB_PATH` reach `reframe` only.

1. **Decompile.** `decompile` with whisper writes `<out-dir>/source.edit_dna.json`. A whisper model is required, because clips and captions need words. Without one it exits 2 before decoding, naming `--whisper-model` and `REELCTL_WHISPER_MODEL`. `--dna` skips this step when a DNA with whisper words already exists.
2. **Plan the cost.** `understand --dry-run` prints the plan line. If its `cost_upper_bound_usd` is over `--max-cost-usd`, `make-reels` exits 4 with the bound.
3. **Ask.** Unless `--yes` is given, it asks on the terminal: `understand will send N frames and the transcript to <model> and cost at most $X. Continue? [y/N]`. Anything but `y` exits 4. With no terminal and no `--yes`, it exits 4. This keeps AGENTS.md's rule that every live call needs the user's yes.
4. **Understand.** It runs `understand`, which writes `<out-dir>/understanding.json`.
5. **Each clip.** Which clips become reels ("custom + picked"):
   - By default, every clip the model picked that passes the clip rules, in the model's order.
   - `--reels N` keeps the first N of those.
   - `--clips 1,3` names the clips by their index in `understanding.json` instead. A flagged clip named here still needs `--allow-flagged`.
   - `make-reels` prints the chosen list (index, hook text, tight length, flags) before rendering.
   - `--pick` stops after `understand`. It writes the clip list to `make_reels.json` with `status: "picked"` and exits 0, so you can choose and then re-run with `--clips` and `--dna`. The understanding is reused and the model is not called again.

   For each chosen clip, working in `<out-dir>/reel-<N>/`, it runs:
   1. `from-clip`
   2. `cut-silence`
   3. `auto-punch-in` against the outliers
   4. `reframe --follow presenter`, falling back to `centre` with the reason in the note when the model is missing
   5. `attach-bridge --append`
   6. `captions` with the run's caption style
   7. `render --size <size>`, writing `reel-<N>.mp4`
   8. `decompile` of the render, then `diff` of the render against the outliers, writing `reel-<N>.diff.json`

   A failed step marks that reel failed with the step and its stderr tail, and goes on to the next reel.
6. **Summary.** It writes `<out-dir>/make_reels.json` with:
   - `schema_version: 1`
   - the source, the outliers, the model and `cost_usd`
   - each reel's `index`, `status`, `path`, `length_ms`, `hook` and `last` text, `diff_flags`, its `notes` (fallbacks and the flags it was allowed with), and `steps` (each step's argv without secrets, exit code and `ms`)
7. **Result.** stdout: `make-reels: out_dir= reels= ok= failed= cost_usd= follow=presenter|centre`. The exit code is 0 if at least one reel rendered, 1 if none did, and 2, 3 or 4 as for the step that refused.

Each step's own files are kept in `reel-<N>/` (`clip.otio` after each step as `NN-<step>.otio`, and the render), so any step can be re-run by hand with `reelctl` on the same files. That copy of every intermediate project is what the studio service later turns into revertible checkpoints.

## Boundaries

- **Always:**
  - Measured times only. The model supplies segment IDs; every cut, caption and crop time comes from whisper, `reelctl` or a frame.
  - One live `understand` call per run, after a yes.
  - The key is never written, printed or put in `make_reels.json`.
  - Synthetic fixtures in tests.
- **Ask first:** changing the clip rules, the default reel count, or anything in `understanding.json`'s schema.
- **Never:**
  - Download or bundle a model, ONNX Runtime or a font.
  - Call a model in tests.
  - Track faces, or identify or label people. The reframe follows the matte's mass, not a face.
  - Copy footage from an outlier: outliers contribute measured numbers only.

## Testing Strategy

- **Pure unit tests** in `crates/timeline`:
  - `from_clip`: frame snapping, metadata, the refusals.
  - Every existing edit on a clip that starts after 0.
  - `auto_punch`: window finding through edits, scale and curve from synthetic outlier DNAs, idempotence.
  - `reframe` planning:
    - centroid with a threshold, low mass holding the position, dead zone, speed limit, ease, a jump at a cut, edge clamping, keyframe thinning
    - inputs are synthetic alpha frames: a box moving left to right, a still box, an empty frame, a cut
  - The crop expression builder: values at keyframes and between them, compared with ffmpeg's evaluator on a test clip.
  - `captions`: mapping through removed spans, split words, the bridge, grouping at cuts and pauses, `drawtext` escaping of `'`, `:`, `%` and `\`.
  - `bridge --append`, and reading an old bridge record.
- **Compositor:** the alpha-only entry point runs with the existing fake `MattingModel`. Tests check that it reads every frame, carries the state, uses the ratio, and writes no file.
- **End to end** in `crates/reelctl/tests/make_reels_fixture.rs`:
  - A new `generate.sh` clip, `lecture90`: 90 s, 1280×720, a bright moving box as the "presenter" on a flat background, a planted cut, planted silences, and a tone. Its whisper words come from a checked-in DNA sidecar, because the cloud has no whisper model; `--dna` skips the decompile step.
  - `understand` runs with `--replay` and a checked-in reply, so no model is called.
  - `reframe` falls back to `centre`, since there is no RVM model in the cloud, and the note says so.
  - The test checks that two reels render, each 1080×1920, with the expected lengths. The card is at the end, captions are present (a `drawtext` in the logged filter graph), and `diff` of each render has no dead-air flag. It also checks `make_reels.json`'s fields, and that a second run writes identical `.otio` files.
  - The environment is stripped as AGENTS.md requires.
- **Manual, on the Mac** (the user): a real lecture from `Reels/` with the Apex whisper model, RVM weights and one live `understand` call after a yes. Look at whether the crop keeps the presenter in frame through the cuts, and record the ms per frame of the reframe measure in `docs/status.md`.

## Success Criteria

1. `make-reels` on `lecture90` in the cloud gives N 1080×1920 reels with captions and the card at the end, and a complete `make_reels.json`.
2. Each existing edit operation gives the same result on a clip that starts after 0 as on one that starts at 0, apart from the offset.
3. The `reframe` plan keeps a synthetic moving box inside the middle 40% of the crop on every frame, never exceeds the speed limit, and jumps at cuts.
4. `auto-punch-in` leaves the rendered reel with no static-window flag from `diff`.
5. No step runs `understand` live without a yes. Tests prove that exit 4 happens before any request.
6. Manual: on one real lecture the presenter stays in frame and the user approves the look.

## Resolved Questions

1. Captions' look: "many options to customize". Presets plus a style file plus flags, as in section 5.
2. Number of reels: "custom + picked". By default every passing pick; `--reels`, `--clips` and `--pick` choose, as in section 6. `understand` is only re-called if `understanding.json` is missing.
3. A `.otio` draft as input instead of a long video: not in this slice.
