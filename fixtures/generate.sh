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

# --- Clips for the visual-motion module: a static textured picture with planted
# punch-ins, zoom-outs and overlay boxes, and no cut anywhere. ---------------

motion_fps=24
motion_zoom=0.18  # a punch-in scales the picture 1.00 -> 1.18
motion_ease=6     # frames a zoom move takes (smoothstep from first to last)

# One frame of fine texture: smooth noise at six sizes (6 to 44 px), a mild S
# curve for edges, and different mixes in the three colour planes so luma and
# chroma both carry detail. A scale by 1.18 moves every feature, so it changes
# most pixels. The solid-colour fixtures do not: they look the same at every
# scale. The noise is a sine hash of the pixel position, not the noise filter,
# whose slice threads make the result depend on the thread count. Fixed
# arithmetic means the same frame on every run, which the frame hashes show.
texture_png() {
  local out="$1" w="$2" h="$3"
  # layer STEP SEED: a grid of hash noise with one cell per STEP pixels, scaled
  # up smoothly, then cropped (the grid is a cell larger than the frame).
  layer() {
    local gw=$(( w / $1 + 2 )) gh=$(( h / $1 + 2 ))
    echo "nullsrc=s=${gw}x${gh}:r=1,format=gray,geq=lum='255*(sin((X+$2)*12.9898+(Y+$2*3)*78.233)*43758.5453-floor(sin((X+$2)*12.9898+(Y+$2*3)*78.233)*43758.5453))',scale=$(( gw * $1 ))x$(( gh * $1 )):flags=bicubic,crop=${w}:${h}:0:0,format=gray"
  }
  local s_curve="curves=all='0/0 0.25/0.12 0.75/0.88 1/1'"
  mkdir -p "$(dirname "$out")"
  ffmpeg -y -hide_banner -loglevel error -filter_complex "
    $(layer 30 1)[a];
    $(layer 13 2),split[b1][b2];
    $(layer 6 3),split[c1][c2];
    $(layer 24 4)[d];
    $(layer 10 5),split[e1][e2];
    $(layer 44 6)[f];
    [a][b1]blend=all_expr='A*0.55+B*0.45'[r1];
    [r1][c1]blend=all_expr='A*0.8+B*0.2',${s_curve}[r];
    [d][e1]blend=all_expr='A*0.55+B*0.45'[g1];
    [g1][c2]blend=all_expr='A*0.8+B*0.2',${s_curve}[g];
    [f][b2]blend=all_expr='A*0.6+B*0.4'[b3];
    [b3][e2]blend=all_expr='A*0.8+B*0.2',${s_curve}[b];
    [g][b][r]mergeplanes=0x001020:gbrp,format=rgb24[o]" \
    -map "[o]" -frames:v 1 "$out"
}

# The 0..1 smoothstep that starts at frame $2 and takes motion_ease frames, as
# an FFmpeg expression in the frame number given as the expression $1. scale
# and crop do not agree on `n`: measured on FFmpeg 8.1, scale's n is one ahead of
# the output frame index (frame 96 sees n=97) while crop's is the index. The
# crop would centre each in-move frame on the wrong size, one frame off. So scale
# takes the index from its timestamp, round(t*fps), and crop takes n. (crop's
# `t` is not set, and its x and y fall back to 0.)
ease_expr() {
  local p="clip(($1-$2)/${motion_ease},0,1)"
  echo "(${p}*${p}*(3-2*${p}))"
}

# The 0..1 cubic-bezier(1/3, y1, 2/3, y2) progress that starts at frame $2 and
# takes $3 frames, with y1 = $4 and y2 = $5: with the time control points at one
# third and two thirds the curve's x is its own parameter, so the progress is
# 3(1-p)^2 p y1 + 3(1-p) p^2 y2 + p^3 for p the share of the move done. $1 is the
# frame number as for ease_expr. y2 above 1 overshoots.
bezier_expr() {
  local p="clip(($1-$2)/$3,0,1)"
  echo "(3*(1-${p})*(1-${p})*${p}*$4+3*(1-${p})*${p}*${p}*$5+${p}*${p}*${p})"
}

