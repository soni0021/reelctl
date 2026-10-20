//! Demux and decode with ffmpeg-next.
//!
//! VideoToolbox was measured on a 60-second 1080p fixture:
//! `ffmpeg -hwaccel videotoolbox` took 3.8s, multithreaded software decode took
//! 0.38s. ffmpeg-next does not expose a VideoToolbox device wrapper, so the
//! structural path uses libavcodec frame threading and skips the in-loop
//! deblock on non-reference frames. Scene score, histograms, and filmstrip
//! thumbs are sampled from the decoded luma/chroma planes. There is no ffmpeg
//! process spawn on this path.

use std::path::Path;
use std::sync::OnceLock;

use ffmpeg_next::decoder::Audio as AudioDecoder;
use ffmpeg_next::decoder::Video as VideoDecoder;
use ffmpeg_next::frame::Audio as AudioFrame;
use ffmpeg_next::frame::Video as VideoFrame;
use ffmpeg_next::media::Type as MediaType;
use ffmpeg_next::util::format::sample::Type as SampleType;
use ffmpeg_next::util::format::Pixel;
use ffmpeg_next::util::format::Sample;
use ffmpeg_next::{self as ffmpeg, Discard, Rational};

use crate::error::DecompileError;
use crate::histogram::{yuv601_to_rgb, FrameColor};
use crate::scene::{ffmpeg_scene_score, ScoredFrame, SCENE_SCORE_THRESHOLD};

const SAMPLE_RATE: u32 = 48_000;
const SAD_TARGET_W: u32 = 160;
const SAD_TARGET_H: u32 = 90;
pub(crate) const THUMB_W: u32 = 64;
pub(crate) const THUMB_H: u32 = 36;
const BEFORE_FRAMES: usize = 4;
const AFTER_FRAMES: usize = 4;
const HIST_TARGET_W: u32 = 32;
const HIST_TARGET_H: u32 = 18;

pub(crate) const DECODE_NOTE: &str = "\
libavcodec frame-threaded software decode (thread_count auto, \
skip_loop_filter=nonref). VideoToolbox hwaccel was slower on the 60s 1080p \
fixture (3.8s vs 0.38s) and ffmpeg-next does not expose a hwdevice wrapper. \
Scene score and color are sampled from decoded planes; the hot path does not \
shell out to ffmpeg.";

pub(crate) struct DecodedReel {
    pub duration_ms: u64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub frames: Vec<ScoredFrame>,
    pub colors: Vec<FrameColor>,
    pub pcm: Vec<f32>,
    pub sample_rate: u32,
    pub boundaries: Vec<RawBoundary>,
}

pub(crate) struct RawBoundary {
    pub frame_index: u64,
    pub time_ms: u64,
    pub before: Vec<Vec<u8>>,
    pub after: Vec<Vec<u8>>,
}

