#!/usr/bin/env bash
# Synthetic fixtures only. Color sources, a hard cut, and a known silence gap.
# No scraped Reels, no network. speech12 needs macOS `say` and its Samantha voice.
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

# draft12 with white noise throughout, so the planted 2.0-2.5 s mute is room
# noise (-37.2 to -36.4 dB per 10 ms window) rather than digital silence. A
# fixed -40 dBFS threshold finds no gap; the per-recording threshold
# (about -32.9 dB) finds the pause. The seed makes the noise reproducible.
noisy12() {
  local out="$1"
  local size="${2:-640x360}"
  encode "$out" \
    "[0:v][1:v]concat=n=2:v=1:a=0[v];[2:a]volume=volume=0:enable='between(t,2.0,2.5)'[t];[t][3:a]amix=inputs=2:normalize=0[a]" \
    -f lavfi -i "color=c=red:s=${size}:r=30:d=6" \
    -f lavfi -i "color=c=white:s=${size}:r=30:d=6" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=12" \
    -f lavfi -i "anoisesrc=color=white:amplitude=0.025:sample_rate=48000:duration=12:seed=7"
}

bench60() {
  local out="$1"
  encode "$out" \
    "[0:v][1:v]concat=n=2:v=1:a=0[v];[2:a]volume=volume=0:enable='between(t,10.0,10.5)'[a]" \
    -f lavfi -i "color=c=red:s=1920x1080:r=30:d=30" \
    -f lavfi -i "color=c=white:s=1920x1080:r=30:d=30" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=60"
}

# 12 s of solid blue with synthesized speech from 0.5 s and a planted 1 s pause
# after "air.". Digital silence everywhere else, so silence_gaps finds the pause.
speech12() {
  local out="$1"
  local base speech
  base="$(mktemp -t reelctl-speech)"
  speech="${base}.aiff"
  say -v Samantha -o "$speech" "Cut the dead air. [[slnc 1000]] Show the proof before the viewer scrolls away."
  encode "$out" \
    "[1:a]aresample=48000,adelay=500:all=1,apad,atrim=duration=12[a];[0:v]null[v]" \
    -f lavfi -i "color=c=blue:s=640x360:r=30:d=12" \
    -i "$speech"
  rm -f "$base" "$speech"
}

# 40 s: a 25 s tone, then synthesized speech, then silence. The tone is sound
# with no words, so the whisper pass finds a hole there, transcribes it again,
# and finds nothing; the speech after it is transcribed as usual.
tonespeech() {
  local out="$1"
  local base speech
  base="$(mktemp -t reelctl-speech)"
  speech="${base}.aiff"
  say -v Samantha -o "$speech" "Cut the dead air. Show the proof before the viewer scrolls away."
  encode "$out" \
    "[1:a]volume=0.5[t];[2:a]aresample=48000[s];[t][s]concat=n=2:v=0:a=1,apad,atrim=duration=40[a];[0:v]null[v]" \
    -f lavfi -i "color=c=blue:s=640x360:r=30:d=40" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=25" \
    -i "$speech"
  rm -f "$base" "$speech"
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
  noisy12)
    noisy12 "$out"
    ;;
  speech12)
    speech12 "$out"
    ;;
  tonespeech)
    tonespeech "$out"
    ;;
  all)
    root="$(cd "$(dirname "$0")" && pwd)"
    dir="${root}/generated"
    mkdir -p "$dir"
    draft12 "${dir}/draft_12s.mp4"
    outlier12 "${dir}/outlier_12s.mp4"
    noisy12 "${dir}/noisy_12s.mp4"
    bench60 "${dir}/bench_60s.mp4"
    ;;
  *)
    echo "usage: generate.sh draft12|outlier12|noisy12|bench60|speech12|tonespeech|all [out.mp4]" >&2
    exit 2
    ;;
esac