# Box geometry for a frame of $1 x $2 pixels, as "x y w h": 89% of the width and
# as tall as makes 12% of the frame, every value even so the 4:2:0 chroma
# does not blend at its edges. $3 is lower (a lower third) or upper.
box_geometry() {
  awk -v w="$1" -v h="$2" -v row="$3" 'function even(v) { return 2 * int(v / 2 + 0.5) }
    BEGIN {
      bw = even(0.89 * w); bh = even(0.12 * w * h / bw)
      y = even((row == "upper" ? 0.20 : 0.78) * h - bh / 2)
      print even((w - bw) / 2), y, bw, bh
    }'
}

# motion_clip OUT WxH SECONDS EVENT...: the texture, still, with each EVENT
# planted, over a 440 Hz tone, plus OUT with .mp4 replaced by .events.json listing
# what was planted, sorted by time. The events, in any order:
#   in:FRAME           a punch-in starts at FRAME: 1.00 -> 1.18 around the centre
#   out:FRAME          a zoom-out starts at FRAME: 1.18 -> 1.00
#   bin:FRAME:LEN:Y1:Y2   (and bout:...) the same, but over LEN frames and eased by
#                      the cubic-bezier(1/3, Y1, 2/3, Y2) progress, which the
#                      sidecar lists as "bezier" with "ease_frames" (ease12)
#   box:FRAME:END:ROW:TONE
#                      an opaque box (12% of the frame, ROW lower|upper, TONE
#                      dark|light) shows from FRAME up to END, or to the end if
#                      END is "end"
# A move takes motion_ease frames, so the first frame that shows it in full is
# FRAME + motion_ease; that is `time_ms` and `frame_index` in the sidecar, the
# convention of a cut and of `visual_events`. `start_ms` is where the move
# begins. A box appears, or goes, in one frame.
# The zoom is a scale (eval=frame) and a centred crop, so the real factor is the
# scaled width over the frame width, which rounds to whole pixels: 637/540 =
# 1.1796, not 1.18. The centre is the frame centre to within half a pixel.
# The box is drawn after the zoom, so it does not scale. The encode has B-frames
# and a keyframe every motion_gop frames, 28 unless set (4.0, 4.25, 6.0, 6.25 and
# 8.0 s are frames 96, 102, 144, 150 and 192; the keyframes land on 84, 112, 140,
# 168 and 196), so no keyframe lies inside a move or on a box's first frame, and a
# decoder that skips frames, or a keyframe on an event, cannot be what finds one.
motion_clip() (
  video_codec=(-c:v libx264 -preset fast -crf 18 -profile:v high -g "${motion_gop:-28}" -bf 3 -sc_threshold 0 -pix_fmt yuv420p)
  local out="$1" size="$2" seconds="$3"
  shift 3
  local w="${size%x*}" h="${size#*x}" frames=$(( seconds * motion_fps ))
  local zoom_t="1" zoom_n="1" boxes="" events="" ev kind a b row tone x y bw bh colour
  local dir sidecar="${out%.*}.events.json"
  dir="$(mktemp -d -t reelctl-motion)"
  texture_png "${dir}/texture.png" "$w" "$h"

  # event_json KIND START_FRAME END_FRAME SCALE CENTER_X CENTER_Y AREA CHANGE [Y1 Y2]
  event_json() {
    awk -v kind="$1" -v s="$2" -v e="$3" -v scale="$4" -v cx="$5" -v cy="$6" -v area="$7" -v change="$8" -v y1="${9:-}" -v y2="${10:-}" -v fps="$motion_fps" '
      function num(v, d) { return v == "null" ? "null" : sprintf("%." d "f", v) }
      BEGIN {
        printf "    {\"kind\": \"%s\", \"start_ms\": %.0f, \"end_ms\": %.0f, \"time_ms\": %.0f, \"frame_index\": %d, ", kind, s * 1000 / fps, e * 1000 / fps, e * 1000 / fps, e
        printf "\"start_frame\": %d, \"scale\": %s, \"center_x\": %s, \"center_y\": %s, \"area\": %s", s, num(scale, 4), num(cx, 4), num(cy, 4), num(area, 4)
        if (change != "") printf ", \"change\": \"%s\"", change
        if (y1 != "") printf ", \"bezier\": [0.3333, %s, 0.6667, %s], \"ease_frames\": %d", y1, y2, e - s
        printf "}"
      }'
  }
  # Each line is "FRAME<tab>JSON" (the frame that shows the change in full), so
  # the list can be sorted by time at the end.
  add_event() { events+="$3"$'\t'"$(event_json "$@")"$'\n'; }

  for ev in "$@"; do
    IFS=: read -r kind a b row tone <<<"$ev"
    case "$kind" in
      in)
        zoom_t+="+${motion_zoom}*$(ease_expr "round(t*${motion_fps})" "$a")"
        zoom_n+="+${motion_zoom}*$(ease_expr n "$a")"
        add_event punch_in "$a" $(( a + motion_ease )) "$(awk -v z="$motion_zoom" 'BEGIN { printf "%.4f", 1 + z }')" 0.5 0.5 null ""
        ;;
      out)
        zoom_t+="-${motion_zoom}*$(ease_expr "round(t*${motion_fps})" "$a")"
        zoom_n+="-${motion_zoom}*$(ease_expr n "$a")"
        add_event zoom_out "$a" $(( a + motion_ease )) "$(awk -v z="$motion_zoom" 'BEGIN { printf "%.6f", 1 / (1 + z) }')" 0.5 0.5 null ""
        ;;
      bin)
        zoom_t+="+${motion_zoom}*$(bezier_expr "round(t*${motion_fps})" "$a" "$b" "$row" "$tone")"
        zoom_n+="+${motion_zoom}*$(bezier_expr n "$a" "$b" "$row" "$tone")"
        add_event punch_in "$a" $(( a + b )) "$(awk -v z="$motion_zoom" 'BEGIN { printf "%.4f", 1 + z }')" 0.5 0.5 null "" "$row" "$tone"
        ;;
      bout)
        zoom_t+="-${motion_zoom}*$(bezier_expr "round(t*${motion_fps})" "$a" "$b" "$row" "$tone")"
        zoom_n+="-${motion_zoom}*$(bezier_expr n "$a" "$b" "$row" "$tone")"
        add_event zoom_out "$a" $(( a + b )) "$(awk -v z="$motion_zoom" 'BEGIN { printf "%.6f", 1 / (1 + z) }')" 0.5 0.5 null "" "$row" "$tone"
        ;;
      box)
        read -r x y bw bh < <(box_geometry "$w" "$h" "$row")
        colour=0x101830; [ "$tone" = light ] && colour=0xF0F0E0
        local stop="$b"; [ "$b" = end ] && stop=$(( frames + 1 ))
        boxes+=",drawbox=x=${x}:y=${y}:w=${bw}:h=${bh}:color=${colour}:t=fill:enable='gte(n,${a})*lt(n,${stop})'"
        local area cx cy
        read -r area cx cy < <(awk -v x="$x" -v y="$y" -v bw="$bw" -v bh="$bh" -v w="$w" -v h="$h" \
          'BEGIN { print bw * bh / (w * h), (x + bw / 2) / w, (y + bh / 2) / h }')
        add_event overlay_change "$a" "$a" null "$cx" "$cy" "$area" appear
        [ "$b" = end ] || add_event overlay_change "$b" "$b" null "$cx" "$cy" "$area" disappear
        ;;
      *)
        echo "motion_clip: unknown event '${ev}'" >&2
        exit 2
        ;;
    esac
  done

  # The scaled size as a rounded pixel count; the crop centres on its own copy
  # of the same size (see ease_expr for why there are two).
  local sw="round(${w}*(${zoom_t}))" sh="round(${h}*(${zoom_t}))"
  local cw="round(${w}*(${zoom_n}))" ch="round(${h}*(${zoom_n}))"
  encode "$out" \
    "[0:v]scale=w='${sw}':h='${sh}':eval=frame:flags=bicubic,crop=${w}:${h}:x='floor((${cw}-${w})/2)':y='floor((${ch}-${h})/2)'${boxes},format=yuv420p[v];[1:a]anull[a]" \
    -loop 1 -framerate "$motion_fps" -t "$seconds" -i "${dir}/texture.png" \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=${seconds}"

  # Sorted by frame (stable), then joined with commas.
  events="$(printf '%s' "$events" | sort -n -s -k1,1 | cut -f2- | awk 'NR > 1 { printf ",\n" } { printf "%s", $0 }')"
  {
    printf '{\n  "fixture": "%s",\n  "width": %d,\n  "height": %d,\n  "fps": %d,\n  "duration_s": %d,\n  "events": [' \
      "$(basename "${out%.*}")" "$w" "$h" "$motion_fps" "$seconds"
    [ -n "$events" ] && printf '\n%s\n  ' "$events"
    printf ']\n}\n'
  } > "$sidecar"
  rm -rf "$dir"
)

