#!/usr/bin/env bash
# Manual check: render the same project with the ffmpeg backend and with Smelter, and
# compare. Not a test: it needs a Smelter server the repository does not hold, and a
# test that returned early without one would report a pass it had not earned.
#
#   fixtures/smelter_agreement.sh /path/to/smelter [runs]
#
# Needs: ffmpeg, ffprobe, awk, and `reelctl` (built with `cargo build -p reelctl`; or set
# REELCTL=/path/to/reelctl). Smelter's mp4 input reads H.264 and AAC only, so the
# fixtures' PCM audio is re-encoded to AAC first (video copied). Each fixture is
# compared twice: untagged (as the generator writes it) and tagged BT.709 (as a phone
# reel is), because Smelter reads an untagged picture as BT.709 and ffmpeg keeps its
# values, which moves the colour.
#
# Prints, for each variant and run: the frame counts of both renders, the mean, 95th
# percentile and maximum of the per-frame mean absolute luma difference (0 to 255), the
# number of frames that differ by more than 40 (a scene change on a different frame, or a
# very different zoom), and the noise floor (the ffmpeg render against itself after one
# more x264 encode at the same quality). The audio lengths of both are printed last.
set -euo pipefail

SMELTER="${1:?usage: smelter_agreement.sh /path/to/smelter [runs]}"
RUNS="${2:-5}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REELCTL="${REELCTL:-$ROOT/target/debug/reelctl}"
[ -x "$REELCTL" ] || { echo "reelctl not found at $REELCTL; run: cargo build -p reelctl" >&2; exit 2; }
WORK="$(mktemp -d "${TMPDIR:-/tmp}/smelter-agreement.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

reel() { env -u REELCTL_WHISPER_MODEL -u REELCTL_TRANSNET_WEIGHTS -u ORT_DYLIB_PATH -u OPENROUTER_API_KEY -u SMELTER_BIN "$REELCTL" "$@"; }

luma_diff() { # a.mp4 b.mp4 -> one mean absolute luma difference per frame
  ffmpeg -hide_banner -loglevel error -i "$1" -i "$2" \
    -filter_complex "[0:v][1:v]blend=all_mode=difference,signalstats,metadata=print:key=lavfi.signalstats.YAVG:file=-" \
    -f null - | grep -o 'YAVG=.*' | sed 's/YAVG=//'
}

stats() { # stdin: numbers -> "mean p95 max over40 frames"
  sort -g | awk '{ v[NR]=$1; s+=$1; if ($1>40) o++ } END {
    p = v[int(0.95*NR) > 0 ? int(0.95*NR) : 1];
    printf "mean %.2f  p95 %.2f  max %.2f  over-40 %d  of %d frames", s/NR, p, v[NR], o, NR }'
}

frames() { ffprobe -v error -count_packets -select_streams v:0 -show_entries stream=nb_read_packets -of csv=p=0 "$1"; }
audio_s() { ffprobe -v error -select_streams a:0 -show_entries stream=duration -of csv=p=0 "$1"; }

variant() { # name source.mp4
  local name="$1" src="$2" dir="$WORK/${1// /_}"
  mkdir -p "$dir"
  reel timeline init --input "$src" --out "$dir/p.otio" >/dev/null
  reel decompile --input "$src" --out "$dir/dna.json" --filmstrip-dir "$dir/views" >/dev/null
  reel timeline cut-silence --project "$dir/p.otio" --dna "$dir/dna.json" --out "$dir/p.otio" >/dev/null
  reel transform punch-in --project "$dir/p.otio" --start-ms 1000 --end-ms 3000 --scale 1.3 --out "$dir/p.otio" >/dev/null
  reel funnel attach-bridge --project "$dir/p.otio" --target-url example.com/go --card-ms 1000 --out "$dir/p.otio" >/dev/null
  reel render --project "$dir/p.otio" --out "$dir/ffmpeg.mp4" >/dev/null
  ffmpeg -hide_banner -loglevel error -y -i "$dir/ffmpeg.mp4" -c:v libx264 -crf 18 -preset veryfast -an "$dir/again.mp4"
  echo "== $name: ffmpeg $(frames "$dir/ffmpeg.mp4") frames, audio $(audio_s "$dir/ffmpeg.mp4") s"
  echo "   noise floor (one more x264 generation): $(luma_diff "$dir/ffmpeg.mp4" "$dir/again.mp4" | stats)"
  local i
  for i in $(seq 1 "$RUNS"); do
    if reel render --project "$dir/p.otio" --out "$dir/smelter$i.mp4" --backend smelter --smelter "$SMELTER" >"$dir/smelter$i.log" 2>&1; then
      echo "   run $i: smelter $(frames "$dir/smelter$i.mp4") frames, audio $(audio_s "$dir/smelter$i.mp4") s; $(luma_diff "$dir/smelter$i.mp4" "$dir/ffmpeg.mp4" | stats)"
    else
      echo "   run $i: FAILED: $(head -c 300 "$dir/smelter$i.log")"
    fi
  done
}

for clip in draft12 textured_cuts12; do
  bash "$ROOT/fixtures/generate.sh" "$clip" "$WORK/$clip.mp4"
  ffmpeg -hide_banner -loglevel error -y -i "$WORK/$clip.mp4" -c:v copy -c:a aac -b:a 192k "$WORK/${clip}_untagged.mp4"
  ffmpeg -hide_banner -loglevel error -y -i "$WORK/${clip}_untagged.mp4" -c copy \
    -bsf:v "h264_metadata=colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1:video_full_range_flag=0" \
    "$WORK/${clip}_bt709.mp4"
  variant "${clip} untagged" "$WORK/${clip}_untagged.mp4"
  variant "${clip} tagged BT.709" "$WORK/${clip}_bt709.mp4"
done
