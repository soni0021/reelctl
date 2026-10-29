# ADR-022: Draw chart overlays with a separately installed manimgx and key them over the video

## Status
Accepted

## Date
2026-10-29

## Context
planer.md names `academa-labs/manimgx` as the renderer for charts and diagrams over a Reel, and `manimgx inspect` as an agent layout check. This slice adds a chart overlay to the OpenTimelineIO project from [ADR-019](ADR-019-edit-an-opentimelineio-project-with-our-own-subset-and-render-with-ffmpeg.md).

What manimgx is, from its README and reference pages on 2026-10-29 (it was **not installed or run** here: `which manimgx` finds nothing, and a `pip install` was refused by the permission system, so it was not worked around): a Rust and wgpu animation engine with a Manim CE-compatible Python API, installed with `uv tool install --python 3.14 manimgx` or `pip` on Python 3.13 or 3.14. A scene is a Python file. `manimgx render FILE [SCENE] -o OUT.mp4 -r WxH --fps N` writes an **x264 MP4**; `manimgx inspect FILE [SCENE] -t end -o OUT.png` checks 2D layout and **exits 1 on problems**. The code is MIT; the native wheels contain x264 and are GPL-3.0-or-later as a whole. It runs on Metal, Direct3D 12 or Vulkan, and falls back to the CPU without a GPU.

Three facts shape the design. The only video output is an MP4 with no alpha, and no page says `background_opacity` reaches the file, so a "transparent" overlay cannot be had from documented behaviour. A scene is Python code, so chart text put into it by string interpolation is a code-injection path. And the tool is large, GPL in its binary form, and needs a GPU or a slow CPU path, so it cannot be vendored or run in the test suite.

## Decision
- **manimgx is a separately installed worker**, as `reelmimic` is: never vendored, found with `--manimgx`, then `MANIMGX_BIN`, then `PATH`, started directly with an argument array (no shell) in a scratch directory with a time limit, and **fail closed**: a missing or broken worker, a time-out, a non-zero exit or a video that fails the checks is an error and the project is left as it was. It must answer `--version` first. Only documented flags are used.
- **Key colour, not alpha.** The scene is drawn on `#ff00ff` (`m.config.background_color`) and `render` keys that colour out with `colorkey`, then lays the result over the picture with `overlay`. After the render the video's corner pixel must be the key colour; if the tool did not honour the background, the overlay is refused instead of becoming an opaque card. The cost is a faint fringe of the key colour on antialiased text edges, and a scene may not use that colour. An opaque inset panel would avoid both but would hide the picture, which is not what an overlay chart is for.
- **Chart text is data, never code.** The scene is a fixed template with one hole. The chart's payload (title, labels, formatted values, the axis range, the length) goes in once, as JSON text made ASCII-only and then encoded as a string, which is a valid Python string literal passed to `json.loads`. Text is drawn only with `m.Text`, which reads no markup. Nothing the user wrote reaches the worker's command line or an ffmpeg filter string; the filter graph holds numbers and the key colour.
- **A typed spec** (`kind` bar, line or number-callout, bounded `series`, `title`, `unit`, a span in ms, `position`, `width_pct`), unknown fields refused, 64 KiB at most, control and text-direction characters refused. Its canonical text gives the overlay id (FNV-1a, 64 bit, 16 hex digits). The same spec is the same overlay: a second `add-chart` leaves the project bytes alone and does not call the worker.
- **The overlay is a second `Video` track** marked `metadata.reelctl.overlays`, of `Gap.1` and `Clip.2` with the chart video as an `ExternalReference.1`, rebuilt from the list of overlays on each change. Overlays may not overlap. Every function that looks for "the" video or audio track skips it. The commands that split and move the main tracks (`cut-silence`, `punch-in`, `attach-bridge`) **refuse** a project with overlays; overlays go last.
- **Compositing is exact in frames.** Each overlay is its own `-i` (with `-protocol_whitelist file`), put on the project's frame grid with `fps`, `settb` and `setpts=N+START`, keyed, scaled to `width_pct` of the output, and overlaid with `enable='between(n,START,LAST)'` and `eof_action=repeat`. `enable` counts the main video's frames rather than float seconds. Pixel checks at 30 and 29.97 fps pass on the frame before the start, the first, the middle, the last and the one after.
- **The command is `reelctl overlay add-chart` / `overlay remove`.** `overlay` is a timeline edit like `transform` and `funnel`; planer.md's `compositor` group is the presenter-isolation slice, a different job. `add-chart` leaves room for other overlay kinds.
- **`manimgx inspect` is a QA step**, run before rendering because it is documented: exit 1 refuses the chart (exit 2: the spec does not fit) and keeps the annotated PNG; any other failure is exit 1; `--skip-inspect` turns it off.

## Consequences
- The real worker has not been run. Seven assumptions about it (A1 to A7 in the [spec](../specs/manimgx-scenes.md#not-verified)) can each be wrong, and the checks after the render are what turn a wrong one into an error rather than a bad video. The first run with the real tool is the recipe in [timeline.md](../timeline.md#checking-an-overlay-with-the-real-manimgx).
- Licence: the manimgx code is MIT, its wheels are GPL-3.0-or-later because of x264. `reelctl` (MIT) only starts it as a separate program and reads its output file; it links nothing from it and ships none of it, so the GPL does not reach this repository. A user who installs the wheels takes its licence for that program.
- A chart can be keyed badly at the edges of text; there is no alpha path to fall back on until manimgx documents one. Switching to alpha later is a change in the scene template, the post-render check and the `colorkey` filter, not in the project format.
- Order matters: cut, zoom and bridge first, overlays last. Changing an overlay is `remove` then `add-chart`.
- The charts are three kinds, non-negative values for bars and lines, one overlay at a time on the track.
- The rendered videos live in `<project>.overlays/` and are not deleted by `remove`.
- Measurements, the mutation pass and what is unverified are in [status](../status.md#manimgx-chart-overlays-2026-10-29).
