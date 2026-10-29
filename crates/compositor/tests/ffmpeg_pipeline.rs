//! The whole run with the real `ffmpeg` and `ffprobe` command line tools and a fake
//! model: decode, matte, composite, encode. No weights and no ONNX Runtime. The
//! clips are made here with `ffmpeg`'s lavfi sources, so nothing is stored.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use compositor::{
    isolate_presenter, BackgroundSpec, CompositorError, FrameRgb, IsolateRequest, MattingModel,
    ModelOutput, OutputMode, RecurrentState,
};

const W: usize = 64;
const H: usize = 48;
const LEFT: [f32; 3] = [192.0, 48.0, 32.0];
const RIGHT: [f32; 3] = [32.0, 96.0, 192.0];

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("compositor-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn ffmpeg(args: &[&str]) {
    let output = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(args)
        .output()
        .expect("spawn ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn path(path: &Path) -> &str {
    path.to_str().expect("utf8 path")
}

/// 25 frames, 64x48: the left half is `LEFT` (0xc03020), the right half `RIGHT`
/// (0x2060c0), over a sine tone.
fn source(dir: &Path) -> PathBuf {
    let out = dir.join("source.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=0xc03020:s=32x48:r=25:d=1",
        "-f",
        "lavfi",
        "-i",
        "color=c=0x2060c0:s=32x48:r=25:d=1",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=1",
        "-filter_complex",
        "[0:v][1:v]hstack[v]",
        "-map",
        "[v]",
        "-map",
        "2:a",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        path(&out),
    ]);
    out
}

fn solid_png(dir: &Path, colour: &str, size: &str) -> PathBuf {
    let out = dir.join("bg.png");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("color=c={colour}:s={size}:d=1"),
        "-frames:v",
        "1",
        path(&out),
    ]);
    out
}

fn probe(file: &Path, entries: &str, select: &str) -> String {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            select,
            "-show_entries",
            entries,
        ])
        .args(["-of", "default=nw=1:nk=1"])
        .arg(file)
        .output()
        .expect("spawn ffprobe");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Every frame of `file` as `rgb24` at its own size.
fn decode_rgb(file: &Path) -> (usize, usize, Vec<u8>) {
    let size = probe(file, "stream=width,height", "v:0");
    let mut numbers = size.lines().map(|line| line.parse::<usize>().unwrap());
    let (width, height) = (numbers.next().unwrap(), numbers.next().unwrap());
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(file)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("spawn ffmpeg");
    assert!(output.status.success());
    (width, height, output.stdout)
}

/// The mean colour of columns `x0..x1` and rows `y0..y1` of frame `index`.
fn mean(
    frames: &[u8],
    width: usize,
    height: usize,
    index: usize,
    x: (usize, usize),
    y: (usize, usize),
) -> [f32; 3] {
    let frame = &frames[index * 3 * width * height..][..3 * width * height];
    let mut sum = [0f32; 3];
    let mut count = 0f32;
    for row in y.0..y.1 {
        for column in x.0..x.1 {
            for (channel, total) in sum.iter_mut().enumerate() {
                *total += f32::from(frame[3 * (row * width + column) + channel]);
            }
            count += 1.0;
        }
    }
    sum.map(|total| total / count)
}

fn near(got: [f32; 3], want: [f32; 3], tolerance: f32) {
    for channel in 0..3 {
        assert!(
            (got[channel] - want[channel]).abs() <= tolerance,
            "got {got:?}, wanted {want:?} within {tolerance}"
        );
    }
}

/// The foreground is the source; alpha is 1 on the left half and 0 on the right.
struct Split {
    fail_at: Option<usize>,
    calls: usize,
}

impl Split {
    fn new() -> Self {
        Self {
            fail_at: None,
            calls: 0,
        }
    }
}

