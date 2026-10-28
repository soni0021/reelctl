# Timeline editing: the OTIO subset, the metadata keys, and what each command does

The spec is [specs/timeline-editor.md](specs/timeline-editor.md). This page describes what is built: the subset of OpenTimelineIO (`.otio`) that `reelctl` reads and writes, the `metadata.reelctl` keys it adds, and the behaviour of each command. The CLI flags and exit codes are in [cli.md](cli.md).

The code is `crates/timeline` (the format, the edits and the ffmpeg filter graph; pure, no process and no file) and `crates/reelctl/src/timeline_cmd.rs` (the commands; they run `ffprobe` and `ffmpeg`). Nothing here uses the network or a model. A media path must be a plain local file: `init`, `render`, `attach-bridge` and `qa inspect-boundaries` resolve it to an absolute path and refuse (exit 2) anything with a `://`, anything that does not exist and anything that is not a regular file (a directory, a device, a `concat:` or `pipe:` string). `ffmpeg` and `ffprobe` are given that absolute path, so it cannot be read as a protocol or an option (a file named `-x.mp4` works), and with `-protocol_whitelist file`, so a playlist or script that lists a URL cannot make them fetch it.

## The OTIO subset

A project is one `Timeline.1` holding one `Stack.1` with a `Video` and, when the file has audio, an `Audio` `Track.1`. Their children are `Clip.2` and `Gap.1`. A `Clip.2` has `media_references` (key `DEFAULT_MEDIA`) and `active_media_reference_key`, as the reference library writes it. A reference is an `ExternalReference.1` (`target_url` is the absolute path `init` wrote, plus `available_range`) or a `GeneratorReference.1` (the bridge card). Times are `RationalTime.1` and `TimeRange.1` at the media's frame rate, in whole frames; a fractional frame, a different rate on one clip, a `Transition`, or a nested stack is refused with a message that names it.

Every object keeps the fields this tool does not model (`effects`, `markers`, `enabled`, `color`, `global_start_time`, `available_image_bounds`, and anything else) and writes them back. The same project always writes the same bytes: the fields of the objects this tool models keep a fixed order, and map-valued fields (`metadata`, `parameters`, `media_references`, and the fields kept unmodelled) have sorted keys; two-space indent, a trailing newline.

A frame count or offset in a project is at most 2147483647 (`i32::MAX`); a larger one, and a track whose length does not fit, are refused with a message (exit 2), as is a frame rate of 0 or below.

The edit commands need the video and audio items to match one for one in duration (a cut or a split is made on both). A project from another tool that does not has to be fixed there first; the error says which item differs.

## `metadata.reelctl`

| Where | Key | Value |
|---|---|---|
| `ExternalReference` | `media` | `{width, height, fps, frames, has_audio}` as `init` measured them. `render` needs it. |
| `Timeline` | `dna` | `{path, duration_ms}`: the `edit_dna.json` of the media (absolute path). `attach-bridge` reads pauses from it. |
| `Timeline` | `cuts` | One entry per span `cut-silence` removed: `{reason: "silence" or "filler", source_start_ms, source_end_ms, start_frame, end_frame, text?}`, in the media's own time, sorted by `start_frame`. |
| `Timeline` | `bridge` | `{url, cutoff_pct, card_ms, cut_frame, cut_ms, frames_before, in_pause}`. |
| `Clip` | `fade_ms` | The audio fade at the clip's joins (see Render). |
| `Clip` | `punch_in` | `{scale, ramp_frames, start, end}`; `start` and `end` are `RationalTime`s in frames **of this clip** (a punch-in over several clips has the same span in each clip's own time, so `start` can be negative and `end` can be past the clip's end). |
| `Clip` (card) | `bridge` | `{url, card_ms}`. |

The bridge card is a `GeneratorReference.1` with `generator_kind` `reelctl.card` and `parameters` `{text, background, foreground}` (`#rrggbb`), plus a clip of `reelctl.silence` on the audio track.

