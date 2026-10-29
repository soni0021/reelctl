# ADR-026: Draw captions in reelctl with ab_glyph and lay them over the video as an image sequence

## Status
Accepted (2026-10-29; the user chose "reelctl draws them" over requiring `ffmpeg-full`)

## Date
2026-10-29

## Context
[make-reels](../specs/make-reels.md) burns word captions into each reel, with many style options: preset, font, size, colours, a highlight on the spoken word, outline, shadow, box, position, margin, words per group, wrapping, case. The draft spec drew them with ffmpeg's `drawtext`. Homebrew's `ffmpeg` 8.1, the one the project is tested with, is built without `drawtext`; the bridge card already uses a built-in bitmap font for that reason ([timeline.md](../timeline.md)). `drawtext` also cannot colour one word of a line differently without one filter per word.

## Decision
- **Draw in Rust.** `reelctl` rasterizes caption text itself with `ab_glyph` (pure Rust, MIT or Apache-2.0, no system libraries), the one new dependency. The `image` crate it already uses writes the pictures.
- **One picture per caption state.** Each state is a group of words with one word highlighted, or none. Each is a transparent RGBA PNG the size of the output. The outline is the text's coverage mask grown by a square max filter, and the shadow is the outline mask offset. A box, the text and the highlight are painted over in that order.
- **Exact frames.** `render` gets one image sequence with one picture per timeline frame. The pictures are hard links to the state PNGs, or to a blank, so a 60 s reel costs one file per state. The sequence is laid over the footage at frame-exact timestamps, before any chart overlay.
- **Pure timing.** The style, the word placement through the edits, the groups and the frame-to-picture map live in `crates/timeline` (`captions.rs`) and are stored in the `.otio`. Drawing lives in `crates/reelctl/src/captions_draw.rs`.
- **No font shipped.** The style's `--font`, or the first system bold sans that exists: Arial Bold on macOS, DejaVu Sans Bold on Linux. None is downloaded. With no font, `overlay captions` exits 2 and names the paths it tried.

## Consequences
- Captions render with the stock Homebrew ffmpeg, and look the same on every machine that has the same font.
- A reel with many words writes one PNG per word. Each is mostly transparent and small, and they are removed with the render's scratch directory.
- The outline's corners are square rather than round at large widths.
- Smelter does not draw captions. `render --backend smelter` refuses a captioned project.
- Text shaping is simple: one glyph per character with kerning, and no ligatures or right-to-left scripts. Hindi in Devanagari would need a shaper. Hinglish in Latin script works.