impl MattingModel for Split {
    fn matte(
        &mut self,
        frame: &FrameRgb,
        _ratio: f32,
        _state: &RecurrentState,
    ) -> Result<ModelOutput, CompositorError> {
        let call = self.calls;
        self.calls += 1;
        if self.fail_at == Some(call) {
            return Err(CompositorError::run("the fake model failed"));
        }
        let pha = (0..frame.width * frame.height)
            .map(|i| {
                if i % frame.width < frame.width / 2 {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        Ok(ModelOutput {
            fgr: frame.data.clone(),
            pha,
            state: RecurrentState::initial(),
        })
    }
}

/// The foreground is the source; alpha is the same everywhere.
struct Uniform(f32);

impl MattingModel for Uniform {
    fn matte(
        &mut self,
        frame: &FrameRgb,
        _ratio: f32,
        _state: &RecurrentState,
    ) -> Result<ModelOutput, CompositorError> {
        Ok(ModelOutput {
            fgr: frame.data.clone(),
            pha: vec![self.0; frame.width * frame.height],
            state: RecurrentState::initial(),
        })
    }
}

fn request(
    input: &Path,
    background: Option<BackgroundSpec>,
    out: &Path,
    mode: OutputMode,
) -> IsolateRequest {
    IsolateRequest {
        input: input.to_path_buf(),
        background,
        out: out.to_path_buf(),
        mode,
        ratio: None,
    }
}

#[test]
fn the_presenter_half_keeps_its_colours_and_the_rest_is_the_background() {
    let dir = scratch("solid");
    let out = dir.join("out.mp4");
    let report = isolate_presenter(
        &request(
            &source(&dir),
            Some(BackgroundSpec::Colour([0, 255, 0])),
            &out,
            OutputMode::Composite,
        ),
        &mut Split::new(),
    )
    .unwrap();
    assert_eq!(report.frames, 25);
    assert_eq!((report.width, report.height), (W as u32, H as u32));
    // 64x48 is under 512: the model sees the full frame.
    assert_eq!(report.ratio, 1.0);
    let (w, h, frames) = decode_rgb(&out);
    assert_eq!((w, h), (W, H));
    assert_eq!(frames.len(), 25 * 3 * W * H);
    for index in [0, 12, 24] {
        near(mean(&frames, w, h, index, (4, 28), (4, 44)), LEFT, 8.0);
        near(
            mean(&frames, w, h, index, (36, 60), (4, 44)),
            [0.0, 255.0, 0.0],
            12.0,
        );
    }
    // The output carries the original audio.
    assert_eq!(probe(&out, "stream=codec_name", "a"), "aac");
    assert_eq!(probe(&out, "stream=codec_name", "v"), "h264");
}

#[test]
fn an_opaque_presenter_returns_the_input_colours_so_there_is_no_matrix_shift() {
    let dir = scratch("opaque");
    let out = dir.join("out.mp4");
    isolate_presenter(
        &request(
            &source(&dir),
            Some(BackgroundSpec::Colour([255, 255, 255])),
            &out,
            OutputMode::Composite,
        ),
        &mut Uniform(1.0),
    )
    .unwrap();
    let (w, h, frames) = decode_rgb(&out);
    near(mean(&frames, w, h, 10, (4, 28), (4, 44)), LEFT, 5.0);
    near(mean(&frames, w, h, 10, (36, 60), (4, 44)), RIGHT, 5.0);
}

#[test]
fn the_output_is_tagged_bt709_limited_range_so_players_read_the_colours_right() {
    let dir = scratch("tags");
    let out = dir.join("out.mp4");
    isolate_presenter(
        &request(
            &source(&dir),
            Some(BackgroundSpec::Colour([255, 255, 255])),
            &out,
            OutputMode::Composite,
        ),
        &mut Uniform(1.0),
    )
    .unwrap();
    for (field, want) in [
        ("color_space", "bt709"),
        ("color_primaries", "bt709"),
        ("color_transfer", "bt709"),
        ("color_range", "tv"),
        ("pix_fmt", "yuv420p"),
    ] {
        assert_eq!(
            probe(&out, &format!("stream={field}"), "v"),
            want,
            "{field}"
        );
    }
}

#[test]
fn a_transparent_presenter_shows_only_the_background() {
    let dir = scratch("transparent");
    let out = dir.join("out.mp4");
    isolate_presenter(
        &request(
            &source(&dir),
            Some(BackgroundSpec::Colour([200, 100, 50])),
            &out,
            OutputMode::Composite,
        ),
        &mut Uniform(0.0),
    )
    .unwrap();
    let (w, h, frames) = decode_rgb(&out);
    near(
        mean(&frames, w, h, 3, (2, 62), (2, 46)),
        [200.0, 100.0, 50.0],
        6.0,
    );
}

#[test]
fn an_image_background_is_cropped_to_cover_not_stretched() {
    let dir = scratch("image");
    // 256x48: black, a white band exactly as wide as the output in the middle, black.
    let bg = dir.join("bg.png");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=256x48:d=1",
        "-vf",
        "drawbox=x=96:y=0:w=64:h=48:color=white:t=fill",
        "-frames:v",
        "1",
        path(&bg),
    ]);
    let out = dir.join("out.mp4");
    isolate_presenter(
        &request(
            &source(&dir),
            Some(BackgroundSpec::File(bg)),
            &out,
            OutputMode::Composite,
        ),
        &mut Uniform(0.0),
    )
    .unwrap();
    let (w, h, frames) = decode_rgb(&out);
    // Stretched, the white band would be a quarter of the width; cropped at the
    // centre it is all of it.
    near(
        mean(&frames, w, h, 5, (2, 62), (2, 46)),
        [255.0, 255.0, 255.0],
        8.0,
    );
}

#[test]
fn a_short_video_background_loops_for_the_whole_clip() {
    let dir = scratch("video");
    let bg = dir.join("bg.mp4");
    // 5 frames of one colour; the presenter clip is 25.
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=0x00a0ff:s=64x48:r=25:d=0.2",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        path(&bg),
    ]);
    let out = dir.join("out.mp4");
    let report = isolate_presenter(
        &request(
            &source(&dir),
            Some(BackgroundSpec::File(bg)),
            &out,
            OutputMode::Composite,
        ),
        &mut Split::new(),
    )
    .unwrap();
    assert_eq!(report.frames, 25);
    let (w, h, frames) = decode_rgb(&out);
    for index in [0, 7, 24] {
        near(
            mean(&frames, w, h, index, (36, 60), (4, 44)),
            [0.0, 160.0, 255.0],
            12.0,
        );
    }
}