# 12 s, 540x960 portrait (a reel's shape): a punch-in at 4.0 s (frame 96), a
# zoom-out at 6.0 s (frame 144) and a dark lower-third box from 8.0 s (frame
# 192) to the end. No cut. still12 has the same picture and none of the edits.
motion12() {
  motion_clip "$1" 540x960 12 in:96 out:144 box:192:end:lower:dark
}

still12() {
  motion_clip "$1" 540x960 12
}

# 12 s, 540x960 portrait, still texture, no cut and no box: three punch-ins that
# each take 12 frames and plant a known easing, and a zoom-out after each that
# takes the same easing back down, except the last, which would overshoot to a
# picture smaller than the frame (scale 0.987) and so uses a gentle ease instead.
# cubic-bezier(1/3, y1, 2/3, y2): an ease-in (0, 0.3) at frame 30, an ease-out
# (0.6, 1.0) at 126 and an overshoot (0.1, 1.4, which peaks at 1.07 of the move)
# at 192; the zoom-outs start at 72, 156 and 216 (the last with 0.4, 0.8).
# A keyframe every 60 frames (0, 60, ... 240) lies inside none of the six moves and
# on none of their first frames.
ease12() {
  local motion_gop=60
  motion_clip "$1" 540x960 12 bin:30:12:0:0.3 bout:72:12:0:0.3 bin:126:12:0.6:1.0 bout:156:12:0.6:1.0 bin:192:12:0.1:1.4 bout:216:12:0.4:0.8
}