## Commands

All of them read one project and write one, atomically (a temporary file in the same directory, then a rename), so `--out` may be `--project`. Running one twice with the same arguments gives the same bytes.

**`timeline init`** probes the video with `ffprobe` (frame rate, frame count, size with a phone rotation applied, whether there is audio) and writes one clip across the whole media on each track. With `--dna` it records the DNA's path and `duration_ms` and refuses a DNA that is more than 100 ms off the video's length.

**`timeline cut-silence`** removes, from both tracks, every `silence_gaps` entry of at least `--min-gap-ms` (180; measured on the DNA's own `duration`, before snapping) and, when the DNA's `asr_backend` is `whisper_cpp`, every word equal to a `--fillers` entry (`um,uh`; lowercase, punctuation stripped, whole words only; voiced bursts are not words). Times are the DNA's, never invented.
- Spans that overlap or touch merge into one removal.
- A word is never cut: a non-filler word that lies in a silence span, or crosses its edge, stays, and the span is removed in pieces around it. Only real `whisper_cpp` words protect or hold a cut back; the voiced bursts of the fallback estimator are not words and do neither.
- A silence edge snaps **into the pause**: the start up to the next frame, the end down to the previous one, so the kept footage grows into the silence and never into a word. A filler edge snaps **out** (start down, end up) to take the whole word, then is held off the neighbouring words (the previous word's end rounded up, the next word's start rounded down). A span that is empty after snapping is dropped.
- Spans are in the media's time, so a span that is already gone removes nothing and a second run changes nothing. The removed spans are listed in `metadata.reelctl.cuts`, and `fade_ms` is set on the clips.
- The DNA must be the media's (length within 100 ms), and the cuts may not remove everything.
- `--fade-ms` is applied even when nothing is removed, so `cut-silence --fade-ms 45` on its own output changes the fade and nothing else; the same arguments again change nothing.

**`transform punch-in`** zooms the centre of the picture from 1.0 to `--scale` over 6 frames at `--start-ms`, holds it, and brings it back over the last 6 frames before `--end-ms`. Times are milliseconds of the edited timeline, rounded to the nearest frame. The video clips under the span are split at its ends and the pieces inside carry `punch_in`; the audio is split in step and left alone. `--scale` must be above 1 and at most 2, the span must be at least 12 frames (two ramps) and inside the timeline. A later punch-in replaces any earlier punch-in it overlaps, on the overlapped frames, and the earlier one ends cleanly at the new start (or, past the new span, starts cleanly at the new end): it ramps to 1.0 there, so the zoom never jumps between two frames by more than one ramp step.

**`funnel attach-bridge`** cuts the timeline at `--cutoff-pct` (0.72) of its length, drops everything after (the media is untouched), and appends a `--card-ms` (2500) card that shows `--target-url`, with silence under it. The cut moves to the start of a pause if the DNA (the one `init` or `cut-silence` recorded, or `--dna`) has a silence gap that starts within 400 ms after the point, and stays if the point is already inside a pause; pauses are found through earlier cuts. A join made by a silence that `cut-silence` removed is a pause too (read from `metadata.reelctl.cuts`; a removed filler word is not), so after `cut-silence` the cut lands on that join and not in the speech around it. The DNA's length is checked against the media (100 ms), and `--card-ms` is at most 60000. A rerun on an unchanged bridge reports the `in_pause` stored in `metadata.reelctl.bridge`. The clip before the card gets a 30 ms audio fade unless it has one. A project that already has the same bridge is left as it is; one with a different bridge is refused (make a new project).

**`render`** builds one ffmpeg filter graph from the project and runs it. H.264 (`libx264`, `veryfast`, CRF 18, `yuv420p`) and AAC 192 kbit/s, 48 kHz stereo, `+faststart`; written next to `--out` and renamed on success. `--size WxH` scales to fill and crops (default: the source's size); each side is even and at most 8192, and the picture at most 33177600 pixels (7680x4320). A project with no frames does not render (exit 2). The output is written to `.<name>.<pid>.tmp.<ext>` beside `--out` (the extension last, so ffmpeg picks the container from it) and renamed; the temporary file is removed on every failure, and `--out` needs an extension and may not be a directory (exit 2). A frame is `trim=start_frame/end_frame`, audio is `atrim=start_sample/end_sample` at 48 kHz, so the video is frame-exact and the audio is exact to a sample.
- **Fades.** At a join where the footage is not continuous (a cut, or the card), a clip with `fade_ms` fades out its end and fades in its start, each over `fade_ms` (at most half the clip). This is a dip, not an overlapping crossfade: an overlap would shorten the audio by `fade_ms` at each join and drift it off the picture. Because cuts snap into the pause, both sides of a silence join are quiet already; the fade is what stops a click when a cut goes through sound (tested with a hand-made DNA over a tone). The pieces of a split clip play on without a fade.
- **Punch-in.** `zoompan` with `d=1` per frame, centred, after the picture is scaled to the output size. It is last in its chain, before the pixel format: a `fps` filter after `zoompan` repeats frames (a 24-frame piece became 12,288).
- **Card.** The URL is drawn by `reelctl` into a PNG the size of the output with a built-in 5x7 bitmap font (capitals, digits and `. , : / - _ ? ! = & % # @ + ~`; anything else is a box), centred, wrapped if it does not fit, with a bar under it. A URL too long for the frame at the wrapped size is cut and ends in `...` (the font has no ellipsis character), and `render` prints `render: the card text was too long for the frame; it shows N of M characters and ends in ...` after its result line. ffmpeg loops the image for `card_ms`. This ffmpeg build (Homebrew 8.1) has no `drawtext`, and a font file would be one more thing to find.
- Fails with exit 1 and a message that names the tool when `ffmpeg` is not on `PATH`. The rendered length equals the project's length: the same frame count, and the audio within one AAC frame of the video.

**`qa inspect-boundaries`** writes one `timeline_view` PNG (the `crates/filmstrip` layout) per join between neighbouring video items into `--out-dir`: two frames before the join and two after, decoded in process with `decompile`'s frame sampler; the audio waveform for 400 ms on each side, decoded with `ffmpeg` (16 kHz mono); and, when the DNA is readable, its word ticks placed on the edited timeline. A side that is the card is a dark frame and silence. The waveform is the source audio as it is in the media, before the render's fade.

## Checking a project with the reference library

`crates/timeline/tests/otio_check.py` loads a project with the python `opentimelineio` package and prints the tracks and every clip's range; it exits 1 if the library cannot read the file, the tracks differ, or the timeline duration is not the sum of the clips. It is run by hand, because the package is not a dependency of the repository:

```
python3.13 -m venv /tmp/otio-venv        # Python 3.14 had no OpenTimelineIO wheel on 2026-10-28
/tmp/otio-venv/bin/pip install OpenTimelineIO
reelctl timeline init --input take.mp4 --dna take.edit_dna.json --out edit.otio
/tmp/otio-venv/bin/python crates/timeline/tests/otio_check.py edit.otio
```

`crates/timeline/tests/reference.otio` is a file written by the reference library (0.18.1) with fields this crate does not model; the unit tests read it and check it round-trips unchanged.

## Limits

- One media file per project; one video and at most one audio track.
- A bridge cannot be changed on a project that has one.
- A punch-in is a centred zoom only; there is no pan, no keyframe editor, and no other effect type.
- The audio fade is a dip at joins, not an overlapping crossfade.
- Filler removal needs whisper words in the DNA. Without a model the DNA has voiced bursts, and fillers are skipped.
- The card font is capitals and a few symbols; no emoji, no non-Latin text, no QR code.
- The media must start at time zero (`trim` is by frame number from the start).