#[test]
fn an_image_background_repeats_for_every_frame() {
    let dir = scratch("png");
    let out = dir.join("out.mp4");
    let bg = solid_png(&dir, "0x30a050", "100x60");
    let report = isolate_presenter(
        &request(
            &source(&dir),
            Some(BackgroundSpec::File(bg)),
            &out,
            OutputMode::Composite,
        ),
        &mut Uniform(0.0),
    )
    .unwrap();
    assert_eq!(report.frames, 25);
    let (w, h, frames) = decode_rgb(&out);
    near(
        mean(&frames, w, h, 24, (2, 62), (2, 46)),
        [48.0, 160.0, 80.0],
        8.0,
    );
}

#[test]
fn the_matte_mode_writes_the_alpha_as_limited_range_luma_with_no_audio() {
    let dir = scratch("matte");
    let out = dir.join("matte.mp4");
    let report = isolate_presenter(
        &request(&source(&dir), None, &out, OutputMode::Matte),
        &mut Split::new(),
    )
    .unwrap();
    assert_eq!(report.frames, 25);
    assert_eq!(probe(&out, "stream=codec_type", "a"), "");
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output()
        .unwrap();
    let luma = |x: usize, y: usize| f32::from(output.stdout[10 * (W * H * 3 / 2) + y * W + x]);
    assert!((luma(10, 20) - 235.0).abs() <= 6.0, "{}", luma(10, 20));
    assert!((luma(50, 20) - 16.0).abs() <= 6.0, "{}", luma(50, 20));
}

#[test]
fn the_rgba_mode_keeps_the_alpha_in_a_prores_4444_mov() {
    let dir = scratch("rgba");
    let out = dir.join("presenter.mov");
    let report = isolate_presenter(
        &request(&source(&dir), None, &out, OutputMode::Rgba),
        &mut Split::new(),
    )
    .unwrap();
    assert_eq!(report.frames, 25);
    assert_eq!(probe(&out, "stream=codec_name", "v"), "prores");
    assert_eq!(probe(&out, "stream=pix_fmt", "v"), "yuva444p12le");
    assert_eq!(probe(&out, "stream=codec_name", "a"), "pcm_s16le");
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .output()
        .unwrap();
    let pixel = |x: usize, y: usize| {
        let at = 4 * (10 * W * H + y * W + x);
        [
            f32::from(output.stdout[at]),
            f32::from(output.stdout[at + 1]),
            f32::from(output.stdout[at + 2]),
            f32::from(output.stdout[at + 3]),
        ]
    };
    let left = pixel(10, 20);
    near([left[0], left[1], left[2]], LEFT, 6.0);
    assert!(left[3] >= 250.0, "{left:?}");
    assert!(pixel(50, 20)[3] <= 5.0, "{:?}", pixel(50, 20));
}

#[test]
fn a_variable_rate_input_comes_out_at_a_constant_rate_and_the_same_length() {
    let dir = scratch("vfr");
    let input = dir.join("vfr.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=s=64x48:r=30:d=2",
        "-vf",
        "setpts='if(lt(N,30),N*2,60+(N-30))/(30*TB)'",
        "-fps_mode",
        "passthrough",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        path(&input),
    ]);
    let out = dir.join("out.mp4");
    let report = isolate_presenter(
        &request(
            &input,
            Some(BackgroundSpec::Colour([0, 0, 0])),
            &out,
            OutputMode::Composite,
        ),
        &mut Uniform(1.0),
    )
    .unwrap();
    // 60 frames over 3 s: the average is 450/23 = 19.57 fps.
    assert!((report.fps - 450.0 / 23.0).abs() < 0.01, "{}", report.fps);
    assert!((58..=60).contains(&report.frames), "{}", report.frames);
    assert_eq!(
        probe(&out, "stream=r_frame_rate", "v"),
        probe(&out, "stream=avg_frame_rate", "v"),
        "constant rate"
    );
    let seconds: f64 = probe(&out, "stream=duration", "v").parse().unwrap();
    assert!((seconds - 3.0).abs() < 0.1, "{seconds}");
}

