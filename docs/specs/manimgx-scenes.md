# Spec: manimgx chart overlays (`manimgx-scenes`, P2 slice)

Status: built (see [status](../status.md#manimgx-chart-overlays-2026-10-29)); written 2026-10-29 under the user's "p1-p3"; the user has not reviewed it line by line. Branch: `feature/manimgx-scenes`. Follows [planer.md](../../planer.md) (Components 4 to 5 and the P2 row; `manimgx inspect` as the agent layout check) and builds on [timeline-editor](timeline-editor.md) and [ADR-019](../decisions/ADR-019-edit-an-opentimelineio-project-with-our-own-subset-and-render-with-ffmpeg.md). The decision record is [ADR-022](../decisions/ADR-022-draw-chart-overlays-with-a-separately-installed-manimgx-and-key-them-over-the-video.md). The plan is [manimgx-scenes-plan.md](manimgx-scenes-plan.md).

## What manimgx turned out to be (step 0, 2026-10-29)

Read from github.com/academa-labs/manimgx, `manimgx.academa.ai/llms.txt` and its reference pages (the tool was not run: see "Not verified").

- It exists. "Blazingly fast 3D animation engine for agents, compatible with Manim CE's API", a Rust and wgpu renderer with a Python front end (PyO3). The code is MIT; the native wheels include x264 and are GPL-3.0-or-later as a whole; FFmpeg inside is LGPL-2.1-or-later.
- Install: `uv tool install --python 3.14 manimgx` (puts `manimgx` on PATH; `manimgx --version`), or `pip install manimgx` in a Python 3.13 or 3.14 environment. It needs Python 3.13 or 3.14 (a macOS system Python 3.9 does not work). It says not to install `manim`, ffmpeg or LaTeX: fonts, Typst and video encoding are bundled.
- Platforms and GPU: macOS, Linux, Windows. Metal on macOS, Direct3D 12 on Windows, Vulkan on Linux; with no GPU it falls back to the CPU (Linux wheels bundle Mesa lavapipe). A sandbox that hides the GPU prints "no GPU adapter".
- A scene is a Python file: `import manimgx as m`, a class that subclasses `m.Scene` (or `m.ThreeDScene`) with `construct(self)`.
- CLI (from the web man page and the reference; run `manimgx <command> --help` to confirm):
  - `manimgx render FILE [SCENE]` writes an MP4 with sound. `-o/--output` (path must end in `.mp4`), `-r/--resolution WxH` (even, positive), `--fps N` (integer), `--preset`, `--crf`. It also writes a storyboard PNG beside the video and an `.srt` when the scene speaks. Defaults 1920x1080 at 60 fps, from the scene's config unless set. The output is named after the scene class unless `-o` is given.
  - `manimgx inspect FILE [SCENE]` runs the scene without video, prints a timeline, checks 2D layout, writes an annotated storyboard PNG, and **exits 1 when it finds layout problems** (notes, such as a graph leaving the frame, do not count). `-t/--time` takes seconds or `end` (repeatable or comma-separated); `-o/--output` ends in `.png`. 3D scenes get no layout check.
  - Also `preview` and `present` (not used here), and `--version`.
- Config keys (set in code before the scene is built, for example `m.config.pixel_width, m.config.pixel_height = 1080, 1920`): `pixel_width`, `pixel_height`, `frame_rate`, `background_color` (default black), `background_opacity` (0 to 1, default 1).
- Charts: `m.BarChart(values, bar_names=None, y_range=None, x_length=None, y_length=None, bar_colors=..., ...)`; `m.Axes(x_range, y_range, x_length, y_length, ...)` with `axes.plot_line_graph(x_values, y_values, line_color=..., add_vertex_dots=...)`; `m.Text(text, font_size=..., color=...)` ("nothing is markup"); `m.Create`, `m.VGroup(...).arrange(m.DOWN, buff=...)`, `mobject.next_to(other, m.DOWN)`, `self.play(..., run_time=...)`, `self.wait(t)`. The charts page documents `BarChart` only; there is no line-chart class, so a line chart is `Axes` + `plot_line_graph`.

**It cannot give transparency as documented.** The only video output is an x264 MP4 (`yuv420p`), which has no alpha, and no page says `background_opacity` reaches the file. "Ask the renderer for a transparent video" therefore cannot be met on documented behaviour. This slice does not rely on it: the scene is drawn on a fixed key colour and `ffmpeg` keys that colour out (see Compositing). The antialiased edge of text is a limit of that choice (a faint fringe of the key colour), stated under Limits.

## Not verified

The real `manimgx` was **not installed or run** in this session: `which manimgx` finds nothing, and installing it (a `pip install` of a package that downloads and runs code) was refused by the permission system, so it was not worked around. Everything about its behaviour above is from its documentation. What is verified: the `reelctl` side against a **fake `manimgx`** (a shell script the tests write, which logs its arguments and renders a fixture MP4 with `ffmpeg`). The first run of the real tool is the by-hand recipe in [timeline.md](../timeline.md#checking-an-overlay-with-the-real-manimgx).

Assumptions about manimgx, each one a place a real run could disagree (A1 to A7):

| # | Assumption | If it fails |
|---|---|---|
| A1 | `m.config.background_color = "#ff00ff"` set at the top of the scene file is honoured by `render` | The background is black, the key finds nothing, the overlay is an opaque black card. The post-render check below (corner pixel must be the key colour) refuses it. |
| A2 | `-r WxH` and `--fps N` override the scene's config | The asset has another size or rate; the post-render ffprobe check refuses a size that differs. |
| A3 | `self.wait(t)` after the animations makes the video last `animation time + t` | The length check (within one frame) refuses it. |
| A4 | A scene file runs ordinary Python (`json`) | The render fails and the tool's stderr is shown. |
| A5 | `chart.bars[i]` is a mobject that `next_to` accepts (documented: `bars` is a `VGroup` of rectangles) | The render fails and the stderr is shown. |
| A6 | `inspect -t end` exits 0 on a clean scene and 1 on layout problems, and no other code means a clean scene | Exit 1 is reported as a layout problem and any other non-zero as a failure; `--skip-inspect` turns the step off. |
| A7 | A render may take tens of seconds on a GPU and longer on CPU | `--timeout-s` (default 300) kills it and exits 1. |

## Objective

An agent, or a person, says "show this number as a bar chart from 2 s to 6 s" with a typed JSON spec, and `reelctl` makes a chart overlay and composites it over the video, deterministically, without a model:

```
reelctl overlay add-chart --project edit.otio --spec chart.json --out edit.otio [--assets-dir DIR] [--manimgx PATH] [--skip-inspect] [--timeout-s 300]
reelctl overlay remove    --project edit.otio --id ID --out edit.otio
reelctl render --project edit.otio --out out.mp4        # composites the overlays
```

Each command reads one project and writes one, atomically, and is idempotent: `add-chart` twice with the same spec gives one overlay and the same project bytes, and the second run does not call the renderer. Name: `overlay` is a timeline edit like `transform` and `funnel`; planer.md's `compositor` group is the presenter-isolation slice (RobustVideoMatting), a different job. `add-chart` leaves room for `add-callout` or `add-image` later.

## The chart spec

A JSON object; unknown fields are refused (`deny_unknown_fields`); the file is at most 64 KiB and a regular local file (a `://` path is refused).

```json
{
  "kind": "bar",
  "title": "Reels posted per week",
  "series": [{"label": "Mon", "value": 3}, {"label": "Tue", "value": 5}],
  "unit": "%",
  "start_ms": 2000,
  "end_ms": 6000,
  "position": "center",
  "width_pct": 90
}
```

| Field | Rule |
|---|---|
| `kind` | `bar`, `line` or `number-callout`. |
| `title` | 1 to 60 characters. |
| `series` | `bar`: 1 to 8 points. `line`: 2 to 24, plotted left to right at equal spacing; only the first and last label are drawn. `number-callout`: exactly 1. A point is `{label, value}`; `label` 1 to 24 characters; `value` finite, 0 to 1e12 for `bar` and `line`, finite with magnitude at most 1e12 for the callout. |
| `unit` | Optional, 1 to 8 characters; appended to every shown value. |
| `start_ms`, `end_ms` | Milliseconds of the edited timeline, rounded to the nearest frame. The span is at least 500 ms and at most 30000 ms, and inside the timeline. |
| `position` | `top`, `center` (default) or `bottom`; centred horizontally. |
| `width_pct` | Width of the overlay as a percentage of the output width, 20 to 100 (default 90). |

Text (title, label, unit) may not contain control characters or bidirectional-format characters; it may contain anything else, including non-ASCII and quotes. Values are formatted by `reelctl` (at most 2 decimals, trailing zeros trimmed, no exponent), not by the scene.

## Contract

### Scene generation (no chart text becomes code)

The scene file is a **fixed template** with one hole. The hole holds the scene payload (derived from the canonical spec) **once**, as `json.loads("<the JSON text, encoded as a string>")`: the payload is serialised to JSON with every non-ASCII character as `\uXXXX`, and that text is serialised again as a string, which is a valid Python string literal. No other part of the file depends on user input, so a title that holds `"""`, `'`, `\`, a newline, `#`, `{}` or any non-ASCII text stays data. The template draws title and labels with `m.Text` only (never `Tex`, `MathTex` or `bar_names`, which typeset their input). Everything numeric the scene needs (`y_range`, the formatted value strings, the duration in seconds) is computed in Rust and carried in the same JSON. Canonical means a fixed field order and defaults filled in, so the same spec always gives the same text and the same overlay id.

The scene sets `m.config.background_color` to the key colour `#ff00ff` and uses an explicit palette that has no colour near it. The scene class is `ReelctlChart`. Its length is `duration_s`: the animation (about 1 s) then `self.wait(duration_s - animation)`.

### Running the worker (fail closed)

- Binary: `--manimgx PATH`, else `MANIMGX_BIN`, else `manimgx` on `PATH`. A value with `://` is refused (exit 2). It is started directly with an argument array, never through a shell. Not found: exit 1 with `uv tool install --python 3.14 manimgx`. Before any scene, `manimgx --version` must exit 0; otherwise exit 1 (the worker is broken).
- The whole run happens in a fresh scratch directory (the tool writes a storyboard PNG, maybe an `.srt`, beside its video); the scene, the logs and the video are inside it and the directory is removed afterwards. The child's stdin is null, `OPENROUTER_API_KEY` is removed from its environment, stdout and stderr go to files, and it is killed after `--timeout-s`.
- Inspect (unless `--skip-inspect`): `manimgx inspect <scene.py> ReelctlChart -t end -o <scratch>/inspect.png`. Exit 0: go on. Exit 1: layout problems; the command refuses (exit 2: the spec does not fit), prints the tool's report, and keeps the annotated PNG at `<assets-dir>/<id>.inspect.png`. Any other non-zero exit or a timeout: exit 1.
- Render: `manimgx render <scene.py> ReelctlChart -o <scratch>/scene.mp4 -r WxH --fps N`. `WxH` is `width_pct` of the source width rounded to an even number by 4:3; `--fps` is the project rate rounded to an integer and clamped to 1 to 60, so 29.97 renders at 30 and the render graph retimes it. Only these flags are used.
- After the render, the video is checked with `ffprobe`: a regular file, at most 256 MiB, with a video stream, the size that was asked for, and a length within one frame of the span; the top-left pixel must be the key colour (A1). Any failure: exit 1, nothing written to the project.
- The video is copied to `<assets-dir>/<id>.mp4` (default `<project stem>.overlays/` beside the project) through a temporary file and a rename.

`SceneRenderer` is the trait the command calls (`version`, `inspect`, `render`); `ManimgxCli` implements it with the process above. Tests use a fake script for the process and a Rust fake for the trait.

### The OTIO overlay track

A second `Track.1` of kind `Video` named `reelctl overlays`, marked `metadata.reelctl.overlays: true` (the marker, not the name, identifies it). Its children are `Gap.1` and `Clip.2` in time order: a gap up to the span's start, then a clip whose `source_range` is the span in frames and whose `ExternalReference.1` is the rendered MP4 (absolute path, `available_range` from the probe). The clip's `metadata.reelctl.overlay` holds `{id, spec, position, width_pct, key_color, renderer, asset: {width, height, frames, fps}}`. Overlays may not overlap in time (a different overlay on frames already taken is refused, exit 2, before anything is rendered). The track is rebuilt from the list of overlays on every change, so adding and removing leave no stray gaps. The `id` is 16 hex digits of FNV-1a (64 bit) over the canonical spec text; it names the file and is how `remove` finds the overlay.

Every function that walks the tracks skips the overlay track (`Timeline::track`, `check_editable`, `single_media`, `media_info`). `cut-silence`, `punch-in` and `attach-bridge` **refuse** a project with overlays (exit 2, "add overlays last"): they split and move the main tracks, and an overlay placed in timeline time would drift. Order: cut, zoom, bridge, then overlays.

### Compositing (`render`)

The video of the project is concatenated as before. When there are overlays, the concat output goes through one `overlay` filter per overlay, in time order, each fed by its own `-i <file>` (with `-protocol_whitelist file`, as the media are; never through `movie=`):

```
[k:v]fps=R,settb=expr=1/R,setpts=N+START,colorkey=0xff00ff:SIM:BLEND,scale=W:-2[ovK];
[vbase][ovK]overlay=x=(main_w-overlay_w)/2:y=Y:eof_action=repeat:enable='between(n,START,LAST)'[vK]
```

`START` is the first frame of the span and `LAST` its last, so the overlay is on for exactly the span's frames. `Y` is `main_h*0.08` (top), `(main_h-overlay_h)/2` (centre) or `main_h-overlay_h-main_h*0.08` (bottom). The overlay's last frame is held until the span ends. No chart text is in the graph; it holds numbers and a fixed key colour. A project without overlays renders with a **byte-identical** graph to before (the existing golden test).

## Commands and errors

Exit 2: a bad spec, a spec file that is not a plain local file or is over 64 KiB, an unknown field, a span outside the timeline, an overlap, layout problems found by `inspect`, a `://` in a path, a project the overlay cannot be put in. Exit 1: the worker is missing, does not answer `--version`, fails, times out, or writes a video that fails the checks; `ffmpeg` fails. `add-chart` prints `overlay add-chart: out=… id=… frames=A..B asset=… unchanged=false renderer="…"`.

## Boundaries

- **Always:** keep chart text out of code, argv and filter strings; check the worker's output before using it; write the project atomically; leave the project untouched on any failure.
- **Ask first:** another chart kind; a second overlay track; using `background_opacity` or any alpha path; installing the real tool in CI.
- **Never:** vendor the manimgx repository or its wheels; call a shell; call the network; invent a manimgx flag; run the real tool in a test.

## Testing Strategy

- Pure unit tests (crate `scenes`): spec validation (every bound at and one past it, unknown field, control and bidi characters), number formatting, nice axis ranges, canonical text and id stability, the generated scene source for hostile titles (exactly one literal; round trip through a JSON parser; no title text outside it), the argument lists of `render` and `inspect` (exactly the documented flags), the runner against fake scripts (success, exit 1 from inspect, exit 3, timeout, missing binary, missing output).
- Unit tests (crate `timeline`): add, same spec twice (unchanged), overlap refused, remove, track marker, reorder by time, `check_editable` and friends ignoring the track, cut/punch/bridge refusing it, the golden graph for a project with overlays, the same golden for none.
- End to end (`crates/reelctl/tests/overlay_fixture.rs`): fake `manimgx` script; add, add again (one overlay, same bytes, the script called once), render, and pixel checks on the frames before the start, at the start, in the middle, at the last frame and after the end, at 30 fps and at 29.97 fps; a missing binary exits 1; a URL path exits 2; a hostile title never appears in the script's logged arguments; `remove` restores the render.
- Mutation pass over the pure logic with `cargo mutants` if it installs, else by hand; survivors listed in [status](../status.md).
- By hand: [timeline.md](../timeline.md#checking-an-overlay-with-the-real-manimgx) records the first run with the real tool, and `crates/timeline/tests/otio_check.py` reads the two-video-track project with the reference library.

## Limits

- Three chart kinds, one spec file per overlay, no overlap, bars and lines are non-negative. Charts are drawn once for the span; no animation parameters are exposed.
- The key colour is lost to the picture: text edges carry a faint fringe of `#ff00ff`, and the scene may not use that colour.
- The overlay is drawn at the source's size and scaled at render; a render much larger than the source softens it.
- 3D scenes, `present`, `preview`, sound and speech (`self.say`) are not used.