pub(crate) fn decode_file(path: &Path) -> Result<DecodedReel, DecompileError> {
    ensure_ffmpeg().map_err(|err| stage("ffmpeg init", err))?;
    let mut input = ffmpeg::format::input(path).map_err(|err| stage("open input", err))?;
    let video_index = input
        .streams()
        .best(MediaType::Video)
        .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?
        .index();
    let audio_index = input
        .streams()
        .best(MediaType::Audio)
        .map(|stream| stream.index());

    let (time_base, start_time, fps_rational) = {
        let stream = input
            .streams()
            .find(|stream| stream.index() == video_index)
            .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?;
        let fps = positive_rational(stream.avg_frame_rate())
            .or_else(|| positive_rational(stream.rate()))
            .unwrap_or(Rational::new(30, 1));
        (stream.time_base(), stream.start_time(), fps)
    };
    let container_ms = {
        let ticks = input.duration();
        if ticks > 0 {
            (ticks / 1_000) as u64
        } else {
            0
        }
    };

    let mut video = {
        let stream = input
            .streams()
            .find(|stream| stream.index() == video_index)
            .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?;
        open_video(stream.parameters()).map_err(|err| stage("open video decoder", err))?
    };
    let mut audio = if let Some(index) = audio_index {
        let stream = input.streams().find(|stream| stream.index() == index);
        match stream {
            Some(stream) => Some(
                open_audio(stream.parameters()).map_err(|err| stage("open audio decoder", err))?,
            ),
            None => None,
        }
    } else {
        None
    };

    let expected_frames = expected_frame_count(container_ms, fps_rational);
    let mut scores = Vec::with_capacity(expected_frames);
    let mut colors = Vec::with_capacity(expected_frames);
    let mut pcm = Vec::with_capacity(expected_pcm(container_ms));
    let mut sad = SadState::new();
    let thumb_bytes = (THUMB_W * THUMB_H * 3) as usize;
    let mut ring = ThumbRing::new(BEFORE_FRAMES, thumb_bytes);
    let mut pending: Option<Pending> = None;
    let mut boundaries = Vec::new();
    let mut video_frame = VideoFrame::empty();
    let mut audio_frame = AudioFrame::empty();
    let mut width = video.width();
    let mut height = video.height();
    let mut last_time_ms = 0u64;

    for (stream, packet) in input.packets() {
        if stream.index() == video_index {
            send_video(&mut video, &packet).map_err(|err| stage("send video packet", err))?;
            drain_video(
                &mut video,
                &mut video_frame,
                &mut sad,
                &mut ring,
                &mut pending,
                &mut boundaries,
                &mut scores,
                &mut colors,
                &mut width,
                &mut height,
                &mut last_time_ms,
                time_base,
                start_time,
                fps_rational,
            )
            .map_err(|err| stage("decode video frame", err))?;
        } else if Some(stream.index()) == audio_index {
            if let Some(decoder) = audio.as_mut() {
                send_audio(decoder, &packet).map_err(|err| stage("send audio packet", err))?;
                drain_audio(decoder, &mut audio_frame, &mut pcm)
                    .map_err(|err| stage("decode audio frame", err))?;
            }
        }
    }
    finish_video(
        &mut video,
        &mut video_frame,
        &mut sad,
        &mut ring,
        &mut pending,
        &mut boundaries,
        &mut scores,
        &mut colors,
        &mut width,
        &mut height,
        &mut last_time_ms,
        time_base,
        start_time,
        fps_rational,
    )
    .map_err(|err| stage("flush video decoder", err))?;
    if let Some(decoder) = audio.as_mut() {
        finish_audio(decoder, &mut audio_frame, &mut pcm)
            .map_err(|err| stage("flush audio decoder", err))?;
    }
    if let Some(open) = pending.take() {
        boundaries.push(open.into_raw());
    }
    if scores.is_empty() {
        return Err(DecompileError::NoFrames(path.display().to_string()));
    }

    let frame_ms = frame_duration_ms(fps_rational);
    let from_frames = last_time_ms.saturating_add(frame_ms);
    let duration_ms = container_ms.max(from_frames);
    Ok(DecodedReel {
        duration_ms,
        fps: rational_to_f64(fps_rational),
        width,
        height,
        frames: scores,
        colors,
        sample_rate: if pcm.is_empty() { 0 } else { SAMPLE_RATE },
        pcm,
        boundaries,
    })
}

fn stage(step: &str, err: impl std::fmt::Display) -> DecompileError {
    DecompileError::Message(format!("{step}: {err}"))
}

fn ensure_ffmpeg() -> Result<(), DecompileError> {
    static READY: OnceLock<Result<(), String>> = OnceLock::new();
    match READY.get_or_init(|| match ffmpeg::init() {
        Ok(()) => {
            ffmpeg::log::set_level(ffmpeg::log::Level::Quiet);
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }) {
        Ok(()) => Ok(()),
        Err(err) => Err(DecompileError::Message(err.clone())),
    }
}

fn open_video(params: ffmpeg::codec::Parameters) -> Result<VideoDecoder, DecompileError> {
    let threads = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4);
    let mut context = ffmpeg::codec::Context::from_parameters(params)?;
    context.set_threading(ffmpeg::threading::Config {
        kind: ffmpeg::threading::Type::Frame,
        count: threads,
    });
    let mut video = context.decoder().video()?;
    video.skip_loop_filter(Discard::NonReference);
    Ok(video)
}

fn open_audio(params: ffmpeg::codec::Parameters) -> Result<AudioDecoder, DecompileError> {
    let context = ffmpeg::codec::Context::from_parameters(params)?;
    Ok(context.decoder().audio()?)
}

