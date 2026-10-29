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
| `--transnet-weights <path>` | `REELCTL_TRANSNET_WEIGHTS` | none | TransNetV2 ONNX weights (`transnetv2.onnx`, input `[1,100,27,48,3]`, output `534`); never downloaded or committed |
| | `ORT_DYLIB_PATH` | none | Full path of an ONNX Runtime library (`libonnxruntime.dylib`, version 1.17 or newer; `brew install onnxruntime` gives `/opt/homebrew/lib/libonnxruntime.dylib`). Only read when TransNetV2 is asked for. No default directory is searched |
| `--decoder <auto\|software>` | | `auto` | Video decoder. `auto` decodes HEVC on the Apple media engine (VideoToolbox) on macOS and everything else in software; if the media engine fails at any point, the file is decoded again in software and `decode_note` says why. `software` forces software. Both give the same cuts, scores, colors, and thumbnails ([ADR-012](decisions/ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md)). Also accepted by `diff` |
| `--whisper-model <path>` | `REELCTL_WHISPER_MODEL` | none | ggml whisper.cpp model. With one, `words` come from whisper.cpp; without one, from the silence-burst estimator. A path that is missing, is not a ggml whisper model, or has a shape no DTW preset matches exits with code 1. See [Word timing](#word-timing) |

`<out stem>` is the output file name minus its last extension, so `--out a/clip.edit_dna.json` puts the PNGs in `a/clip.edit_dna_timeline/`.

Cut detectors:

- `scene`: the scene-score detector ([ADR-002](decisions/ADR-002-scene-score-cut-detector.md)). Never looks at the weights or the library.
- `transnet`: TransNetV2 or an error. The weights file, the library and the model are checked **before the video is decoded**; a problem exits with code 1 and writes nothing, for example `error: decompile: TransNetV2 needs the ONNX Runtime library: ORT_DYLIB_PATH is not set; set ORT_DYLIB_PATH to the full path of an ONNX Runtime library, version 1.17 or newer (macOS: brew install onnxruntime, ...)`. A relative `ORT_DYLIB_PATH` is an error (`ort` tries a relative path beside the executable first, so it could load a different file than the one checked), and so is a weights path that is a directory. An error while the model runs also exits with code 1: the model giving a probability that is not a number from 0 to 1, or a clip of more than 200,000 frames (777.6 MB of 48x27 frames), which fails before the model runs with a message that the clip is too long for TransNetV2 and suggests `--cut-detector scene`. It never returns scene-score cuts.
- `auto`: TransNetV2 if `--transnet-weights` names an existing file and the library and model load; otherwise the scene score. With no weights path it is the scene score and `cut_detector_note` is the scene-score note. With a weights path that is missing, a library that cannot be loaded, a model that is not TransNetV2, an error while running, or a clip over the 200,000-frame limit, decompile falls back to the scene score and `cut_detector_note` starts `Fell back to the FFmpeg scene-score detector:` followed by the reason.

TransNetV2 reads every frame, so it costs 7 to 13 ms per video frame (13 to 23 s for 60 s at 30 fps, depending on machine load) on the M3's CPU (see [the spec](specs/transnet-cuts.md#measured)), far outside the 500 ms structural budget, and `signal_ms` includes it. When the model ran on 9,000 frames or more (5 minutes at 30 fps), `cut_detector_note` adds the frame count and that rate. Manual end-to-end run on the real model (not part of `cargo test`):

```bash
export ORT_DYLIB_PATH=/opt/homebrew/lib/libonnxruntime.dylib      # or any libonnxruntime >= 1.17
export REELCTL_TRANSNET_WEIGHTS=~/models/transnet/transnetv2.onnx
bash fixtures/generate.sh dissolve12 /tmp/dissolve12.mp4
target/release/reelctl decompile --cut-detector transnet --input /tmp/dissolve12.mp4 --out /tmp/dissolve12.edit_dna.json
```

### Output

One line on stdout (the punch-ins, zoom-outs and overlay changes it found are in the JSON's `visual_events`, see [edit-dna.md](edit-dna.md)):

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

Decompile a draft in memory and compare it with one to three outliers' edit DNA.

```bash
reelctl diff --draft <draft.mp4> --outlier <outlier.edit_dna.json> [--outlier <b.edit_dna.json> ...] [options]
```

| Flag | Meaning |
|---|---|
| `--draft <path>` | Draft video. It is decompiled in memory; no JSON or PNGs are written for it |
| `--outlier <path>` | `edit_dna.json` from an earlier `reelctl decompile`. Give it one to three times; a fourth exits with code 2 |
| `--format <json\|text>` | `json` (default) or a readable report. See [the comparison](#comparing-with-outliers) |
| `--cut-detector`, `--transnet-weights`, `--whisper-model` | As for `decompile`, applied to the draft |

The report is JSON on stdout: `flags`, then `comparison` (below). This is the actual `flags` for the `draft12` and `outlier12` fixtures; `comparison` follows it in the output:

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
| `static_window` | The draft goes more than 2,500 ms without a cut or a `visual_events` entry; the start and end of the video count as boundaries | `start_ms`, `end_ms`, `duration_ms` |
| `missing_payoff_cue` | The outlier has a cut between 8 and 10 s and the draft does not | `window_start_ms`, `window_end_ms` |

Flags come in that order: all dead air, then all static windows, then the payoff flag. An empty `flags` array means nothing was flagged. Only `missing_payoff_cue` uses the outlier; the other two depend on the draft alone.

### Comparing with outliers

`flags` above is the same for one outlier or several, apart from the payoff flag: with several outliers it is raised when more than half of them have a cue in 8–10 s and the draft has none. The comparison itself is a second key, `comparison`, after `flags`:

| Key | Meaning |
|---|---|
| `draft`, `outliers[]` | Metrics per reel, named by file name (an outlier's trailing `.edit_dna.json` is dropped): `duration_ms`, `cut_count`, `mean_shot_ms`, `longest_shot_ms`, `dead_air_count`, `dead_air_per_minute` (gaps over 180 ms), `silent_share` (0–1), `words_per_minute` with `words_source` (`silence_bursts` means an estimate), `integrated_lufs` |
| `windows[]` | The draft's length in 5,000 ms windows (the last one shorter): `start_ms`, `end_ms`, `draft_cuts`, and `outlier_cuts`, one number per outlier, or `null` once that outlier has ended |
| `flags[]` | Relative findings, kept apart from `flags` so nothing that reads `flags` changes (below) |

| `kind` in `comparison.flags` | Raised when | Fields |
|---|---|---|
| `rhythm_gap` | The draft has no cut in a window, and more than half of the outliers still running have one. Adjacent windows are joined, and the counts are those of the weakest window | `window_start_ms`, `window_end_ms`, `outliers_with_cut`, `outliers_counted` |
| `pause_density` | The draft's dead-air gaps per minute are more than 1.5 times the outliers' median and at least 3 more | `draft_per_minute`, `outlier_per_minute` |

The 1.5, the 3, and the 5 s window were chosen on two slow reels; they have not been tried against a fast-cut outlier. Speaking rate and loudness are listed, not judged. For `draft12` against `outlier12`, `comparison.flags` is a `rhythm_gap` for 0–5 s (the outlier cuts at 3 s; the draft first cuts at 6 s) and a `pause_density` of 5.0 against 0.0 per minute.

`--format text` prints the same report as a table, with reel names cut to between 8 and 20 characters so that lines stay within 80 (fewer characters when more reels share a line, and numbered if two cut names would match; the JSON keeps the full names), and the windows continuing on further rows when they do not fit one line, then one line per flag (the older flags first, then the relative ones). This is the actual output for `draft12` against `outlier12`:

```
                     draft12     outlier12
length               12.0 s      12.0 s
cuts                 1           2
mean shot            6.0 s       4.0 s
longest shot         6.0 s       6.0 s
pauses over 180 ms   1 (5/min)   0 (0/min)
silent time          4 %         0 %
words/min (est.)     10.0 est.   5.0 est.
loudness             -21.9 LUFS  -21.8 LUFS

cuts per 5 s window
                     0-5      5-10     10-12
draft12              0        1        0
outlier12            1        1        0

DEAD AIR 2.01-2.51 s (500 ms)
NO CUT 0.0-6.0 s (6.0 s)
NO CUT 6.0-12.0 s (6.0 s)
NO PAYOFF 8-10 s: the outliers have a cut here, the draft does not
NO CUT 0.0-5.0 s: 1 of 1 outliers cut here, the draft does not
PAUSES 5.0 per minute against 0.0 in the outliers
```

## reelctl understand

Pick Reel-sized clips from a long video and label its cuts, with a hosted vision-language model. It needs the video's `edit_dna.json` from `decompile` **with a whisper model** (the words, silence gaps, cuts, and duration are used; the audio is not decoded again) and an OpenRouter API key.

```bash
reelctl decompile --input talk.mp4 --out talk.edit_dna.json --whisper-model ~/models/whisper/ggml-apex-hinglish-q8_0.bin
set -a; source .env; set +a          # exports OPENROUTER_API_KEY
reelctl understand --input talk.mp4 --dna talk.edit_dna.json --out talk.understanding.json --dry-run
reelctl understand --input talk.mp4 --dna talk.edit_dna.json --out talk.understanding.json
```

| Flag | Default | Meaning |
|---|---|---|
| `--input <path>` | required | The long video (the one `--dna` came from) |
| `--dna <path>` | required | Its `edit_dna.json`, made with whisper (`asr_backend` is `whisper_cpp`) |
| `--out <path>` | required | Where to write `understanding.json` ([fields](understanding.md)) |
| `--model <id>` | `qwen/qwen3.5-9b` | OpenRouter model. `meta/muse-spark-1.2 --quant unknown` is the higher-quality option |
| `--quant <list>` | `fp8,bf16,fp16` | Provider quantizations allowed, comma-separated. If no endpoint of the model has one of them, the command fails and lists the providers that exist; it never retries with looser settings |
| `--every-s <n>` | `5` | Seconds between sampled frames. A frame is also taken 300 ms before and after every cut |
| `--sheet <auto\|1\|2\|3>` | `auto` | Single frames, or tiles of 2×2 or 3×3. `auto` tiles only when the request would not fit the model's context (the 28.6-minute lecture: 346 frames in 87 sheets of 2×2) |
| `--effort <low\|medium\|high>` | none | Reasoning effort, for models that take one |
| `--max-cost-usd <n>` | `0.50` | Refuse, with exit code 4, when the cost upper bound is over this |
| `--dry-run` | off | Plan only: print the plan and the bound, and do not call the model. It needs no API key, but fetches the model's public price listing |
| `--decoder <auto\|software>` | `auto` | As for `decompile` |

Without `--dry-run`, stdout gets two lines, the plan and the result:

```
frames=346 images=87 sheet=2x2 segments=535 context_tokens=262144 input_tokens_estimate=139315 cost_upper_bound_usd=0.0277 max_cost_usd=0.50
clips=3 rejected=1 model=qwen/qwen3.5-9b provider=Parasail cost_usd=0.0082 out=Reels/out/understand_runs/qwen35-9b.understanding.json
```

`cost_upper_bound_usd` is an upper bound: it prices every input token at the highest eligible price and counts 1,445 tokens per image or sheet. Both lines above are real output for the 28.6-minute lecture (Qwen3.5-9B, 2026-10-28; the run wrote its file under `Reels/out/`). The first is the `--dry-run` of the final prompt. The second is from the first live run, which used the first prompt: its bound was $0.0276 and the response reported $0.0082, with 65,852 input tokens against the 139,077 estimated then, so the bound overestimates by about 2×. The 87 sheets and the transcript together billed 65,852 tokens, so a 2×2 sheet billed at most 757, well under 1,445.

What is sent to the model: a system prompt built for this video (its clip-length limit is counted in this video's segments, as a span of at most M segments counting both ends; a video under 70 s asks for one candidate with no 45–60 s aim; see [ADR-016](decisions/ADR-016-state-the-clip-length-in-segments-not-seconds.md)), the sampled frames (448 px JPEGs, each preceded by its time) and the transcript as numbered segments. Every request carries `data_collection: deny` and `require_parameters: true`. The API key is read only from the environment, sent only as a header, and never printed or written.

Two flags are hidden from `--help`, like `whisper-worker`, and exist so the tests run with no network: `--replay <reply.json>` uses a recorded model reply instead of calling the model (no key needed), and `--catalog <catalog.json>` uses a model listing from a file instead of fetching it.

Exit codes specific to this command: 2 for `--every-s 0`, a `--dna` with a duration of 0, or a `--dna` without whisper words or with no words at all (all checked before any network access, so no price listing is fetched), 3 when `OPENROUTER_API_KEY` is not set (and neither `--dry-run` nor `--replay` was given), 4 when the cost upper bound is over `--max-cost-usd`. A failed model call (an HTTP error, no matching provider, a reply that is not the schema, a reply cut off at `max_tokens`) exits with 1 and says which.

## reelctl look

```
reelctl look --draft draft.mp4 --outlier outlier.mp4 --out outlier.cube [--size 17|33|65] [--samples 60] [--decoder auto|software]
```

Writes a `.cube` 3D LUT that moves the draft's colour toward the outlier's. It samples `--samples` frames (1 to 600, default 60) evenly from each video at 320 px wide, builds a 256-level histogram per colour channel from each, and maps the draft's cumulative distribution onto the outlier's channel by channel. The file is Adobe/Resolve `.cube` (`LUT_3D_SIZE`, red varying fastest) and loads with `ffmpeg -vf lut3d=outlier.cube`. It does not run on the timed `decompile` path and adds nothing to `signal_ms`.

One line on stdout: `size=33 samples_draft=60 samples_outlier=60 mean_shift_r=-17.70 mean_shift_g=2.33 mean_shift_b=0.27 out=outlier.cube`. The three shifts are the change the LUT makes to each channel's mean over the draft's sampled frames, on a 0–255 scale. Identical inputs give an identity LUT. The same inputs give the same bytes.

Exit codes: 2 for a bad argument or a missing or unreadable file (checked before any decoding), 1 for a file that does not decode. No `.cube` is written on failure.

**Limits.** The match is separable and per channel in RGB: it moves tone and colour cast, and cannot reproduce split toning, a hue-selective grade or a saturation change. Colours use libswscale's BT.601 defaults and HDR is not tone-mapped. It matches distributions, not looks: on footage of the same kind (talking head to talking head, or a clip against a graded copy of itself) it transfers the grade; between very different content, such as a photographic clip and a flat-colour graphic reel, it transfers the content's colour statistics and posterizes or tints the result (see [status](status.md#cube-lut-2026-10-28)).
## Editing: timeline, transform, funnel, render, qa

Deterministic edits over an OpenTimelineIO project. Each command reads one `.otio`, applies one change, and writes one (atomically; `--out` may equal `--project`). Running a command twice with the same arguments gives the same file. The project format and metadata keys are in [timeline.md](timeline.md). `init` needs `ffprobe`; `render` and `qa inspect-boundaries` need `ffmpeg`; a missing tool exits 1 and names it.

```
reelctl timeline init --input take.mp4 [--dna take.edit_dna.json] --out edit.otio
reelctl timeline cut-silence --project edit.otio --dna take.edit_dna.json [--min-gap-ms 180] [--fade-ms 30] [--fillers um,uh] --out edit.otio
reelctl transform punch-in --project edit.otio --start-ms T1 --end-ms T2 --scale 1.18 --out edit.otio
reelctl funnel attach-bridge --project edit.otio --target-url URL [--cutoff-pct 0.72] [--card-ms 2500, at most 60000] [--dna take.edit_dna.json] --out edit.otio
reelctl render --project edit.otio --out out.mp4 [--size 1080x1920]
reelctl qa inspect-boundaries --project edit.otio --out-dir views/
```

| Command | Stdout line |
|---|---|
| `timeline init` | `init: out= frames= fps= size= audio=` |
| `timeline cut-silence` | `cut-silence: out= removed_spans= removed_ms= timeline_ms=` |
| `transform punch-in` | `punch-in: out= frames=a..b clips= scale=` |
| `funnel attach-bridge` | `attach-bridge: out= cut_ms= in_pause= card_ms= unchanged=` |
| `render` | `render: out= frames= duration_ms= size= ffmpeg_ms=` |
| `qa inspect-boundaries` | `inspect-boundaries: out_dir= joins=` |

- `cut-silence` removes every silence gap of at least `--min-gap-ms` and, with whisper words in the DNA, whole words that equal a `--fillers` entry. Times come from the DNA and snap to the frame grid into the pause, never into a word. The audio gets a `--fade-ms` dip at each join (a fade-out and a fade-in, not an overlapping crossfade, so the audio keeps pace with the picture).
- `punch-in` needs `1 < --scale <= 2` and a span of at least 12 frames: a centred zoom ramping in over 6 frames, held, and ramping out. A second punch-in over the same pieces replaces the first.
- `attach-bridge` cuts at `--cutoff-pct` of the timeline (in a pause if the DNA has one within 400 ms after, or if `cut-silence` left a join there; the DNA must be the media's length within 100 ms) and appends a card showing the URL, drawn with a built-in bitmap font (capitals, digits and a few symbols; other characters show as a box), because this ffmpeg build may lack `drawtext`. A different bridge on a project that already has one is refused.
- `render` writes H.264 + AAC and scales and crops to `--size` (even sides, at most 8192 each and 33177600 pixels in all; default the source size). `--out` needs an extension and may not be a directory. It writes `.<name>.<pid>.tmp.<ext>` beside `--out`, renames it, and removes it on any failure. A card URL too long for the frame is cut with `...` and a second stdout line says how many characters show.
- Media paths must be plain local files (no `://`, `concat:` or `pipe:`; a directory or device is refused); `ffmpeg` and `ffprobe` run with only the `file` protocol allowed.
- Exit 2: a bad argument, an unreadable or invalid project or DNA, a media file that is missing, not a regular file or not a local path, a file `ffprobe` cannot read, a span outside the timeline, a project with no frames or with values out of range, an oversize `--size` or `--card-ms`, or a DNA that is not the media's. Exit 1: `ffmpeg` or `ffprobe` missing, `ffmpeg` failing, or an `--out` that cannot be written.

## reelctl dataset build

```bash
reelctl dataset build --dna <dir|edit_dna.json>... [--understanding <understanding.json>...] --out <dir> [--val-percent 10] [--salt reelctl-dataset-v1]
```

Writes instruction-tuning records for a later fine-tune ([finetune.md](finetune.md), [spec](specs/p3-dataset-eval.md)). Offline: no network, no model. The output is `train.jsonl`, `val.jsonl` and `manifest.json` in `--out` (created if missing; files of those names are replaced through a temporary file; other files are left alone). It prints one line, `wrote N records (a train, b val) from R reels, S skipped, to <out>`, and one `skipped ...` or `warning: ...` line on stderr for each skip and warning.

- `--dna`: a directory (every regular file directly inside it whose name ends `.edit_dna.json`, not recursive) or files; repeatable. The reel id is the file name without `.edit_dna.json`. A file whose id has characters other than `A-Za-z0-9._-` (or is empty, starts with a dot, or is over 96 characters) is skipped as `bad_reel_id`; two files with one id are an error.
- `--understanding`: `understanding.json` files. A run joins the reel whose id is the file stem of its `source.path` (never copied) and whose duration matches the `edit_dna.json`'s. It adds a `clip_picks` record of the clips that met every clip rule; flagged clips and all model prose are left out and counted.
- `--val-percent 0..50` and `--salt`: the split is by reel, from the salt and the reel id only, so every record of a reel is in one split and adding reels never moves one. `--val-percent 0` leaves `val` empty.
- Records: `edit_events` and `pacing` for each reel, `clip_picks` for each usable understanding run. The record fields, the targets, and the reasons a record is skipped are in [finetune.md](finetune.md#the-data-format-the-collector-writes) and the [spec](specs/p3-dataset-eval.md). Output is byte-identical for the same inputs whatever order they are given in.
- An `edit_dna.json` whose `schema_version` is missing, 0 or above 4, or an `understanding.json` whose is not 1, is an error (exit 1) and nothing is written. Unparseable files are errors too.
- Exit 2: `--val-percent` over 50, a `--dna` directory with no `.edit_dna.json`, or an input that is not a regular file. Exit 1: unreadable or unusable input, duplicate reel ids, or an output that cannot be written.

## reelctl eval

```bash
reelctl eval --truth <understanding.json | dataset dir | .jsonl> --answers <file> [--dna <edit_dna.json>] [--split val|train|all] [--cut-tolerance-ms 100] [--event-tolerance-ms 250] [--format json|text]
```

Scores a model's answers against measured truth; reads files only, never calls a model ([spec](specs/p3-dataset-eval.md) defines every metric, its ties and its degenerate cases).

- **Truth is an `understanding.json`:** `--dna` of the same video is required (exit 2 without it) because the clip rules need the measured pauses; the durations must match (exit 1 otherwise). `--answers` is a model reply: the file `reelctl understand --replay` reads, or a bare reply. Only `reel_candidates` is read. `--split` is ignored.
- **Truth is a dataset** (the directory `dataset build` wrote, or one of its `.jsonl` files): `--answers` is JSON lines `{"id": "<record id>", "answer": <shaped like the task's target>}`. `--split` (default `val`) picks the records. Records with no line are `missing`; a line that does not parse is `unparseable` (listed in `problems`, the first 20) and scored as no answer; a duplicate id is an error; a line whose id is not among the scored records, including records of the other split, is counted in `extra_answers` and ignored. A split with no records is an error (exit 1), not a score.
- **Report:** JSON by default, or `--format text`. `eval_schema_version`, `truth_kind`, `split`, `records`, `answered`, `missing`, `unparseable`, `extra_answers`, `problems`, `tolerances`, then `clip_picks`, `edit_events` and `pacing` where the truth has them. `clip_picks` has `valid_rate` and `rule_pass_rate` (about the answer alone), then agreement with the named teacher: `hook`, `payoff_given_hook`, `cutoff_given_hook` and `interval` (IoU at least 0.5). `edit_events` has `cuts` and `visual_events` (precision, recall, F1, timing error), `scale` and `zoom_curves` errors. `pacing` has mean absolute error and exact rate per field and the payoff accuracy. A score that cannot be computed (nothing to score) is `null`, never 0 or NaN. Two reports are compared by diffing them; nothing in them is a pass mark.
- `--cut-tolerance-ms` and `--event-tolerance-ms`: times this close (inclusive) are the same cut, or the same event of one kind.
- Exit 2: an `understanding.json` without `--dna`. Exit 1: an unreadable or malformed file, an unknown schema version, a duration mismatch, an empty split, a duplicate answer id.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Runtime error. stderr gets `error: ` and the cause chain, for example `error: decompile: open input: No such file or directory` or `error: decompile: whisper: model not found at /missing.bin` |
| 2 | Invalid arguments, including more than three `--outlier` files or none, or a `--dna` for `understand` without whisper words, or an `eval` of an `understanding.json` without `--dna`, or `--val-percent` over 50; clap prints the usage |
| 3 | `reelctl understand` without `OPENROUTER_API_KEY` |
| 4 | `reelctl understand` with a cost upper bound over `--max-cost-usd` |

## Input requirements

- HEVC decodes on the media engine on macOS. It has a fixed cost of about 160 ms, so a short, small HEVC clip is faster with `--decoder software`; a 1080p recording is faster with `auto` (87 s: 4.0–5.3 s against 8.5 s).
- The file needs a video stream. If there are several video or audio streams, the one FFmpeg ranks best is used.
- Video must decode to 4:2:0: 8-bit (`yuv420p`, `yuvj420p`, or `nv12`) or 10-bit (`yuv420p10le`, as iPhone HEVC Main 10 recordings do, or `p010le`, which a hardware decoder returns). 10-bit samples are read as their top 8 bits, a frame is read as full range if it is `yuvj420p` or its metadata says so, and HDR (HLG or PQ) is not tone-mapped; `decode_note` says when either applies ([ADR-011](decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)). Anything else, such as 4:2:2, 4:4:4, or 12-bit, stops with an error that names the format, for example `error: decompile: decode video frame: unsupported pixel format yuv422p; decompile reads 4:2:0 video as yuv420p, yuvj420p, nv12, yuv420p10le, or p010le`.
- The frame size may change part-way through a stream. `frame_size` is the last frame's, and the scene score compares frames of different sizes by position in the picture.
- Audio must decode to 16-bit integer or 32-bit float samples, as AAC and 16-bit PCM do. It is mixed to mono and, if it is not 48 kHz, resampled to 48 kHz by linear interpolation as one stream, so a moment at time t in the input is at t in the output. There is no low-pass filter, so audio sampled above 48 kHz (88.2 or 96 kHz) can alias.
- Without audio, `silence_gaps` and `words` are empty, `integrated_lufs` is −70, and `words_per_minute` is 0.
