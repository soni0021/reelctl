# ADR-019: Edit an OpenTimelineIO project with our own subset, and render with the ffmpeg CLI

## Status
Accepted

## Date
2026-10-28

## Context
`reelctl diff` says what to fix and nothing fixes it. planer.md's P2 wants deterministic edit commands over an OpenTimelineIO (`.otio`) project, rendered by Smelter, manimgx and RobustVideoMatting. There is no maintained Rust crate for OpenTimelineIO, a C++ binding is heavy, and the three renderers are large external projects that cannot be run in the test suite. The user chose, on 2026-10-28, a first slice with OTIO JSON and an ffmpeg render.

## Decision
- `crates/timeline` reads and writes the subset of `.otio` JSON the commands need (`Timeline.1`, `Stack.1`, `Track.1`, `Clip.2`, `Gap.1`, `ExternalReference.1`, `GeneratorReference.1`, `RationalTime.1`, `TimeRange.1`), keeps every field it does not model, and writes the same bytes for the same project. Edits are recorded as `metadata.reelctl` on the object they affect. The files load in the reference library (opentimelineio 0.18.1, checked).
- Each command reads one project, applies one change, and writes one (atomically). Running it twice with the same arguments gives the same file.
- `render` builds one ffmpeg filter graph from the project and runs the `ffmpeg` CLI; `timeline init` runs `ffprobe`. Decompile stays free of an ffmpeg subprocess; this is a separate concern.
- A silence cut gives a short audio dip (a fade-out and fade-in) at each join, not an overlapping crossfade, so the audio keeps pace with the picture. A cut snaps into the pause, never into a word.
- The bridge card is drawn by `reelctl` with a built-in bitmap font, because the ffmpeg build here has no `drawtext`.

## Consequences
- Anything in the OTIO schema beyond the subset is carried through and not edited; a project from another tool with unmatched video and audio items must be fixed there first.
- One media file per project, a centred punch-in, and a dip rather than a crossfade are limits of this slice.
- Smelter, manimgx, RobustVideoMatting, reframe, captions and voice are later slices, each needing its own spec.
- Measurements and the audit findings are in [status](../status.md#timeline-editor-2026-10-28).