# motion12 at 640x360 landscape, whose luma grid has other steps and a shape.
motion12_wide() {
  motion_clip "$1" 640x360 12 in:96 out:144 box:192:end:lower:dark
}

# 60 s, 540x960, for the cost of the scale fit (solid-colour bench60 never runs
# it). A punch-in every 5 s from 2 s, each undone 1 s later, so a move every
# 2.5 s; boxes 20-30 s (dark, lower), 40-45 s (light, upper) and from 54.2 s
# (dark, lower), none within 6 frames of a zoom. Every 28th frame would put a
# keyframe inside seven of the 24 moves (found by listing the keyframes against
# the sidecar); a keyframe every 95 frames lands inside none.
bench60_textured() {
  local motion_gop=95 args=() k
  for k in $(seq 0 11); do
    args+=("in:$(( 48 + 120 * k ))" "out:$(( 72 + 120 * k ))")
  done
  motion_clip "$1" 540x960 60 "${args[@]}" box:480:720:lower:dark box:960:1080:upper:light box:1300:end:lower:dark
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
  motion12)
    motion12 "$out"
    ;;
  still12)
    still12 "$out"
    ;;
  motion12_wide)
    motion12_wide "$out"
    ;;
  bench60_textured)
    bench60_textured "$out"
    ;;
  ease12)
    ease12 "$out"
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
    motion12 "${dir}/motion12.mp4"
    still12 "${dir}/still12.mp4"
    motion12_wide "${dir}/motion12_wide.mp4"
    bench60_textured "${dir}/bench60_textured.mp4"
    ease12 "${dir}/ease12.mp4"
    ;;
  *)
    echo "usage: generate.sh draft12|draft12_10bit|draft12_44k|textured_cuts12|textured_cuts12_hevc|textured_cuts12_hevc8|textured_cuts12_hevc_full|resize12_hevc|shrink12_hevc|depth12_hevc|outlier12|noisy12|quiet12|lecture70|bench60|motion12|still12|motion12_wide|bench60_textured|ease12|speech12|tonespeech|all [out.mp4]" >&2
    exit 2
    ;;
esac
