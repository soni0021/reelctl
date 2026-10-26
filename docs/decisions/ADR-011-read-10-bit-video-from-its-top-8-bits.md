# ADR-011: Read 10-bit video from its top 8 bits, without tone mapping

## Status
Accepted

## Date
2026-10-26

## Context
iPhone recordings are HEVC Main 10 with an HLG transfer, and FFmpeg's software decoders give them as `yuv420p10le`. `decompile` read only 8-bit 4:2:0 planes and stopped with `unsupported pixel format`, so a creator had to convert every phone clip first. Everything downstream of the planes is 8-bit: the scene-score formula and its 0.30 cut threshold, the 8-bit histogram bins, `yuv601_to_rgb`, and the PNG thumbnails. A frame is read only at sampled points: a 160×90 luma grid for the scene score, a 32×18 grid for color, and 64×36 thumbnails, about 23,000 of a 1080p frame's 3.1 million samples. The requirements are in the [spec](../specs/decode-inputs.md).

## Decision
- Each sampled 10-bit value is read as its top 8 bits (`v >> 2`), through a `Plane` accessor that reads 8-bit planes too. Limited-range 64–940 becomes 16–235, the range 8-bit uses, so the threshold, bins, and color conversion apply unchanged.
- Only the sampled points are read. No frame is converted as a whole.
- A 10-bit frame's range comes from its metadata, because FFmpeg has no full-range 10-bit pixel format. 8-bit is unchanged: only `yuvj420p` is read as full range.
- HDR is not tone-mapped. Colors come from the coded values through the existing BT.601 conversion, as for SDR. `decode_note` says when the video was 10-bit and when its transfer was HLG or PQ, and that colors were not tone-mapped.
- Other formats (4:2:2, 4:4:4, 12-bit, P010) still stop with an error, which now names the format.

## Alternatives Considered

### Convert each frame to 8-bit with libswscale
- Well tested, but it converts every sample of every frame to read about 0.7% of them, on the path the 500 ms gate times. Not measured.

### Ask the decoder for 8-bit output
- FFmpeg's software HEVC decoder outputs the stream's own bit depth; it has no option to decode Main 10 at 8 bits.

### Keep 10-bit precision downstream
- The cut threshold, histogram bins, and thumbnails would all need rescaling, and the 0.30 threshold was set on 8-bit values. The two dropped bits changed no result on the 10-bit test clip.

### Tone-map HDR to SDR
- Per-shot colors would match what a viewer sees, but it needs the BT.2020 matrix and the HLG and PQ curves. Left for a later color module, together with the BT.601/BT.709 known issue (the spec's Resolved Question 3).

## Consequences
- `yuv420p10le` input, such as iPhone HEVC Main 10, decodes without conversion.
- 8-bit output is unchanged. All six existing fixtures gave identical `edit_dna.json` apart from timings, and identical `timeline_view` PNGs. The 60 s benchmark's median `signal_ms` was 248 ms before and 248.5 ms after, over 10 alternating runs each (Apple M3).
- `draft12_10bit`, `draft12` as 10-bit HEVC tagged HLG, gives the same cut frame, the same silence gaps, and per-shot red of 254.67 against 255.
- For HLG and PQ footage, per-shot colors and thumbnails are flatter and less saturated than the picture a viewer sees.
- 10-bit HEVC is slow to decode: FFmpeg took 6.23 s for the first 60 s of a 1080p iPhone recording, against 1.90 s for its 8-bit H.264 conversion. That is module `fast-decode`.
- The test clip's HLG tags are written into the bitstream after encoding with FFmpeg's `hevc_metadata` filter. Tagging the encoder's output instead makes FFmpeg 8 convert the pixels to the BT.2020 matrix, and then the colors cannot be compared with `draft12`'s.