#[test]
fn a_rotated_phone_clip_is_matted_upright() {
    let dir = scratch("rotated");
    let plain = dir.join("plain.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=s=64x48:r=25:d=1",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        path(&plain),
    ]);
    let rotated = dir.join("rotated.mp4");
    ffmpeg(&[
        "-display_rotation",
        "90",
        "-i",
        path(&plain),
        "-c",
        "copy",
        path(&rotated),
    ]);
    let out = dir.join("out.mp4");
    let report = isolate_presenter(
        &request(
            &rotated,
            Some(BackgroundSpec::Colour([0, 0, 0])),
            &out,
            OutputMode::Composite,
        ),
        &mut Uniform(1.0),
    )
    .unwrap();
    assert_eq!((report.width, report.height), (48, 64));
    let (w, h, frames) = decode_rgb(&out);
    assert_eq!((w, h), (48, 64));
    assert_eq!(frames.len(), 25 * 3 * 48 * 64);
}

#[test]
fn an_odd_sized_input_is_cropped_to_even_sides_by_the_encoder() {
    let dir = scratch("odd");
    let input = dir.join("odd.mkv");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=s=65x49:r=25:d=1",
        "-pix_fmt",
        "yuv444p",
        "-c:v",
        "ffv1",
        path(&input),
    ]);
    let out = dir.join("out.mp4");
    let report = isolate_presenter(
        &request(
            &input,
            Some(BackgroundSpec::Colour([0, 0, 0])),
            &out,
            OutputMode::Composite,
        ),
        &mut Uniform(1.0),
    )
    .unwrap();
    assert_eq!((report.width, report.height), (65, 49));
    let (w, h, _) = decode_rgb(&out);
    assert_eq!((w, h), (64, 48));
}

#[test]
fn a_model_failure_leaves_no_output_and_no_temporary_file() {
    let dir = scratch("failure");
    let out_dir = dir.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let out = out_dir.join("out.mp4");
    let mut model = Split::new();
    model.fail_at = Some(3);
    let error = isolate_presenter(
        &request(
            &source(&dir),
            Some(BackgroundSpec::Colour([0, 0, 0])),
            &out,
            OutputMode::Composite,
        ),
        &mut model,
    )
    .unwrap_err();
    assert!(!error.is_setup());
    assert!(
        error.to_string().contains("frame 3: the fake model failed"),
        "{error}"
    );
    assert!(!out.exists());
    let left: Vec<_> = fs::read_dir(&out_dir).unwrap().collect();
    assert!(left.is_empty(), "left behind: {left:?}");
}

#[test]
fn an_existing_output_is_replaced_only_by_a_complete_run() {
    let dir = scratch("replace");
    let out = dir.join("out.mp4");
    fs::write(&out, b"previous").unwrap();
    let mut model = Split::new();
    model.fail_at = Some(2);
    let colour = Some(BackgroundSpec::Colour([0, 0, 0]));
    let input = source(&dir);
    isolate_presenter(
        &request(&input, colour.clone(), &out, OutputMode::Composite),
        &mut model,
    )
    .unwrap_err();
    assert_eq!(fs::read(&out).unwrap(), b"previous");
    isolate_presenter(
        &request(&input, colour, &out, OutputMode::Composite),
        &mut Split::new(),
    )
    .unwrap();
    assert!(fs::metadata(&out).unwrap().len() > 100);
}

#[test]
fn a_file_ffmpeg_cannot_decode_is_a_setup_error_before_any_model_runs() {
    let dir = scratch("garbage");
    let input = dir.join("not-a-video.mp4");
    fs::write(&input, b"this is not a video").unwrap();
    let mut model = Split::new();
    let error = isolate_presenter(
        &request(
            &input,
            Some(BackgroundSpec::Colour([0, 0, 0])),
            &dir.join("out.mp4"),
            OutputMode::Composite,
        ),
        &mut model,
    )
    .unwrap_err();
    assert!(error.is_setup(), "{error}");
    assert!(error.to_string().contains("ffprobe cannot read"), "{error}");
    assert_eq!(model.calls, 0);
}
