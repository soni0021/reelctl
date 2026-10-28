# Spec: OpenTimelineIO editing commands (`timeline-editor`, P2 first slice)

Status: built (see [status](../status.md#timeline-editor-2026-10-28)); written 2026-10-28 under the user's "p1-p3" and the answer "OTIO JSON + ffmpeg render" for the first slice; the user has not reviewed it line by line. Branch: `feature/timeline-editor`. Follows [planer.md](../../planer.md) Components 4 to 5 and the P2 row. Smelter, manimgx and RobustVideoMatting are later slices (each needs its own spec); `reelctl motion mimic-2d` stays optional and out of scope.

## Objective

`diff` tells a creator what to fix; nothing yet fixes it. This slice gives an agent or a person deterministic edit commands over an OpenTimelineIO (`.otio`) project, and a render:

```
reelctl timeline init --input take.mp4 [--dna take.edit_dna.json] --out edit.otio
reelctl timeline cut-silence --project edit.otio --dna take.edit_dna.json [--min-gap-ms 180] [--fade-ms 30] [--fillers um,uh] --out edit.otio
reelctl transform punch-in --project edit.otio --start-ms T1 --end-ms T2 --scale 1.18 --out edit.otio
reelctl funnel attach-bridge --project edit.otio --target-url URL [--cutoff-pct 0.72] [--card-ms 2500] --out edit.otio
reelctl render --project edit.otio --out out.mp4 [--size 1080x1920]
reelctl qa inspect-boundaries --project edit.otio --out-dir views/
```

Each command reads one `.otio`, applies one change, and writes one `.otio` (the same path is allowed). A command is idempotent: running it twice with the same arguments gives the same file. Nothing runs a model.

## Evidence

- planer.md names these commands in its tool table (`timeline cut-silence --min-gap-ms 180 --fade-ms 30`, `transform punch-in --start-ms --end-ms --scale 1.18`, `funnel attach-bridge --cutoff-pct 0.72`, `qa inspect-boundaries`).
- `edit_dna.json` already has everything `cut-silence` needs (`silence_gaps`, whisper `words` with times) and `visual_events` records punch-ins in the same terms the transform writes.
- There is no maintained Rust crate for OpenTimelineIO. `.otio` is JSON with a documented schema (`OTIO_SCHEMA` strings such as `Timeline.1`, `Stack.1`, `Track.1`, `Clip.2`, `Gap.1`, `ExternalReference.1`, `RationalTime.1`, `TimeRange.1`); this slice reads and writes that JSON itself for the subset below, and a file it wrote must load in the reference library (checked in a test that is skipped by being absent: see Testing).
- `ffmpeg` (the CLI, installed on macOS developer machines) renders trims, concatenation, fades and zooms. `decompile` stays free of an ffmpeg subprocess (ADR on structural decode); the render is a separate concern and may use it, and says so.

## Tech Stack

Rust workspace. A new crate `crates/timeline` (the OTIO subset, the edit operations, the ffmpeg filter-graph builder; pure and testable without video), commands in `crates/reelctl`. Dependencies already in the tree (`serde`, `serde_json`, `clap`, `thiserror`). The render and the filmstrip use the `ffmpeg` CLI and `crates/filmstrip` respectively.

## Contract

### The OTIO subset

`Timeline.1` with one `Stack.1` holding a video `Track.1` and an audio `Track.1` of `Clip.2` items with `ExternalReference.1` media (absolute path as written by `init`, `available_range`), `source_range` in `RationalTime.1` and `TimeRange.1` at the media frame rate, and `Gap.1`. Edits are recorded as `metadata.reelctl` on the clip they affect: `{"punch_in": {"scale": 1.18, "start": ..., "end": ...}}`, `{"fade_ms": 30}`, `{"bridge": {"url": ..., "card_ms": ...}}`. Unknown fields in a file are kept on write.

### Commands

- **init:** one clip spanning the whole media (video and audio), frame rate from the file. With `--dna`, stores its path and `duration_ms` check in metadata.
- **cut-silence:** removes every `silence_gaps` entry of at least `--min-gap-ms` (default 180) from both tracks, leaving a 30 ms audio crossfade (`--fade-ms`) at each join, and, when `--dna` has whisper words, removes whole words that equal a `--fillers` entry (default `um,uh`, case-insensitive, punctuation stripped). Times come from the DNA, are never invented, and are snapped to the frame grid (the cut moves to the nearest frame, never into a word: it snaps outward into the pause). The result lists the removed spans in `metadata.reelctl.cuts`. Running it on its own output removes nothing more.
- **punch-in:** adds a scale keyframe (1.0 to `--scale` over 6 frames at `--start-ms`, held, back over 6 frames ending at `--end-ms`) to the video clip covering the span, splitting a clip if the span crosses a boundary. `--scale` must be above 1 and at most 2.
- **attach-bridge:** at `--cutoff-pct` (default 0.72) of the timeline's length, cuts the timeline (the viewer is left at a tension point; what follows is dropped from the output, not from the media), and appends a card clip of `--card-ms` showing `--target-url` as text. The card is a generated clip (an `ffmpeg` `color` source with `drawtext`), recorded as a `GeneratorReference.1`. A cut falls in a pause if one is within 400 ms after the point in the DNA, else at the point.
- **render:** builds one ffmpeg filter graph from the project (trims, concatenation, audio crossfades, the zoom for punch-ins, the card) and runs it; output H.264 + AAC, `--size` scales and crops to fill (default the source size). Fails with a clear message if `ffmpeg` is not found. The rendered duration equals the project's duration within one frame.
- **inspect-boundaries:** a filmstrip PNG per join (using `crates/filmstrip`) in `--out-dir`, so a person or an agent can check each cut.

### Errors

Exit 2 for a bad argument, an unreadable or invalid `.otio`, or a span outside the timeline; exit 1 for an ffmpeg failure. Every error names the file or argument.

## Boundaries

- **Always:** keep the source media untouched; write the `.otio` atomically (temp file then rename); keep unknown OTIO fields.
- **Ask first:** adding an OTIO schema version or another effect type; a dependency.
- **Never:** invent a time; cut inside a word; run a model; call the network; commit media.

## Testing Strategy

- **Unit tests (crate `timeline`):** OTIO round trip with unknown fields kept; `init`; `cut-silence` on a hand-made DNA (gaps below, at and above 180 ms; a gap at the start and end; overlapping gaps; fillers at the boundary of a pause; idempotence); punch-in splitting a clip and rejecting a bad span or scale; bridge placement with and without a nearby pause; frame snapping goes outward; the filter-graph text for a known project (golden).
- **End to end** (fixtures from `fixtures/generate.sh`: `draft12` has a planted 2 s pause and a cut; speech fixtures exist): `init` + `cut-silence` + `render` of `draft12` gives a file shorter by the planted pause (within a frame), its audio has no click at the join (a short-window peak check), and re-running `decompile` on the output finds no gap of 180 ms or more where the pause was; `punch-in` on `motion12`'s untouched section and `decompile` of the render find a `punch_in` event near the requested time (it closes the loop with `visual-motion`, if merged); `attach-bridge` render ends with the card (frame check at the end).
- **OTIO compatibility:** a test loads the written `.otio` with the reference Python `opentimelineio` and compares clip counts and ranges, when that package is importable; the test is not skipped silently: it is a separate command in `docs/status.md` run by hand and its result recorded, because the package is not installed here. If it cannot be run, `status.md` says the compatibility is unverified.
- **Mutation:** the fade handling, the outward snap, the 180 ms boundary and the bridge percentage.

## Success Criteria

1. The commands above exist, are idempotent, and write valid `.otio`.
2. The end-to-end silence cut removes the planted pause and no gap of 180 ms or more remains there; the join has no click.
3. A punch-in render is detected by `decompile` near the requested time.
4. `render` of a project with a bridge ends with the card.
5. `cli.md` (each command), `docs/timeline.md` (the OTIO subset and the metadata keys), README, `status.md`, the capability map, CHANGELOG and AGENTS.md match; OTIO compatibility with the reference library is verified or stated as unverified.

## Resolved Questions

1. **Our own OTIO JSON subset, not a binding.** There is no Rust crate and a C++ binding is heavy.
2. **The `ffmpeg` CLI for render only.**
3. **One command, one file in, one file out, idempotent**, so an agent can retry.
4. **Smelter, manimgx, RobustVideoMatting, reframe, captions, voice** are later slices.
