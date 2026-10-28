#!/usr/bin/env bash
# Synthetic fixtures only. Color sources, a hard cut, and a known silence gap.
# No scraped Reels, no network. speech12 needs macOS `say` and its Samantha voice.
set -euo pipefail

# Codec arguments for encode. draft12_10bit and draft12_44k replace them in a
# subshell, so `all` does not encode the fixtures after them with their codecs.
video_codec=(-c:v libx264 -preset ultrafast -tune zerolatency -pix_fmt yuv420p -bf 0 -g 30)
audio_codec=(-c:a pcm_s16le)

encode() {
  local out="$1"
  local filter="$2"
  shift 2
  mkdir -p "$(dirname "$out")"
  ffmpeg -y -hide_banner -loglevel error \
    "$@" \
    -filter_complex "$filter" \
    -map "[v]" -map "[a]" \
    "${video_codec[@]}" \
    "${audio_codec[@]}" \
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

# draft12 as 10-bit HEVC (Main 10, yuv420p10le), tagged BT.2020 and HLG like
# an iPhone recording. The tags are written into the bitstream after encoding:
# tagging the encoder's output instead makes FFmpeg convert the pixels to the
# BT.2020 matrix (red becomes Y 73, U 96, V 239 rather than 81, 90, 240), and
# this clip must decode to draft12's values so its colors can be compared.
draft12_10bit() (
  video_codec=(-c:v libx265 -preset ultrafast -pix_fmt yuv420p10le -tag:v hvc1
    -x265-params "bframes=0:keyint=30:log-level=error"
    -bsf:v "hevc_metadata=colour_primaries=9:transfer_characteristics=18:matrix_coefficients=9")
  draft12 "$1"
)

# 60 s of 640x360 video over a 44.1 kHz sine, encoded as AAC, with a mute at
# 50.0-50.5 s (the volume filter switches on 1,024-sample frames, so the mute
# is really 50.0159-50.5034 s). Audio that is not 48 kHz is resampled by reelctl, and resampling
# each decoded frame on its own made the pause drift early by 0.5 ms per second.
# AAC frames are 1,024 samples, 0.56 samples short each.
draft12_44k() (
  audio_codec=(-c:a aac -b:a 128k)
  encode "$1" \
    "[0:v][1:v]concat=n=2:v=1:a=0[v];[2:a]volume=volume=0:enable='between(t,50.0,50.5)'[a]" \
    -f lavfi -i "color=c=red:s=640x360:r=30:d=30" \
    -f lavfi -i "color=c=white:s=640x360:r=30:d=30" \
    -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=60"
)

# 12 s of four 3 s shots with hard cuts at 3, 6, and 9 s (frames 72, 144, 216
# at 24 fps), between textured sources: two test patterns and two Mandelbrot
# zooms. Each changes smoothly from frame to frame, so a second detection on the
# frame after a cut means the decoder is wrong, not the source. (Not `gradients`,
# which differs from run to run even with a seed, nor `life` and `cellauto`,
# which change drastically between frames.) The encode has B-frames, scene-cut
# detection off, and a keyframe every 25 frames, not 24, so none lands on a
# cut. The encoder places no keyframe at a cut and packet sizes do not mark
# one (measured on a longer corpus, status.md): a decoder that skips frames
# would lose these cuts.
textured_cuts12() (
  video_codec=(-c:v libx264 -preset fast -crf 22 -profile:v high -g 25 -bf 3 -sc_threshold 0 -pix_fmt yuv420p)
  textured_cuts_shots "$1"
)

# The four shots, encoded with whatever video_codec is set.
textured_cuts_shots() {
  local cut="trim=duration=3,setpts=PTS-STARTPTS,format=yuv420p"
  encode "$1" \
    "[0:v]${cut}[a0];[1:v]${cut}[a1];[2:v]${cut}[a2];[3:v]${cut}[a3];[a0][a1][a2][a3]concat=n=4:v=1:a=0[v];[4:a]anull[a]" \
    -f lavfi -i "testsrc2=s=640x360:r=24" \
    -f lavfi -i "mandelbrot=s=640x360:r=24" \
    -f lavfi -i "testsrc=s=640x360:r=24" \
    -f lavfi -i "mandelbrot=s=640x360:r=24:start_x=-0.743643887037:start_y=0.131825904205:start_scale=0.01:end_scale=0.003:end_pts=300" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=12"
}

# textured_cuts12 as HEVC, for the media-engine decoder: 10-bit Main 10 (what an
# iPhone records) and 8-bit Main. Keyframes every 25 frames, scene cuts off.
textured_cuts12_hevc() (
  video_codec=(-c:v libx265 -preset fast -crf 24 -pix_fmt "${2:-yuv420p10le}" -tag:v hvc1
    -x265-params "keyint=25:min-keyint=25:scenecut=0:bframes=3:log-level=error")
  textured_cuts_shots "$1"
)

# textured_cuts12_hevc8 (the 8-bit variant) flagged as full range (0-255), as some Android and
# screen-recording exports are. FFmpeg's software decoder reads it as yuvj420p;
# the media engine returns nv12 and only the frame metadata says "full".
textured_cuts12_hevc_full() (
  video_codec=(-c:v libx265 -preset fast -crf 24 -pix_fmt yuv420p -color_range pc -tag:v hvc1
    -x265-params "keyint=25:min-keyint=25:scenecut=0:bframes=3:range=full:log-level=error")
  textured_cuts_shots "$1"
)

# 6 s of HEVC that grows from 320x180 to 640x360 after 3 s, as raw Annex B: a
# new SPS and a keyframe, with no container to hold one frame size. Only the
# frame size changes. Video only, so it has no silence and no words. Given
# "shrink" it goes the other way, from 640x360 to 320x180.
resize12_hevc() {
  local out="$1" dir
  local sizes=(320x180 640x360)
  [ "${2:-}" = shrink ] && sizes=(640x360 320x180)
  dir="$(mktemp -d -t reelctl-resize)"
  for size in "${sizes[@]}"; do
    ffmpeg -y -hide_banner -loglevel error \
      -f lavfi -i "testsrc2=s=${size}:r=24:d=3" \
      -c:v libx265 -preset fast -crf 24 -pix_fmt yuv420p \
      -x265-params "keyint=25:min-keyint=25:scenecut=0:bframes=3:log-level=error" \
      -f hevc "${dir}/${size}.hevc"
  done
  mkdir -p "$(dirname "$out")"
  cat "${dir}/${sizes[0]}.hevc" "${dir}/${sizes[1]}.hevc" > "$out"
  rm -rf "$dir"
}

# 6 s of HEVC that changes from 8-bit Main to 10-bit Main 10 after 3 s, as raw
# Annex B, at one frame size. Video only.
depth12_hevc() {
  local out="$1" dir format
  dir="$(mktemp -d -t reelctl-depth)"
  for format in yuv420p yuv420p10le; do
    ffmpeg -y -hide_banner -loglevel error \
      -f lavfi -i "testsrc2=s=640x360:r=24:d=3" \
      -c:v libx265 -preset fast -crf 24 -pix_fmt "$format" \
      -x265-params "keyint=25:min-keyint=25:scenecut=0:bframes=3:log-level=error" \
      -f hevc "${dir}/${format}.hevc"
  done
  mkdir -p "$(dirname "$out")"
  cat "${dir}/yuv420p.hevc" "${dir}/yuv420p10le.hevc" > "$out"
  rm -rf "$dir"
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

# 12 s of a 440 Hz tone at -46 dB peak (about -49 dB RMS, the level of a very
# quiet download or a low-gain microphone) with a planted 0.5 s mute at
# 5.0-5.5 s. Everything is under the old -40 dB silence floor. lavfi's sine has
# a fixed peak of -18 dBFS, so volume lowers it by 28 dB.
quiet12() {
  local out="$1"
  encode "$out" \
    "[0:v]null[v];[1:a]volume=-28dB,volume=volume=0:enable='between(t,5.0,5.5)'[a]" \
    -f lavfi -i "color=c=black:s=640x360:r=30:d=12" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=12"
}

# 70 s, 640x360: 35 s of red, then 35 s of blue (one hard cut at 35 s), over a tone.
# Long enough for a 30-60 s clip pick; `reelctl understand` tests give it words.
lecture70() {
  local out="$1"
  encode "$out" \
    "[0:v][1:v]concat=n=2:v=1:a=0[v];[2:a]anull[a]" \
    -f lavfi -i "color=c=red:s=640x360:r=24:d=35" \
    -f lavfi -i "color=c=blue:s=640x360:r=24:d=35" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=70"
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
  draft12_10bit)
    draft12_10bit "$out"
    ;;
  draft12_44k)
    draft12_44k "$out"
    ;;
  textured_cuts12)
    textured_cuts12 "$out"
    ;;
  textured_cuts12_hevc)
    textured_cuts12_hevc "$out"
    ;;
  textured_cuts12_hevc8)
    textured_cuts12_hevc "$out" yuv420p
    ;;
  textured_cuts12_hevc_full)
    textured_cuts12_hevc_full "$out"
    ;;
  resize12_hevc)
    resize12_hevc "$out"
    ;;
  shrink12_hevc)
    resize12_hevc "$out" shrink
    ;;
  depth12_hevc)
    depth12_hevc "$out"
    ;;
  outlier12)
    outlier12 "$out"
    ;;
  bench60)
    bench60 "$out"
    ;;
  quiet12)
    quiet12 "$out"
    ;;
  lecture70)
    lecture70 "$out"
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
    draft12_10bit "${dir}/draft_12s_10bit.mp4"
    draft12_44k "${dir}/draft_12s_44k.mp4"
    textured_cuts12 "${dir}/textured_cuts_12s.mp4"
    textured_cuts12_hevc "${dir}/textured_cuts_12s_hevc.mp4"
    textured_cuts12_hevc "${dir}/textured_cuts_12s_hevc8.mp4" yuv420p
    textured_cuts12_hevc_full "${dir}/textured_cuts_12s_hevc_full.mp4"
    resize12_hevc "${dir}/resize_6s.hevc"
    resize12_hevc "${dir}/shrink_6s.hevc" shrink
    depth12_hevc "${dir}/depth_6s.hevc"
    outlier12 "${dir}/outlier_12s.mp4"
    noisy12 "${dir}/noisy_12s.mp4"
    quiet12 "${dir}/quiet_12s.mp4"
    lecture70 "${dir}/lecture_70s.mp4"
    bench60 "${dir}/bench_60s.mp4"
    ;;
  *)
    echo "usage: generate.sh draft12|draft12_10bit|draft12_44k|textured_cuts12|textured_cuts12_hevc|textured_cuts12_hevc8|textured_cuts12_hevc_full|resize12_hevc|shrink12_hevc|depth12_hevc|outlier12|noisy12|quiet12|lecture70|bench60|speech12|tonespeech|all [out.mp4]" >&2
    exit 2
    ;;
esac
