#!/usr/bin/env bash
# Synthetic fixtures only. Color sources, a hard cut, and a known silence gap.
# No scraped Reels, no network.
set -euo pipefail

encode() {
  local out="$1"
  local filter="$2"
  shift 2
  mkdir -p "$(dirname "$out")"
  ffmpeg -y -hide_banner -loglevel error \
    "$@" \
    -filter_complex "$filter" \
    -map "[v]" -map "[a]" \
    -c:v libx264 -preset ultrafast -tune zerolatency -pix_fmt yuv420p -bf 0 -g 30 \
    -c:a pcm_s16le \
    -movflags +faststart \
    "$out"
}

draft12() {
  local out="$1"
  local size="${2:-640x360}"
  encode "$out" \
    "[0:v][1:v]concat=n=2:v=1:a=0[v];[2:a]volume=volume=0:enable='between(t,2.0,2.5)'[a]" \
    -f lavfi -i "color=c=red:s=${size}:r=30:d=6" \
    -f lavfi -i "color=c=white:s=${size}:r=30:d=6" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=12"
}

outlier12() {
  local out="$1"
  local size="${2:-640x360}"
  encode "$out" \
    "[0:v][1:v][2:v]concat=n=3:v=1:a=0[v];[3:a]anull[a]" \
    -f lavfi -i "color=c=red:s=${size}:r=30:d=3" \
    -f lavfi -i "color=c=white:s=${size}:r=30:d=6" \
    -f lavfi -i "color=c=0x00ff00:s=${size}:r=30:d=3" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=12"
}

bench60() {
  local out="$1"
  encode "$out" \
    "[0:v][1:v]concat=n=2:v=1:a=0[v];[2:a]volume=volume=0:enable='between(t,10.0,10.5)'[a]" \
    -f lavfi -i "color=c=red:s=1920x1080:r=30:d=30" \
    -f lavfi -i "color=c=white:s=1920x1080:r=30:d=30" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=60"
}

cmd="${1:-}"
out="${2:-}"
case "$cmd" in
  draft12)
    draft12 "$out"
    ;;
  outlier12)
    outlier12 "$out"
    ;;
  bench60)
    bench60 "$out"
    ;;
  all)
    root="$(cd "$(dirname "$0")" && pwd)"
    dir="${root}/generated"
    mkdir -p "$dir"
    draft12 "${dir}/draft_12s.mp4"
    outlier12 "${dir}/outlier_12s.mp4"
    bench60 "${dir}/bench_60s.mp4"
    ;;
  *)
    echo "usage: generate.sh draft12|outlier12|bench60|all [out.mp4]" >&2
    exit 2
    ;;
esac