fn send_video(video: &mut VideoDecoder, packet: &ffmpeg::Packet) -> Result<(), DecompileError> {
    match video.send_packet(packet) {
        Ok(()) => Ok(()),
        Err(err) if is_again(&err) => Err(DecompileError::Message(format!(
            "video send would block: {err}"
        ))),
        Err(err) => Err(DecompileError::Message(format!("send video packet: {err}"))),
    }
}

fn send_audio(audio: &mut AudioDecoder, packet: &ffmpeg::Packet) -> Result<(), DecompileError> {
    match audio.send_packet(packet) {
        Ok(()) => Ok(()),
        Err(err) if is_again(&err) => Err(DecompileError::Message(format!(
            "audio send would block: {err}"
        ))),
        Err(err) => Err(DecompileError::Message(format!("send audio packet: {err}"))),
    }
}

fn drain_video(
    video: &mut VideoDecoder,
    frame: &mut VideoFrame,
    sad: &mut SadState,
    ring: &mut ThumbRing,
    pending: &mut Option<Pending>,
    boundaries: &mut Vec<RawBoundary>,
    scores: &mut Vec<ScoredFrame>,
    colors: &mut Vec<FrameColor>,
    width: &mut u32,
    height: &mut u32,
    last_time_ms: &mut u64,
    time_base: Rational,
    start_time: i64,
    fps: Rational,
) -> Result<(), DecompileError> {
    loop {
        match video.receive_frame(frame) {
            Ok(()) => {
                observe_frame(
                    frame,
                    sad,
                    ring,
                    pending,
                    boundaries,
                    scores,
                    colors,
                    width,
                    height,
                    last_time_ms,
                    time_base,
                    start_time,
                    fps,
                )?;
            }
            Err(err) if is_retry(&err) => break,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn finish_video(
    video: &mut VideoDecoder,
    frame: &mut VideoFrame,
    sad: &mut SadState,
    ring: &mut ThumbRing,
    pending: &mut Option<Pending>,
    boundaries: &mut Vec<RawBoundary>,
    scores: &mut Vec<ScoredFrame>,
    colors: &mut Vec<FrameColor>,
    width: &mut u32,
    height: &mut u32,
    last_time_ms: &mut u64,
    time_base: Rational,
    start_time: i64,
    fps: Rational,
) -> Result<(), DecompileError> {
    match video.send_eof() {
        Ok(()) => {}
        Err(err) if is_retry(&err) => {}
        Err(err) => return Err(err.into()),
    }
    drain_video(
        video,
        frame,
        sad,
        ring,
        pending,
        boundaries,
        scores,
        colors,
        width,
        height,
        last_time_ms,
        time_base,
        start_time,
        fps,
    )
}

fn finish_audio(
    audio: &mut AudioDecoder,
    frame: &mut AudioFrame,
    pcm: &mut Vec<f32>,
) -> Result<(), DecompileError> {
    match audio.send_eof() {
        Ok(()) => {}
        Err(err) if is_retry(&err) => {}
        Err(err) => return Err(err.into()),
    }
    drain_audio(audio, frame, pcm)
}

fn drain_audio(
    audio: &mut AudioDecoder,
    frame: &mut AudioFrame,
    pcm: &mut Vec<f32>,
) -> Result<(), DecompileError> {
    loop {
        match audio.receive_frame(frame) {
            Ok(()) => push_audio(frame, pcm)?,
            Err(err) if is_retry(&err) => break,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn observe_frame(
    frame: &VideoFrame,
    sad: &mut SadState,
    ring: &mut ThumbRing,
    pending: &mut Option<Pending>,
    boundaries: &mut Vec<RawBoundary>,
    scores: &mut Vec<ScoredFrame>,
    colors: &mut Vec<FrameColor>,
    width: &mut u32,
    height: &mut u32,
    last_time_ms: &mut u64,
    time_base: Rational,
    start_time: i64,
    fps: Rational,
) -> Result<(), DecompileError> {
    let view = yuv_view(frame)?;
    *width = view.width;
    *height = view.height;
    let index = scores.len() as u64;
    let time_ms = frame_time_ms(frame, index, time_base, start_time, fps);
    *last_time_ms = time_ms;
    let scene_score = sad.push(&view)?;
    let mut color = FrameColor::new(index, time_ms);
    accumulate_histogram(&view, &mut color);
    note_boundary(
        index,
        time_ms,
        scene_score,
        &view,
        ring,
        pending,
        boundaries,
    );
    scores.push(ScoredFrame {
        time_ms,
        scene_score,
    });
    colors.push(color);
    Ok(())
}

fn is_again(err: &ffmpeg::Error) -> bool {
    match err {
        ffmpeg::Error::Other { errno } => {
            *errno == ffmpeg::error::EAGAIN || *errno == ffmpeg::error::EWOULDBLOCK
        }
        _ => false,
    }
}

fn is_retry(err: &ffmpeg::Error) -> bool {
    if *err == ffmpeg::Error::Eof {
        return true;
    }
    is_again(err)
}

fn positive_rational(value: Rational) -> Option<Rational> {
    if value.numerator() > 0 && value.denominator() > 0 {
        Some(value)
    } else {
        None
    }
}

fn rational_to_f64(value: Rational) -> f64 {
    f64::from(value.numerator()) / f64::from(value.denominator())
}

fn frame_duration_ms(fps: Rational) -> u64 {
    if fps.numerator() <= 0 {
        return 33;
    }
    (1_000u128 * u128::try_from(fps.denominator()).unwrap_or(1)
        / u128::try_from(fps.numerator()).unwrap_or(1)) as u64
}

fn expected_frame_count(duration_ms: u64, fps: Rational) -> usize {
    if duration_ms == 0 || fps.denominator() <= 0 {
        return 256;
    }
    let frames = u128::from(duration_ms) * u128::try_from(fps.numerator()).unwrap_or(30)
        / (1_000 * u128::try_from(fps.denominator()).unwrap_or(1));
    (frames as usize).saturating_add(8).max(16)
}

fn expected_pcm(duration_ms: u64) -> usize {
    if duration_ms == 0 {
        return 48_000;
    }
    (u128::from(duration_ms) * u128::from(SAMPLE_RATE) / 1_000) as usize + 4_096
}

fn frame_time_ms(
    frame: &VideoFrame,
    index: u64,
    time_base: Rational,
    start_time: i64,
    fps: Rational,
) -> u64 {
    if let Some(pts) = frame.pts() {
        let ticks = if start_time > 0 {
            pts.saturating_sub(start_time)
        } else {
            pts
        };
        if ticks >= 0 && time_base.denominator() != 0 {
            return ticks_to_ms(ticks, time_base);
        }
    }
    index_to_ms(index, fps)
}

fn ticks_to_ms(ticks: i64, time_base: Rational) -> u64 {
    if ticks <= 0 || time_base.denominator() == 0 {
        return 0;
    }
    let ms = i128::from(ticks) * i128::from(time_base.numerator()) * 1_000
        / i128::from(time_base.denominator());
    if ms <= 0 {
        0
    } else {
        ms as u64
    }
}

fn index_to_ms(index: u64, fps: Rational) -> u64 {
    if fps.numerator() <= 0 || fps.denominator() <= 0 {
        return 0;
    }
    (u128::from(index) * 1_000 * u128::try_from(fps.denominator()).unwrap_or(1)
        / u128::try_from(fps.numerator()).unwrap_or(1)) as u64
}

struct SadState {
    step_x: u32,
    step_y: u32,
    cols: u32,
    rows: u32,
    prev: Vec<u8>,
    curr: Vec<u8>,
    prev_mafd: f64,
    primed: bool,
}

impl SadState {
    fn new() -> Self {
        Self {
            step_x: 1,
            step_y: 1,
            cols: 1,
            rows: 1,
            prev: Vec::new(),
            curr: Vec::new(),
            prev_mafd: 0.0,
            primed: false,
        }
    }

    fn push(&mut self, view: &YuvView<'_>) -> Result<f64, DecompileError> {
        if !self.primed {
            self.configure(view.width, view.height);
            sample_luma(
                view.y,
                view.y_stride,
                self.step_x,
                self.step_y,
                self.cols,
                self.rows,
                &mut self.prev,
            )?;
            self.primed = true;
            return Ok(0.0);
        }
        sample_luma(
            view.y,
            view.y_stride,
            self.step_x,
            self.step_y,
            self.cols,
            self.rows,
            &mut self.curr,
        )?;
        let mut sad = 0u64;
        for (left, right) in self.prev.iter().zip(self.curr.iter()) {
            sad += u64::from(left.abs_diff(*right));
        }
        let pixels = (self.cols * self.rows).max(1) as f64;
        let mafd = sad as f64 / pixels;
        let score = ffmpeg_scene_score(mafd, self.prev_mafd);
        self.prev_mafd = mafd;
        std::mem::swap(&mut self.prev, &mut self.curr);
        Ok(score)
    }

    fn configure(&mut self, width: u32, height: u32) {
        self.step_x = (width / SAD_TARGET_W).max(1);
        self.step_y = (height / SAD_TARGET_H).max(1);
        self.cols = (width / self.step_x).max(1);
        self.rows = (height / self.step_y).max(1);
        let n = (self.cols * self.rows) as usize;
        self.prev.resize(n, 0);
        self.curr.resize(n, 0);
    }
}

fn sample_luma(
    plane: &[u8],
    stride: usize,
    step_x: u32,
    step_y: u32,
    cols: u32,
    rows: u32,
    out: &mut [u8],
) -> Result<(), DecompileError> {
    let needed = (cols * rows) as usize;
    if out.len() < needed {
        return Err(DecompileError::Message(
            "luma sample buffer is short".into(),
        ));
    }
    for row in 0..rows {
        let y = (row * step_y) as usize;
        let off = y * stride;
        if off >= plane.len() {
            return Err(DecompileError::Message(
                "luma sample walked past the plane".into(),
            ));
        }
        for col in 0..cols {
            let x = (col * step_x) as usize;
            let index = off + x;
            if index >= plane.len() {
                return Err(DecompileError::Message(
                    "luma sample x walked past the row".into(),
                ));
            }
            out[(row * cols + col) as usize] = plane[index];
        }
    }
    Ok(())
}

struct YuvView<'a> {
    y: &'a [u8],
    y_stride: usize,
    width: u32,
    height: u32,
    chroma: ChromaPlanes<'a>,
}

enum ChromaPlanes<'a> {
    Planar {
        u: &'a [u8],
        v: &'a [u8],
        u_stride: usize,
        v_stride: usize,
        full: bool,
    },
    Nv12 {
        uv: &'a [u8],
        stride: usize,
        full: bool,
    },
}

fn yuv_view(frame: &VideoFrame) -> Result<YuvView<'_>, DecompileError> {
    if frame.planes() == 0 || frame.width() == 0 || frame.height() == 0 {
        return Err(DecompileError::Message(
            "decoded video frame is empty".into(),
        ));
    }
    let width = frame.width();
    let height = frame.height();
    let y_stride = frame.stride(0);
    if y_stride < width as usize {
        return Err(DecompileError::Message(format!(
            "luma stride {y_stride} is narrower than width {width}"
        )));
    }
    let y = frame.data(0);
    let y_needed = y_stride.saturating_mul(height as usize);
    if y.len() < y_needed {
        return Err(DecompileError::Message(
            "luma plane is shorter than stride * height".into(),
        ));
    }
    let pixel = frame.format();
    let chroma = chroma_planes(frame, pixel, width, height)?;
    Ok(YuvView {
        y: &y[..y_needed],
        y_stride,
        width,
        height,
        chroma,
    })
}

fn chroma_planes<'a>(
    frame: &'a VideoFrame,
    pixel: Pixel,
    width: u32,
    height: u32,
) -> Result<ChromaPlanes<'a>, DecompileError> {
    let chroma_w = width.div_ceil(2) as usize;
    let chroma_h = height.div_ceil(2) as usize;
    if pixel == Pixel::YUV420P || pixel == Pixel::YUVJ420P {
        if frame.planes() < 3 {
            return Err(DecompileError::Message(
                "yuv420 frame is missing chroma planes".into(),
            ));
        }
        let u_stride = frame.stride(1);
        let v_stride = frame.stride(2);
        if u_stride < chroma_w || v_stride < chroma_w {
            return Err(DecompileError::Message(
                "chroma stride is narrower than the frame".into(),
            ));
        }
        let u = frame.data(1);
        let v = frame.data(2);
        if u.len() < u_stride * chroma_h || v.len() < v_stride * chroma_h {
            return Err(DecompileError::Message("chroma plane is short".into()));
        }
        return Ok(ChromaPlanes::Planar {
            u,
            v,
            u_stride,
            v_stride,
            full: pixel == Pixel::YUVJ420P,
        });
    }
    if pixel == Pixel::NV12 {
        if frame.planes() < 2 {
            return Err(DecompileError::Message(
                "nv12 frame is missing the chroma plane".into(),
            ));
        }
        let stride = frame.stride(1);
        if stride < chroma_w * 2 {
            return Err(DecompileError::Message(
                "nv12 chroma stride is short".into(),
            ));
        }
        let uv = frame.data(1);
        if uv.len() < stride * chroma_h {
            return Err(DecompileError::Message("nv12 chroma plane is short".into()));
        }
        return Ok(ChromaPlanes::Nv12 {
            uv,
            stride,
            full: false,
        });
    }
    Err(DecompileError::Message(
        "unsupported pixel format; the fast path reads yuv420p or nv12".into(),
    ))
}

fn pixel_rgb(view: &YuvView<'_>, x: u32, y: u32) -> [u8; 3] {
    let luma = view.y[y as usize * view.y_stride + x as usize];
    let (u, v, full) = match &view.chroma {
        ChromaPlanes::Planar {
            u,
            v,
            u_stride,
            v_stride,
            full,
        } => {
            let cx = (x / 2) as usize;
            let cy = (y / 2) as usize;
            (u[cy * *u_stride + cx], v[cy * *v_stride + cx], *full)
        }
        ChromaPlanes::Nv12 { uv, stride, full } => {
            let cx = (x / 2) as usize;
            let cy = (y / 2) as usize;
            let base = cy * *stride + cx * 2;
            (uv[base], uv[base + 1], *full)
        }
        other => {
            let _never: std::convert::Infallible = match other {
                ChromaPlanes::Planar { .. } | ChromaPlanes::Nv12 { .. } => {
                    unreachable!("chroma plane kind is handled above")
                }
            };
            match _never {}
        }
    };
    yuv601_to_rgb(luma, u, v, full)
}

fn accumulate_histogram(view: &YuvView<'_>, color: &mut FrameColor) {
    let step_x = (view.width / HIST_TARGET_W).max(1);
    let step_y = (view.height / HIST_TARGET_H).max(1);
    let cols = (view.width / step_x).max(1);
    let rows = (view.height / step_y).max(1);
    for row in 0..rows {
        let y = row * step_y;
        if y >= view.height {
            break;
        }
        for col in 0..cols {
            let x = col * step_x;
            if x >= view.width {
                break;
            }
            color.push_rgb(pixel_rgb(view, x, y));
        }
    }
}

fn fill_thumb(view: &YuvView<'_>, dest: &mut [u8]) {
    let step_x = (view.width / THUMB_W).max(1);
    let step_y = (view.height / THUMB_H).max(1);
    let cols = (view.width / step_x).min(THUMB_W);
    let rows = (view.height / step_y).min(THUMB_H);
    if cols != THUMB_W || rows != THUMB_H {
        dest.fill(0);
    }
    for row in 0..rows {
        let y = row * step_y;
        if y >= view.height {
            break;
        }
        for col in 0..cols {
            let x = col * step_x;
            if x >= view.width {
                break;
            }
            let rgb = pixel_rgb(view, x, y);
            let index = ((row * THUMB_W + col) * 3) as usize;
            dest[index] = rgb[0];
            dest[index + 1] = rgb[1];
            dest[index + 2] = rgb[2];
        }
    }
}

struct ThumbRing {
    slots: Vec<Vec<u8>>,
    next: usize,
    filled: usize,
}

impl ThumbRing {
    fn new(capacity: usize, bytes: usize) -> Self {
        Self {
            slots: vec![vec![0u8; bytes]; capacity],
            next: 0,
            filled: 0,
        }
    }

    fn push_with(&mut self, fill: impl FnOnce(&mut [u8])) {
        fill(&mut self.slots[self.next]);
        let cap = self.slots.len();
        self.next = (self.next + 1) % cap;
        self.filled = (self.filled + 1).min(cap);
    }

    fn latest(&self) -> &[u8] {
        let cap = self.slots.len();
        let index = (self.next + cap - 1) % cap;
        &self.slots[index]
    }

    fn snapshot(&self) -> Vec<Vec<u8>> {
        let cap = self.slots.len();
        let count = self.filled;
        if count == 0 {
            return Vec::new();
        }
        let start = if self.filled < cap { 0 } else { self.next };
        (0..count)
            .map(|offset| self.slots[(start + offset) % cap].clone())
            .collect()
    }
}

struct Pending {
    frame_index: u64,
    time_ms: u64,
    before: Vec<Vec<u8>>,
    after: Vec<Vec<u8>>,
}

impl Pending {
    fn into_raw(self) -> RawBoundary {
        RawBoundary {
            frame_index: self.frame_index,
            time_ms: self.time_ms,
            before: self.before,
            after: self.after,
        }
    }
}

fn note_boundary(
    index: u64,
    time_ms: u64,
    scene_score: f64,
    view: &YuvView<'_>,
    ring: &mut ThumbRing,
    pending: &mut Option<Pending>,
    boundaries: &mut Vec<RawBoundary>,
) {
    let is_cut = index > 0 && scene_score > SCENE_SCORE_THRESHOLD;
    if is_cut {
        if let Some(previous) = pending.take() {
            boundaries.push(previous.into_raw());
        }
        let before = ring.snapshot();
        ring.push_with(|slot| fill_thumb(view, slot));
        let mut after = Vec::with_capacity(AFTER_FRAMES);
        after.push(ring.latest().to_vec());
        *pending = Some(Pending {
            frame_index: index,
            time_ms,
            before,
            after,
        });
        return;
    }
    ring.push_with(|slot| fill_thumb(view, slot));
    if let Some(open) = pending.as_mut() {
        if open.after.len() < AFTER_FRAMES {
            open.after.push(ring.latest().to_vec());
        }
    }
}

fn push_audio(frame: &AudioFrame, pcm: &mut Vec<f32>) -> Result<(), DecompileError> {
    let channels = usize::from(frame.channels());
    let samples = frame.samples();
    let rate = frame.rate();
    if channels == 0 || samples == 0 || rate == 0 {
        return Ok(());
    }
    let format = frame.format();
    let mut mono = Vec::with_capacity(samples);
    if format == Sample::F32(SampleType::Planar) {
        let planes: Vec<&[f32]> = (0..channels)
            .map(|index| frame.plane::<f32>(index))
            .collect();
        for index in 0..samples {
            let mut acc = 0.0f32;
            for plane in &planes {
                acc += plane[index];
            }
            mono.push(acc / channels as f32);
        }
    } else if format == Sample::F32(SampleType::Packed) {
        let data = frame.plane::<f32>(0);
        for index in 0..samples {
            let mut acc = 0.0f32;
            for channel in 0..channels {
                acc += data[index * channels + channel];
            }
            mono.push(acc / channels as f32);
        }
    } else if format == Sample::I16(SampleType::Packed) {
        let data = frame.plane::<i16>(0);
        for index in 0..samples {
            let mut acc = 0.0f32;
            for channel in 0..channels {
                acc += f32::from(data[index * channels + channel]);
            }
            mono.push((acc / channels as f32) / 32_768.0);
        }
    } else if format == Sample::I16(SampleType::Planar) {
        let planes: Vec<&[i16]> = (0..channels)
            .map(|index| frame.plane::<i16>(index))
            .collect();
        for index in 0..samples {
            let mut acc = 0.0f32;
            for plane in &planes {
                acc += f32::from(plane[index]);
            }
            mono.push((acc / channels as f32) / 32_768.0);
        }
    } else {
        return Err(DecompileError::Message(format!(
            "unsupported audio format {format:?} at {rate} Hz"
        )));
    }
    if rate == SAMPLE_RATE {
        pcm.extend(mono);
    } else {
        resample_linear(&mono, rate, pcm);
    }
    Ok(())
}

fn resample_linear(input: &[f32], in_rate: u32, output: &mut Vec<f32>) {
    if input.is_empty() || in_rate == 0 {
        return;
    }
    let out_len = (input.len() as u64 * u64::from(SAMPLE_RATE) / u64::from(in_rate)) as usize;
    if out_len == 0 {
        return;
    }
    output.reserve(out_len);
    let last = input.len() - 1;
    for index in 0..out_len {
        let src = index as f64 * f64::from(in_rate) / f64::from(SAMPLE_RATE);
        let base = src.floor() as usize;
        let frac = (src - base as f64) as f32;
        let left = input[base.min(last)];
        let right = input[(base + 1).min(last)];
        output.push(left + (right - left) * frac);
    }
}
