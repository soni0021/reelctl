# Spec: `.cube` LUT from a reference reel (`cube-lut`)

Status: written 2026-10-28 under the user's blanket "Do all of them"; the user has not reviewed it line by line. The answers at the end are the recommended ones. Branch: `feature/cube-lut`. Module `cube-lut` in the [P0 capability map](p0-capability-map.md). Follows [planer.md](../../planer.md) Pass 1 ("per-shot RGB/LAB color histograms to synthesize a matching .cube 3D LUT").

## Objective

A creator who likes an outlier reel's colour wants their own draft to look like it. `edit_dna.json` records 8-bin colour histograms per shot, which is enough to compare two videos and not enough to build a grade. `cube_lut` has been a `StubbedPass` ("unimplemented") since the first release.

This module adds `reelctl look`: it reads a draft video and an outlier video, matches the draft's colour distribution to the outlier's, and writes a `.cube` 3D LUT that any editor or `ffmpeg -vf lut3d` applies to the draft.

```
reelctl look --draft draft.mp4 --outlier outlier.mp4 --out outlier.cube [--size 33] [--samples 60]
```

It works from sampled frames, not from the timed `decompile` path, so it adds nothing to `signal_ms`. The method is per-channel histogram matching: for each of R, G and B, the cumulative distribution of the draft's sampled pixels is mapped onto the outlier's. The cube holds `out = (matchR(r), matchG(g), matchB(b))`, which is a separable grade: it moves tone and colour cast and cannot reproduce a cross-channel look (split toning, a hue-selective grade). That limit is stated in the output and the docs, not hidden.

Not in this module: per-shot LUTs, a LUT for a segment of a video, LAB or perceptual matching, and applying the LUT (the editor or `ffmpeg` does that).

## Evidence

- `decompile` already decodes frames to RGB at chosen times (`decompile::decode_frames_at`, used by `understand`), so no new decode path is needed.
- Colours are converted with libswscale's BT.601 defaults, as in ADR-011, so absolute values are slightly off for BT.709 video. A LUT built and applied in the same assumed space is consistent; this limit is recorded.
- Two real reels are at hand (the 38.5 s NEET explainer and a 15 s milk clip) with very different grades, and the synthetic fixtures can be graded with `ffmpeg` to a known answer.

## Tech Stack

Rust workspace as today. A new crate `crates/look` (pure: histograms, matching, the `.cube` writer and reader), the command in `crates/reelctl`. No new dependencies. `ffmpeg` is used by the tests only to apply a LUT.

## Contract

- `--draft` and `--outlier` are video files. `--out` is the `.cube` path. `--size` is the cube edge (17, 33 or 65; default 33). `--samples` is the number of frames taken from each video, evenly spaced, at least 1 and at most 600 (default 60). `--decoder` as for `decompile`.
- Output file: Adobe/Resolve `.cube`: `TITLE`, `LUT_3D_SIZE n`, `DOMAIN_MIN 0 0 0`, `DOMAIN_MAX 1 1 1`, then n³ lines of three floats, red index varying fastest. Floats have 6 decimals. The title says it was made by `reelctl look` and from which two files (names only).
- Stdout: one line, `size=33 samples_draft=60 samples_outlier=60 mean_shift_r=… mean_shift_g=… mean_shift_b=… out=…`: the change in each channel's mean, on a 0–255 scale, that the LUT makes to the draft's sampled frames.
- Pixels are weighted equally across all sampled frames, so a longer shot counts for more only through how many samples it receives.
- Identical inputs give an identity LUT (each output within 1/255 of its input).
- Deterministic: the same inputs give a byte-identical file.
- Exit codes: 2 for a bad argument or a missing or unreadable file (before any decoding of the second video), 1 for a decode failure.

## Boundaries

- **Always:** use only pixels from the two videos; keep the cube inside the domain (0 to 1); state the separable limit in `cli.md`.
- **Ask first:** matching in a colour space other than RGB; per-shot LUTs.
- **Never:** write a LUT outside 0–1; read a model file; call the network; commit a real reel.

## Testing Strategy

- **Unit tests (crate `look`):** histograms of known pixel sets; matching two equal distributions gives the identity; a pure brightness offset gives that offset; a distribution compressed into a narrow range stretches; empty or single-valued channels do not divide by zero; the match is monotonic; the `.cube` writer's header and size (n³ lines, red fastest); a writer and reader round trip; sizes 17, 33, 65.
- **End to end:** a textured fixture and a copy graded by `ffmpeg` (a warm `colorbalance` and a `curves` lift): `reelctl look --draft graded --outlier plain` then `ffmpeg -vf lut3d` on the graded clip; the mean absolute error per channel against the plain clip falls by at least 80% and the per-channel CDF distance by at least 90%. Identical files give an identity LUT. `ffmpeg` loads the file without error. Two runs give identical bytes. Missing file exits 2.
- **Real reels:** the NEET explainer and the milk clip in both directions; the numbers and one applied frame each way recorded in `status.md` (judged by eye as well as by the shift).

## Success Criteria

1. The graded-fixture error falls as above, for sizes 17, 33 and 65.
2. Identity for identical inputs; byte-identical reruns; `ffmpeg lut3d` accepts the file.
3. The real reels' mean shifts, the frames, and the time `reelctl look` takes are recorded.
4. `edit_dna.json`'s `cube_lut` reason points to `reelctl look` (schema description only; the stub stays `unimplemented` there, since a LUT needs two videos).
5. `cli.md`, `status.md`, the capability map, README, CHANGELOG and AGENTS.md match the code.

## Resolved Questions

1. **A command, not a field of `edit_dna.json`.** A LUT is a function of two videos; one video's DNA does not make one.
2. **Per-channel matching in RGB.** It is the part that can be tested against a known grade; a hue-selective or cross-channel grade is a limit.
3. **Frames, not the 8-bin histograms.** Eight bins cannot carry a tone curve.
4. **Size 33 by default**, the common editor size.
