"""Load a project written by `reelctl` with the reference OpenTimelineIO library.

Run by hand (the package is not a dependency of this repository):

    python3 -m venv /tmp/otio-venv   # a Python with an OpenTimelineIO wheel (3.13 has one)
    /tmp/otio-venv/bin/pip install OpenTimelineIO
    /tmp/otio-venv/bin/python crates/timeline/tests/otio_check.py edit.otio

Prints the track and clip counts and every clip's source range, and exits 1 if the
library cannot read the file, a video and an audio track differ in clip count or
duration, the timeline duration is not the sum of the clips, or an overlay track
(`metadata.reelctl.overlays`, written by `reelctl overlay add-chart`) is longer than
the video. Overlay tracks are listed apart: a project with them has a second Video track.
"""

import sys

import opentimelineio as otio


def main(path):
    timeline = otio.adapters.read_from_file(path)
    def is_overlay(track):
        return bool(track.metadata.get("reelctl", {}).get("overlays"))

    overlays = [track for track in timeline.tracks if is_overlay(track)]
    tracks = {track.kind: track for track in timeline.tracks if not is_overlay(track)}
    print(f"opentimelineio {otio.__version__}: {path}")
    print(f"tracks: {[(track.name, track.kind, len(track)) for track in timeline.tracks]}")
    for kind, track in tracks.items():
        total = 0.0
        for index, item in enumerate(track):
            rng = item.source_range
            total += rng.duration.value
            reference = getattr(item, "media_reference", None)
            what = type(reference).__name__ if reference else type(item).__name__
            print(
                f"  {kind}[{index}] {what} start={rng.start_time.value:.0f} "
                f"duration={rng.duration.value:.0f} rate={rng.duration.rate:g}"
            )
        print(f"  {kind} total frames: {total:.0f}")
    video, audio = tracks.get("Video"), tracks.get("Audio")
    ok = True
    if video is not None and audio is not None:
        if len(video) != len(audio):
            print("MISMATCH: clip counts differ")
            ok = False
        for v, a in zip(video, audio):
            if v.source_range.duration.value != a.source_range.duration.value:
                print("MISMATCH: durations differ")
                ok = False
    if video is not None:
        duration = timeline.duration().value
        summed = sum(item.source_range.duration.value for item in video)
        print(f"timeline.duration() = {duration:.0f} frames, sum of video clips = {summed:.0f}")
        ok = ok and duration == summed
    for track in overlays:
        total = sum(item.source_range.duration.value for item in track)
        clips = [item for item in track if hasattr(item, "media_reference")]
        print(f"overlay track {track.name!r}: {len(clips)} clips, {total:.0f} frames")
        for item in clips:
            reference = item.media_reference
            print(
                f"  overlay {item.name} duration={item.source_range.duration.value:.0f} "
                f"url={getattr(reference, 'target_url', None)}"
            )
        if video is not None:
            video_total = sum(item.source_range.duration.value for item in video)
            if total > video_total:
                print("MISMATCH: an overlay track is longer than the video")
                ok = False
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main(sys.argv[1])
