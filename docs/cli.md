# reelctl CLI reference

Build with `cargo build --release`; the binary is `target/release/reelctl`. `reelctl --version` prints the workspace version (currently `0.1.0`).

## reelctl decompile

Decode one video and write its edit DNA.

```bash
reelctl decompile --input <reel.mp4> --out <edit_dna.json> [options]
```

| Flag | Env var | Default | Meaning |
|---|---|---|---|
| `--input <path>` | | required | Local video file |
| `--out <path>` | | required | Where to write `edit_dna.json`; missing parent directories are created |
| `--filmstrip-dir <dir>` | | `<out stem>_timeline` beside `--out` | Where to write the `timeline_view` PNGs |
| `--cut-detector <auto\|scene\|transnet>` | | `auto` | Which cut detector to run; see below |
| `--transnet-weights <path>` | `REELCTL_TRANSNET_WEIGHTS` | none | TransNetV2 ONNX weights |
| `--decoder <auto\|software>` | | `auto` | Video decoder. `auto` decodes HEVC on the Apple media engine (VideoToolbox) on macOS and everything else in software; if the media engine fails at any point, the file is decoded again in software and `decode_note` says why. `software` forces software. Both give the same cuts, scores, colors, and thumbnails ([ADR-012](decisions/ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md)). Also accepted by `diff` |
| `--whisper-model <path>` | `REELCTL_WHISPER_MODEL` | none | ggml whisper.cpp model. With one, `words` come from whisper.cpp; without one, from the silence-burst estimator. A path that is missing, is not a ggml whisper model, or has a shape no DTW preset matches exits with code 1. See [Word timing](#word-timing) |

`<out stem>` is the output file name minus its last extension, so `--out a/clip.edit_dna.json` puts the PNGs in `a/clip.edit_dna_timeline/`.

Cut detectors:

- `scene`: the scene-score detector ([ADR-002](decisions/ADR-002-scene-score-cut-detector.md)).
- `transnet`: TransNetV2 or an error. Until ort is linked this always exits with code 1, for example `error: decompile: TransNetV2 ONNX weights were not provided. ...`
- `auto`: TransNetV2 if `--transnet-weights` names an existing file, otherwise scene score. If TransNetV2 fails, which it does until ort is linked, decompile falls back to scene score and gives the reason in `cut_detector_note`.

### Output

One line on stdout:

```
signal_ms=11 asr_ms=0 asr_us=333 structural_under_500ms=true cuts=1 silence_gaps=1 duration_ms=12000
```

| Key | Meaning |
|---|---|
| `signal_ms` | Measured structural time; the same value as in the JSON |
| `asr_ms`, `asr_us` | Word-timing time in milliseconds and in microseconds |
| `structural_under_500ms` | `true` if the structural time is under 500 ms |
| `cuts`, `silence_gaps` | How many of each the JSON contains |
| `duration_ms` | Length of the video |

If `REELCTL_PROFILE` is set to any value, stderr also gets the time of each stage:

```
profile decode_ms=9 analyze_ms=0 silence_ms=0 loudness_ms=1 asr_ms=0 asr_first_ms=0 asr_second_ms=0 json_ms=0 filmstrips=1
```

`decode_ms` includes cut scoring, color sampling, and thumbnail capture, which happen inside the decode loop. `analyze_ms` covers choosing cuts from the scores, building per-shot histograms, and finding the payoff cue. `asr_first_ms` and `asr_second_ms` split `asr_ms` with a whisper model: the first pass, including starting the worker and loading the model, and the re-runs of stretches the first pass left without words. Both are 0 without a model. `filmstrips` is the number of PNGs written.

Files written:

- `edit_dna.json`, pretty-printed. Every field is described in [edit-dna.md](edit-dna.md).
- One PNG per detected cut, named `timeline_view_cut_<NN>_f<frame>.png`. For example, `timeline_view_cut_01_f270.png` is the second cut, and frame 270 is the first frame of the new shot.

### timeline_view PNG

For a cut with 4 frames on each side, the image is 1024×168 pixels. From top to bottom:

1. Filmstrip, 72 px: up to 4 frames before the cut and 4 frames starting at the cut, each 64×36 pixels scaled up 2×. An orange line marks the left edge of the first frame of the new shot.
2. Waveform, 64 px: the mono audio from 1.5 s before to 1.5 s after the cut.
3. Word ticks, 24 px: a yellow tick at the start of each word inside that 3-second range. Without `--whisper-model`, each "word" is a voiced burst.

The filmstrip and the waveform do not share a time scale. The 8 frames span about 0.27 s at 30 fps, while the waveform spans 3 s across the same width.

### Word timing

With `--whisper-model`, whisper.cpp 1.8.3 transcribes the audio on the CPU. Any ggml whisper model works, including fine-tunes such as Oriserve Whisper-Hindi2Hinglish-Apex, which writes Hinglish in Roman script:

```bash
reelctl decompile --input reel.mp4 --out reel.edit_dna.json \
  --whisper-model ~/models/whisper/ggml-apex-hinglish-q8_0.bin
```

- English-only (`.en`) models transcribe English; multilingual models detect the language.
- Each word starts at its DTW timestamp and ends at the next word's start, or at the next silence gap if that comes first. DTW needs the model's alignment-head preset, which `reelctl` picks from the shape in the model file's header, so renamed files work.
- whisper.cpp prints nothing. Its time is `asr_ms`, which is not part of `signal_ms` or the 500 ms target. A 60 s clip took about 19 s with the Apex model on an Apple M3.
- Stretches of sound that the first pass left without words are transcribed again, and `asr_note` lists them ([edit-dna.md](edit-dna.md#audio)).
- whisper runs in a child process, `reelctl whisper-worker`, which `--help` does not list. The parent writes the 16 kHz audio to `$TMPDIR/reelctl-whisper-<pid>-<time>.f32` and removes it when the run ends, also after an error. whisper.cpp 1.8.3 can abort the process on short windows when DTW is on; if the worker dies, that one pass runs again in a fresh worker without DTW, and `asr_note` says which. If it dies again, `decompile` exits with code 1, for example `error: decompile: whisper: the whisper worker was killed by signal 6 (SIGABRT) on 16.0-30.9 s, and was killed by signal 6 (SIGABRT) again without DTW`. If `reelctl` cannot find its own executable, whisper runs in-process instead.
- `reelctl` never picks a model on its own. To use one by default, export `REELCTL_WHISPER_MODEL` in your shell profile.

## reelctl diff

Decompile a draft in memory and compare it with an outlier's edit DNA.

```bash
reelctl diff --draft <draft.mp4> --outlier <outlier.edit_dna.json> [options]
```

| Flag | Meaning |
|---|---|
| `--draft <path>` | Draft video. It is decompiled in memory; no JSON or PNGs are written for it |
| `--outlier <path>` | `edit_dna.json` from an earlier `reelctl decompile` |
| `--cut-detector`, `--transnet-weights`, `--whisper-model` | As for `decompile`, applied to the draft |

The report is JSON on stdout. This is the actual output for the `draft12` and `outlier12` fixtures:

```json
{
  "flags": [
    {
      "kind": "dead_air",
      "start_ms": 2010,
      "end_ms": 2510,
      "duration_ms": 500
    },
    {
      "kind": "static_window",
      "start_ms": 0,
      "end_ms": 6000,
      "duration_ms": 6000
    },
    {
      "kind": "static_window",
      "start_ms": 6000,
      "end_ms": 12000,
      "duration_ms": 6000
    },
    {
      "kind": "missing_payoff_cue",
      "window_start_ms": 8000,
      "window_end_ms": 10000
    }
  ]
}
```

| `kind` | Raised when | Fields |
|---|---|---|
| `dead_air` | A silence gap in the draft is longer than 180 ms | `start_ms`, `end_ms`, `duration_ms` |
| `static_window` | The draft goes more than 2,500 ms without a cut; the start and end of the video count as boundaries | `start_ms`, `end_ms`, `duration_ms` |
| `missing_payoff_cue` | The outlier has a cut between 8 and 10 s and the draft does not | `window_start_ms`, `window_end_ms` |

Flags come in that order: all dead air, then all static windows, then the payoff flag. An empty `flags` array means nothing was flagged. Only `missing_payoff_cue` uses the outlier; the other two depend on the draft alone.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Runtime error. stderr gets `error: ` and the cause chain, for example `error: decompile: open input: No such file or directory` or `error: decompile: whisper: model not found at /missing.bin` |
| 2 | Invalid arguments; clap prints the usage |

## Input requirements

- HEVC decodes on the media engine on macOS. It has a fixed cost of about 160 ms, so a short, small HEVC clip is faster with `--decoder software`; a 1080p recording is faster with `auto` (87 s: 4.0–5.3 s against 8.5 s).
- The file needs a video stream. If there are several video or audio streams, the one FFmpeg ranks best is used.
- Video must decode to 4:2:0: 8-bit (`yuv420p`, `yuvj420p`, or `nv12`) or 10-bit (`yuv420p10le`, as iPhone HEVC Main 10 recordings do, or `p010le`, which a hardware decoder returns). 10-bit samples are read as their top 8 bits, and HDR (HLG or PQ) is not tone-mapped; `decode_note` says when either applies ([ADR-011](decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)). Anything else, such as 4:2:2, 4:4:4, or 12-bit, stops with an error that names the format, for example `error: decompile: decode video frame: unsupported pixel format yuv422p; decompile reads 4:2:0 video as yuv420p, yuvj420p, nv12, yuv420p10le, or p010le`.
- Audio must decode to 16-bit integer or 32-bit float samples, as AAC and 16-bit PCM do. It is mixed to mono and, if it is not 48 kHz, resampled to 48 kHz by linear interpolation as one stream, so a moment at time t in the input is at t in the output. There is no low-pass filter, so audio sampled above 48 kHz (88.2 or 96 kHz) can alias.
- Without audio, `silence_gaps` and `words` are empty, `integrated_lufs` is −70, and `words_per_minute` is 0.
