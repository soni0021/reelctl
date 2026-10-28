//! Demux and decode with ffmpeg-next.
//!
//! Two decoders produce the frames, and they give the same picture:
//!
//! - Software: libavcodec with frame threading. The in-loop deblock runs on
//!   every frame. Skipping it on non-reference frames saved 6% of decode time
//!   but moved HEVC scene scores by up to 0.022 against the hardware decoder,
//!   which always filters (status.md), so it is not skipped.
//! - Hardware, for HEVC only on macOS (`hwdecode.rs`): VideoToolbox took 4.0 s
//!   against 8.9 s on the 87 s iPhone recording. For 1080p H.264 it was slower
//!   (11.2 s against 3.0 s), so H.264 stays in software.
//!
//! Scene score, histograms, and filmstrip thumbs are sampled from the decoded
//! luma/chroma planes. There is no ffmpeg process spawn on this path.
//!
//! The planes are 8-bit 4:2:0 (`yuv420p`, `yuvj420p`, `nv12`) or 10-bit 4:2:0
//! (`yuv420p10le`, what FFmpeg's software decoders give for iPhone HEVC Main
//! 10, or `p010` from the hardware decoder). Both are read one sampled point at
//! a time through [`Plane`], and a 10-bit sample is read as its top 8 bits, so
//! limited-range 64-940 becomes the 16-235 that 8-bit uses. No frame is
//! converted as a whole, and HDR is not tone-mapped (ADR-011).

use std::path::Path;
use std::sync::OnceLock;

mod probe;
mod sample;

pub use probe::probe_duration_ms;
pub use sample::{decode_frames_at, SampledFrame};

use ffmpeg_next::decoder::Audio as AudioDecoder;
use ffmpeg_next::decoder::Video as VideoDecoder;
use ffmpeg_next::frame::Audio as AudioFrame;
use ffmpeg_next::frame::Video as VideoFrame;
use ffmpeg_next::media::Type as MediaType;
use ffmpeg_next::util::color::{Range, TransferCharacteristic};
use ffmpeg_next::util::format::sample::Type as SampleType;
use ffmpeg_next::util::format::Pixel;
use ffmpeg_next::util::format::Sample;
use ffmpeg_next::{self as ffmpeg, Rational};

use crate::error::DecompileError;
use crate::histogram::{yuv601_to_rgb, FrameColor};
use crate::hwdecode::{self, HardwareDevice};
use crate::linear_resample::LinearResampler;
use crate::motion::{DetectedEvent, MotionDetector};
use crate::scene::{ffmpeg_scene_score, ScoredFrame, SCENE_SCORE_THRESHOLD};

pub(crate) const SAMPLE_RATE: u32 = 48_000;
const SAD_TARGET_W: u32 = 160;
const SAD_TARGET_H: u32 = 90;
pub(crate) const THUMB_W: u32 = 64;
pub(crate) const THUMB_H: u32 = 36;
const BEFORE_FRAMES: usize = 4;
const AFTER_FRAMES: usize = 4;
const HIST_TARGET_W: u32 = 32;
const HIST_TARGET_H: u32 = 18;

const DECODE_NOTE: &str = "\
libavcodec frame-threaded software decode (thread_count auto, loop filter on \
every frame, so the picture is the one the format defines). Scene score and color are sampled from decoded planes; the hot path does not \
shell out to ffmpeg.";

/// What the first decoded video frame says about its samples, for `decode_note`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct VideoFormat {
    pub ten_bit: bool,
    pub hdr: Option<Hdr>,
}

/// An HDR transfer function, from the frame's color metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hdr {
    /// Hybrid log-gamma (`arib-std-b67`), what iPhones record.
    Hlg,
    /// Perceptual quantizer (`smpte2084`), used by HDR10 and Dolby Vision.
    Pq,
}

impl VideoFormat {
    fn of(frame: &VideoFrame) -> Self {
        let hdr = match frame.color_transfer_characteristic() {
            TransferCharacteristic::ARIB_STD_B67 => Some(Hdr::Hlg),
            TransferCharacteristic::SMPTE2084 => Some(Hdr::Pq),
            _ => None,
        };
        Self {
            ten_bit: matches!(frame.format(), Pixel::YUV420P10LE | Pixel::P010LE),
            hdr,
        }
    }
}

const HARDWARE_NOTE: &str = "\
HEVC decoded on the Apple media engine (VideoToolbox) through libavcodec, each \
frame copied to system memory as nv12 or p010 and sampled like a software frame. \
On an 87 s 1080x1920 HEVC Main 10 recording this took 4.0 s against 8.9 s in \
software; H.264 was slower in hardware (11.2 s against 3.0 s), so H.264 is \
decoded in software. Scene score and color are sampled from decoded planes; the \
hot path does not shell out to ffmpeg.";

/// Which decoder `decompile` may use (`--decoder`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DecoderSetting {
    /// Hardware for HEVC where it works, software for everything else.
    #[default]
    Auto,
    /// Software for everything.
    Software,
}

/// Which decoder produced the frames, for `decode_note`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecoderUsed {
    /// The software decoder, because the codec has no hardware path here.
    Software,
    /// The software decoder, because `--decoder software` asked for it.
    Forced,
    /// The media engine, shown by every frame being a VideoToolbox frame.
    Hardware,
    /// The software decoder, after hardware was tried and failed.
    Fallback(String),
}

/// What to do for a codec and a setting.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    TryHardware,
    Software(DecoderUsed),
}

/// HEVC goes to the media engine (4.0 s against 8.9 s on the iPhone file); the
/// rest stay in software, where H.264 is 3.7 times faster.
fn choose_decoder(codec: ffmpeg::codec::Id, setting: DecoderSetting) -> Plan {
    match setting {
        DecoderSetting::Software => Plan::Software(DecoderUsed::Forced),
        DecoderSetting::Auto if cfg!(target_os = "macos") && codec == ffmpeg::codec::Id::HEVC => {
            Plan::TryHardware
        }
        DecoderSetting::Auto => Plan::Software(DecoderUsed::Software),
    }
}

/// `decode_note` for one file: how it was decoded, then what was read from a
/// 10-bit or HDR input and what was not done to it.
pub(crate) fn decode_note(format: VideoFormat, decoder: &DecoderUsed) -> String {
    let mut note = match decoder {
        DecoderUsed::Hardware => HARDWARE_NOTE.to_string(),
        _ => DECODE_NOTE.to_string(),
    };
    match decoder {
        DecoderUsed::Forced => {
            note.push_str(" Software decode was requested with --decoder software.");
        }
        DecoderUsed::Fallback(reason) => note.push_str(&format!(
            " Hardware decode (VideoToolbox) was tried and not used: {reason}. \
             Software decode ran instead."
        )),
        DecoderUsed::Software | DecoderUsed::Hardware => {}
    }
    if format.ten_bit {
        note.push_str(
            " The video is 10-bit 4:2:0; scene score, color, and thumbnails read the \
             top 8 bits of each sample.",
        );
    }
    if let Some(hdr) = format.hdr {
        let name = match hdr {
            Hdr::Hlg => "HLG (arib-std-b67)",
            Hdr::Pq => "PQ (smpte2084)",
        };
        note.push_str(&format!(
            " Its transfer is {name}, which is HDR; colors and thumbnails were not \
             tone-mapped, so they are flatter and less saturated than the picture a \
             viewer sees."
        ));
    }
    note
}

pub(crate) struct DecodedReel {
    pub duration_ms: u64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub format: VideoFormat,
    pub decoder: DecoderUsed,
    pub frames: Vec<ScoredFrame>,
    pub colors: Vec<FrameColor>,
    pub pcm: Vec<f32>,
    pub sample_rate: u32,
    pub boundaries: Vec<RawBoundary>,
    /// Punch-ins, zoom-outs and overlay changes measured on the luma grids.
    pub motion_events: Vec<DetectedEvent>,
}

pub(crate) struct RawBoundary {
    pub frame_index: u64,
    pub time_ms: u64,
    pub before: Vec<Vec<u8>>,
    pub after: Vec<Vec<u8>>,
}

/// Decode a file. HEVC tries the media engine first (`choose_decoder`); on any
/// failure, at any frame, the whole file is decoded again in software, since a
/// decode depends on nothing but the file. If software fails too, that error is
/// the one returned.
pub(crate) fn decode_file(
    path: &Path,
    setting: DecoderSetting,
) -> Result<DecodedReel, DecompileError> {
    ensure_ffmpeg().map_err(|err| stage("ffmpeg init", err))?;
    decode_planned(path, choose_decoder(video_codec(path)?, setting))
}

fn decode_planned(path: &Path, plan: Plan) -> Result<DecodedReel, DecompileError> {
    match plan {
        Plan::Software(used) => decode_with(path, Decoder::Software(used)),
        Plan::TryHardware => {
            let attempt = HardwareDevice::create().and_then(|device| {
                decode_with(path, Decoder::Hardware(&device)).map_err(|err| err.to_string())
            });
            match attempt {
                Ok(reel) => Ok(reel),
                Err(reason) => decode_with(path, Decoder::Software(DecoderUsed::Fallback(reason))),
            }
        }
    }
}

/// The codec of the file's video stream.
fn video_codec(path: &Path) -> Result<ffmpeg::codec::Id, DecompileError> {
    let input = ffmpeg::format::input(path).map_err(|err| stage("open input", err))?;
    let stream = input
        .streams()
        .best(MediaType::Video)
        .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?;
    Ok(stream.parameters().id())
}

/// The decoder to run for one pass over the file.
enum Decoder<'a> {
    Software(DecoderUsed),
    Hardware(&'a HardwareDevice),
}

fn decode_with(path: &Path, decoder: Decoder<'_>) -> Result<DecodedReel, DecompileError> {
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

    let device = match &decoder {
        Decoder::Hardware(device) => Some(*device),
        Decoder::Software(_) => None,
    };
    let mut receiver = {
        let stream = input
            .streams()
            .find(|stream| stream.index() == video_index)
            .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?;
        let video = open_video(stream.parameters(), device)
            .map_err(|err| stage("open video decoder", err))?;
        Receiver::new(video, device.is_some())
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
    let mut pcm = Pcm::with_capacity(expected_pcm(container_ms));
    let mut sad = SadState::new();
    let thumb_bytes = (THUMB_W * THUMB_H * 3) as usize;
    let mut ring = ThumbRing::new(BEFORE_FRAMES, thumb_bytes);
    let mut pending: Option<Pending> = None;
    let mut boundaries = Vec::new();
    let mut audio_frame = AudioFrame::empty();
    let mut picture = Picture {
        width: receiver.video.width(),
        height: receiver.video.height(),
        format: VideoFormat::default(),
    };
    let mut last_time_ms = 0u64;

    for (stream, packet) in input.packets() {
        if stream.index() == video_index {
            send_video(&mut receiver.video, &packet)
                .map_err(|err| stage("send video packet", err))?;
            drain_video(
                &mut receiver,
                &mut sad,
                &mut ring,
                &mut pending,
                &mut boundaries,
                &mut scores,
                &mut colors,
                &mut picture,
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
        &mut receiver,
        &mut sad,
        &mut ring,
        &mut pending,
        &mut boundaries,
        &mut scores,
        &mut colors,
        &mut picture,
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
    let pcm = pcm.finish();
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
        width: picture.width,
        height: picture.height,
        format: picture.format,
        decoder: match decoder {
            Decoder::Hardware(_) => DecoderUsed::Hardware,
            Decoder::Software(used) => used,
        },
        frames: scores,
        colors,
        sample_rate: if pcm.is_empty() { 0 } else { SAMPLE_RATE },
        pcm,
        boundaries,
        motion_events: sad.finish_motion(),
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

fn open_video(
    params: ffmpeg::codec::Parameters,
    device: Option<&HardwareDevice>,
) -> Result<VideoDecoder, DecompileError> {
    let threads = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4);
    let mut context = ffmpeg::codec::Context::from_parameters(params)?;
    context.set_threading(ffmpeg::threading::Config {
        kind: ffmpeg::threading::Type::Frame,
        count: threads,
    });
    if let Some(device) = device {
        hwdecode::attach(&mut context, device).map_err(DecompileError::Message)?;
    }
    Ok(context.decoder().video()?)
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
    receiver: &mut Receiver,
    sad: &mut SadState,
    ring: &mut ThumbRing,
    pending: &mut Option<Pending>,
    boundaries: &mut Vec<RawBoundary>,
    scores: &mut Vec<ScoredFrame>,
    colors: &mut Vec<FrameColor>,
    picture: &mut Picture,
    last_time_ms: &mut u64,
    time_base: Rational,
    start_time: i64,
    fps: Rational,
) -> Result<(), DecompileError> {
    loop {
        match receiver.receive()? {
            Received::Frame => {
                observe_frame(
                    receiver.frame(),
                    sad,
                    ring,
                    pending,
                    boundaries,
                    scores,
                    colors,
                    picture,
                    last_time_ms,
                    time_base,
                    start_time,
                    fps,
                )?;
            }
            Received::Wait => break,
        }
    }
    Ok(())
}

fn finish_video(
    receiver: &mut Receiver,
    sad: &mut SadState,
    ring: &mut ThumbRing,
    pending: &mut Option<Pending>,
    boundaries: &mut Vec<RawBoundary>,
    scores: &mut Vec<ScoredFrame>,
    colors: &mut Vec<FrameColor>,
    picture: &mut Picture,
    last_time_ms: &mut u64,
    time_base: Rational,
    start_time: i64,
    fps: Rational,
) -> Result<(), DecompileError> {
    match receiver.video.send_eof() {
        Ok(()) => {}
        Err(err) if is_retry(&err) => {}
        Err(err) => return Err(err.into()),
    }
    drain_video(
        receiver,
        sad,
        ring,
        pending,
        boundaries,
        scores,
        colors,
        picture,
        last_time_ms,
        time_base,
        start_time,
        fps,
    )
}

fn finish_audio(
    audio: &mut AudioDecoder,
    frame: &mut AudioFrame,
    pcm: &mut Pcm,
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
    pcm: &mut Pcm,
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

/// The video decoder with the frames it fills. A hardware frame is copied into
/// `staging` (a software frame) before it is sampled.
struct Receiver {
    video: VideoDecoder,
    decoded: VideoFrame,
    staging: VideoFrame,
    hardware: bool,
}

enum Received {
    /// `frame()` holds the next decoded frame.
    Frame,
    /// The decoder wants more packets, or is finished.
    Wait,
}

impl Receiver {
    fn new(video: VideoDecoder, hardware: bool) -> Self {
        Self {
            video,
            decoded: VideoFrame::empty(),
            staging: VideoFrame::empty(),
            hardware,
        }
    }

    fn receive(&mut self) -> Result<Received, DecompileError> {
        match self.video.receive_frame(&mut self.decoded) {
            Ok(()) => {}
            Err(err) if is_retry(&err) => return Ok(Received::Wait),
            Err(err) => return Err(err.into()),
        }
        if self.hardware {
            hwdecode::download(&self.decoded, &mut self.staging)
                .map_err(DecompileError::Message)?;
        }
        Ok(Received::Frame)
    }

    /// The frame to sample: in system memory whichever decoder made it.
    fn frame(&self) -> &VideoFrame {
        if self.hardware {
            &self.staging
        } else {
            &self.decoded
        }
    }
}

/// The decoded picture: its size from the latest frame, its format from the first.
struct Picture {
    width: u32,
    height: u32,
    format: VideoFormat,
}

fn observe_frame(
    frame: &VideoFrame,
    sad: &mut SadState,
    ring: &mut ThumbRing,
    pending: &mut Option<Pending>,
    boundaries: &mut Vec<RawBoundary>,
    scores: &mut Vec<ScoredFrame>,
    colors: &mut Vec<FrameColor>,
    picture: &mut Picture,
    last_time_ms: &mut u64,
    time_base: Rational,
    start_time: i64,
    fps: Rational,
) -> Result<(), DecompileError> {
    let view = yuv_view(frame)?;
    picture.width = view.width;
    picture.height = view.height;
    let index = scores.len() as u64;
    if index == 0 {
        picture.format = VideoFormat::of(frame);
    }
    let time_ms = frame_time_ms(frame, index, time_base, start_time, fps);
    *last_time_ms = time_ms;
    let scene_score = sad.observe(index, time_ms, &view)?;
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
    /// The frame size the grid was set from (the first frame's).
    width: u32,
    height: u32,
    step_x: u32,
    step_y: u32,
    cols: u32,
    rows: u32,
    prev: Vec<u8>,
    curr: Vec<u8>,
    prev_mafd: f64,
    primed: bool,
    /// Reads the same grids for punch-ins and overlays (`motion.rs`).
    motion: Option<MotionDetector>,
}

impl SadState {
    fn new() -> Self {
        Self {
            width: 1,
            height: 1,
            step_x: 1,
            step_y: 1,
            cols: 1,
            rows: 1,
            prev: Vec::new(),
            curr: Vec::new(),
            prev_mafd: 0.0,
            primed: false,
            motion: None,
        }
    }

    /// The scene score of a frame, and the frame's grid to the motion detector. A
    /// frame the scene score calls a cut is told to it as one, so no event lies on a cut.
    fn observe(
        &mut self,
        index: u64,
        time_ms: u64,
        view: &YuvView<'_>,
    ) -> Result<f64, DecompileError> {
        let score = self.push(view)?;
        let is_cut = index > 0 && score > SCENE_SCORE_THRESHOLD;
        if self.motion.is_none() {
            self.motion = Some(MotionDetector::new(self.cols as usize, self.rows as usize));
        }
        if let Some(motion) = self.motion.as_mut() {
            // After `push` the current grid is in `prev` (the buffers were swapped).
            motion.push(index, time_ms, &self.prev, is_cut);
        }
        Ok(score)
    }

    /// The events found over the whole clip, sorted by time.
    fn finish_motion(&mut self) -> Vec<DetectedEvent> {
        self.motion
            .take()
            .map(MotionDetector::finish)
            .unwrap_or_default()
    }

    fn push(&mut self, view: &YuvView<'_>) -> Result<f64, DecompileError> {
        if !self.primed {
            self.configure(view.width, view.height);
            sample_luma(view, self.grid(), &mut self.prev)?;
            self.primed = true;
            return Ok(0.0);
        }
        sample_luma(view, self.grid(), &mut self.curr)?;
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

    fn grid(&self) -> Grid {
        Grid {
            width: self.width,
            height: self.height,
            step_x: self.step_x,
            step_y: self.step_y,
            cols: self.cols,
            rows: self.rows,
        }
    }

    fn configure(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.step_x = (width / SAD_TARGET_W).max(1);
        self.step_y = (height / SAD_TARGET_H).max(1);
        self.cols = (width / self.step_x).max(1);
        self.rows = (height / self.step_y).max(1);
        let n = (self.cols * self.rows) as usize;
        self.prev.resize(n, 0);
        self.curr.resize(n, 0);
    }
}

/// The points the scene score reads: `cols` x `rows` of them, `step_x` and
/// `step_y` apart in a frame of `width` x `height` (the first frame's size).
#[derive(Clone, Copy)]
struct Grid {
    width: u32,
    height: u32,
    step_x: u32,
    step_y: u32,
    cols: u32,
    rows: u32,
}

impl Grid {
    /// Where grid column or row `index` falls in a frame whose side is `side`
    /// long, when the grid was set from a side of `base`. In a frame of the
    /// grid's own size that is `index * step`; in any other size it is the same
    /// place in the picture, so a stream that changes size is compared by
    /// position and the grid always spans the whole frame.
    fn place(index: u32, step: u32, base: u32, side: u32) -> usize {
        let at = u64::from(index) * u64::from(step);
        if side == base {
            at as usize
        } else {
            (at * u64::from(side) / u64::from(base.max(1))) as usize
        }
    }
}

fn sample_luma(view: &YuvView<'_>, grid: Grid, out: &mut [u8]) -> Result<(), DecompileError> {
    let needed = (grid.cols * grid.rows) as usize;
    if out.len() < needed {
        return Err(DecompileError::Message(
            "luma sample buffer is short".into(),
        ));
    }
    let last_row = Grid::place(
        grid.rows.saturating_sub(1),
        grid.step_y,
        grid.height,
        view.height,
    );
    let last_col = Grid::place(
        grid.cols.saturating_sub(1),
        grid.step_x,
        grid.width,
        view.width,
    );
    if !view.y.covers(last_row, last_col) {
        return Err(DecompileError::Message(
            "luma sample walked past the plane".into(),
        ));
    }
    for row in 0..grid.rows {
        let y = Grid::place(row, grid.step_y, grid.height, view.height);
        for col in 0..grid.cols {
            let x = Grid::place(col, grid.step_x, grid.width, view.width);
            out[(row * grid.cols + col) as usize] = view.y.at(y, x);
        }
    }
    Ok(())
}

/// One plane of a decoded frame, read one sample at a time as 8 bits.
#[derive(Clone, Copy)]
enum Plane<'a> {
    Eight {
        data: &'a [u8],
        stride: usize,
    },
    /// Little-endian 16-bit samples holding 10-bit values (`yuv420p10le`).
    Ten {
        data: &'a [u8],
        stride: usize,
    },
    /// Little-endian 16-bit samples holding a 10-bit value in the top bits
    /// (`p010`, value << 6), so the top 8 bits of the value are the high byte.
    High16 {
        data: &'a [u8],
        stride: usize,
    },
}

impl Plane<'_> {
    /// The sample at `row`, `col`: the byte itself, or the top 8 of 10 bits.
    /// `col` counts samples, not bytes.
    /// Strides are in bytes, as FFmpeg reports them. Panics outside the data;
    /// `plane` checks a frame's planes against its size, and `covers` checks
    /// one point.
    fn at(&self, row: usize, col: usize) -> u8 {
        match *self {
            Plane::Eight { data, stride } => data[row * stride + col],
            Plane::Ten { data, stride } => {
                let index = row * stride + col * 2;
                let sample = u16::from_le_bytes([data[index], data[index + 1]]);
                (sample >> 2).min(255) as u8
            }
            Plane::High16 { data, stride } => data[row * stride + col * 2 + 1],
        }
    }

    /// Whether the sample at `row`, `col` lies inside the plane's data.
    fn covers(&self, row: usize, col: usize) -> bool {
        match *self {
            Plane::Eight { data, stride } => row * stride + col < data.len(),
            Plane::Ten { data, stride } | Plane::High16 { data, stride } => {
                row * stride + col * 2 + 1 < data.len()
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Depth {
    Eight,
    Ten,
    /// `p010`: 16-bit samples, 10-bit value in the top bits.
    P010,
}

struct YuvView<'a> {
    y: Plane<'a>,
    width: u32,
    height: u32,
    chroma: ChromaPlanes<'a>,
}

#[derive(Clone, Copy)]
enum ChromaPlanes<'a> {
    Planar {
        u: Plane<'a>,
        v: Plane<'a>,
        full: bool,
    },
    /// U and V bytes interleaved in one plane.
    Nv12 { uv: Plane<'a>, full: bool },
}

fn yuv_view(frame: &VideoFrame) -> Result<YuvView<'_>, DecompileError> {
    if frame.planes() == 0 || frame.width() == 0 || frame.height() == 0 {
        return Err(DecompileError::Message(
            "decoded video frame is empty".into(),
        ));
    }
    let width = frame.width();
    let height = frame.height();
    let pixel = frame.format();
    let depth = match pixel {
        Pixel::YUV420P | Pixel::YUVJ420P | Pixel::NV12 => Depth::Eight,
        Pixel::YUV420P10LE => Depth::Ten,
        Pixel::P010LE => Depth::P010,
        other => return Err(unsupported(other)),
    };
    let y = plane(frame, 0, depth, width as usize, height as usize, "luma")?;
    let chroma = chroma_planes(frame, pixel, depth, width, height)?;
    Ok(YuvView {
        y,
        width,
        height,
        chroma,
    })
}

fn unsupported(pixel: Pixel) -> DecompileError {
    let name = pixel
        .descriptor()
        .map_or("unknown", |descriptor| descriptor.name());
    DecompileError::Message(format!(
        "unsupported pixel format {name}; decompile reads 4:2:0 video as yuv420p, \
         yuvj420p, nv12, yuv420p10le, or p010le"
    ))
}

/// Plane `index` of `frame`, checked to hold `rows` rows of `samples` samples.
fn plane<'a>(
    frame: &'a VideoFrame,
    index: usize,
    depth: Depth,
    samples: usize,
    rows: usize,
    name: &str,
) -> Result<Plane<'a>, DecompileError> {
    let stride = frame.stride(index);
    let bytes = match depth {
        Depth::Eight => samples,
        Depth::Ten | Depth::P010 => samples * 2,
    };
    if stride < bytes {
        return Err(DecompileError::Message(format!(
            "{name} stride {stride} is narrower than {samples} samples"
        )));
    }
    let data = frame.data(index);
    let needed = stride.saturating_mul(rows);
    if data.len() < needed {
        return Err(DecompileError::Message(format!(
            "{name} plane is shorter than stride * height"
        )));
    }
    let data = &data[..needed];
    Ok(match depth {
        Depth::Eight => Plane::Eight { data, stride },
        Depth::Ten => Plane::Ten { data, stride },
        Depth::P010 => Plane::High16 { data, stride },
    })
}

fn chroma_planes<'a>(
    frame: &'a VideoFrame,
    pixel: Pixel,
    depth: Depth,
    width: u32,
    height: u32,
) -> Result<ChromaPlanes<'a>, DecompileError> {
    let chroma_w = width.div_ceil(2) as usize;
    let chroma_h = height.div_ceil(2) as usize;
    if pixel == Pixel::NV12 || pixel == Pixel::P010LE {
        if frame.planes() < 2 {
            return Err(DecompileError::Message(
                "nv12 or p010 frame is missing the chroma plane".into(),
            ));
        }
        return Ok(ChromaPlanes::Nv12 {
            uv: plane(frame, 1, depth, chroma_w * 2, chroma_h, "nv12 chroma")?,
            full: frame.color_range() == Range::JPEG,
        });
    }
    if frame.planes() < 3 {
        return Err(DecompileError::Message(
            "yuv420 frame is missing chroma planes".into(),
        ));
    }
    let full = pixel == Pixel::YUVJ420P || frame.color_range() == Range::JPEG;
    Ok(ChromaPlanes::Planar {
        u: plane(frame, 1, depth, chroma_w, chroma_h, "u chroma")?,
        v: plane(frame, 2, depth, chroma_w, chroma_h, "v chroma")?,
        full,
    })
}

fn pixel_rgb(view: &YuvView<'_>, x: u32, y: u32) -> [u8; 3] {
    let (x, y) = (x as usize, y as usize);
    let luma = view.y.at(y, x);
    let (cx, cy) = (x / 2, y / 2);
    let (u, v, full) = match view.chroma {
        ChromaPlanes::Planar { u, v, full } => (u.at(cy, cx), v.at(cy, cx), full),
        ChromaPlanes::Nv12 { uv, full } => (uv.at(cy, cx * 2), uv.at(cy, cx * 2 + 1), full),
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

fn push_audio(frame: &AudioFrame, pcm: &mut Pcm) -> Result<(), DecompileError> {
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
    pcm.push(&mono, rate);
    Ok(())
}

/// The decoded audio as mono 48 kHz. Audio at another rate goes through one
/// [`LinearResampler`] for the whole stream.
struct Pcm {
    samples: Vec<f32>,
    resampler: Option<LinearResampler>,
}

impl Pcm {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
            resampler: None,
        }
    }

    fn push(&mut self, mono: &[f32], rate: u32) {
        if rate == SAMPLE_RATE && self.resampler.is_none() {
            self.samples.extend_from_slice(mono);
            return;
        }
        // A new rate mid-stream ends the old resampler's stream.
        if self.resampler.as_ref().is_some_and(|r| r.in_rate() != rate) {
            self.finish_resampler();
        }
        if rate == SAMPLE_RATE {
            self.samples.extend_from_slice(mono);
            return;
        }
        self.resampler
            .get_or_insert_with(|| LinearResampler::new(rate))
            .push(mono, &mut self.samples);
    }

    fn finish_resampler(&mut self) {
        if let Some(mut resampler) = self.resampler.take() {
            resampler.finish(&mut self.samples);
        }
    }

    fn finish(mut self) -> Vec<f32> {
        self.finish_resampler();
        self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Little-endian 16-bit rows, each padded with zeros to `stride` bytes.
    fn ten_bit_rows(rows: &[&[u16]], stride: usize) -> Vec<u8> {
        let mut data = Vec::new();
        for row in rows {
            let start = data.len();
            for sample in *row {
                data.extend_from_slice(&sample.to_le_bytes());
            }
            data.resize(start + stride, 0);
        }
        data
    }

    #[test]
    fn an_eight_bit_plane_reads_the_byte() {
        let data = [10, 11, 12, 0, 0, 20, 21, 22, 0, 0];
        let plane = Plane::Eight {
            data: &data,
            stride: 5,
        };
        assert_eq!(plane.at(0, 0), 10);
        assert_eq!(plane.at(0, 2), 12);
        assert_eq!(plane.at(1, 0), 20);
        assert_eq!(plane.at(1, 2), 22);
        assert!(plane.covers(1, 4));
        assert!(!plane.covers(2, 0));
    }

    #[test]
    fn a_ten_bit_plane_reads_the_top_eight_bits() {
        // Limited-range black, mid grey, and white, then the extremes and a
        // value whose low two bits are dropped.
        let data = ten_bit_rows(&[&[64, 512, 940], &[1023, 3, 7]], 8);
        let plane = Plane::Ten {
            data: &data,
            stride: 8,
        };
        assert_eq!(plane.at(0, 0), 16);
        assert_eq!(plane.at(0, 1), 128);
        assert_eq!(plane.at(0, 2), 235);
        assert_eq!(plane.at(1, 0), 255);
        assert_eq!(plane.at(1, 1), 0);
        assert_eq!(plane.at(1, 2), 1);
        assert!(plane.covers(1, 3));
        assert!(!plane.covers(1, 4));
        assert!(!plane.covers(2, 0));
    }

    #[test]
    fn luma_sampling_reads_ten_bit_planes_on_the_same_grid() {
        let eight = [16, 1, 235, 1, 2, 2, 2, 2, 81, 1, 128, 1];
        let ten = ten_bit_rows(&[&[64, 4, 940, 4], &[8, 8, 8, 8], &[324, 4, 512, 4]], 10);
        let mut from_eight = vec![0; 4];
        let mut from_ten = vec![0; 4];
        let eight = Plane::Eight {
            data: &eight,
            stride: 4,
        };
        let ten = Plane::Ten {
            data: &ten,
            stride: 10,
        };
        let grid = Grid {
            width: 4,
            height: 3,
            step_x: 2,
            step_y: 2,
            cols: 2,
            rows: 2,
        };
        sample_luma(&luma_view(eight, 4, 3, &[128]), grid, &mut from_eight).expect("8-bit grid");
        sample_luma(&luma_view(ten, 4, 3, &[128]), grid, &mut from_ten).expect("10-bit grid");
        assert_eq!(from_eight, [16, 235, 81, 128]);
        assert_eq!(from_ten, from_eight);
    }

    #[test]
    fn luma_sampling_stops_at_the_end_of_the_plane() {
        let data = [0u8; 6];
        let plane = Plane::Eight {
            data: &data,
            stride: 3,
        };
        let mut out = vec![0; 9];
        let grid = Grid {
            width: 3,
            height: 3,
            step_x: 1,
            step_y: 1,
            cols: 3,
            rows: 3,
        };
        assert!(sample_luma(&luma_view(plane, 3, 3, &[128]), grid, &mut out).is_err());
    }

    #[test]
    fn ten_bit_pixels_convert_like_their_top_eight_bits() {
        // draft12's red: Y 81, U 90, V 240, and the same as 10-bit samples.
        let (y8, u8_, v8) = ([81u8; 4], [90u8], [240u8]);
        let y10 = ten_bit_rows(&[&[324, 324], &[324, 324]], 4);
        let (u10, v10) = (ten_bit_rows(&[&[360]], 2), ten_bit_rows(&[&[960]], 2));
        let view = |y, u, v| YuvView {
            y,
            width: 2,
            height: 2,
            chroma: ChromaPlanes::Planar { u, v, full: false },
        };
        let eight = |data, stride| Plane::Eight { data, stride };
        let ten = |data, stride| Plane::Ten { data, stride };
        let from_eight = view(eight(&y8, 2), eight(&u8_, 1), eight(&v8, 1));
        let from_ten = view(ten(&y10, 4), ten(&u10, 2), ten(&v10, 2));
        assert_eq!(
            pixel_rgb(&from_eight, 1, 1),
            yuv601_to_rgb(81, 90, 240, false)
        );
        assert_eq!(pixel_rgb(&from_ten, 1, 1), pixel_rgb(&from_eight, 1, 1));
    }

    #[test]
    fn an_unsupported_format_is_named_in_the_error() {
        ensure_ffmpeg().expect("ffmpeg");
        let message = unsupported(Pixel::YUV422P).to_string();
        assert!(message.contains("yuv422p"), "{message}");
        assert!(message.contains("yuv420p10le"), "{message}");
    }

    #[test]
    fn eight_bit_sdr_keeps_the_decode_note() {
        assert_eq!(
            decode_note(VideoFormat::default(), &DecoderUsed::Software),
            DECODE_NOTE
        );
    }

    #[test]
    fn the_decode_note_says_ten_bit_and_hdr() {
        let hlg = decode_note(
            VideoFormat {
                ten_bit: true,
                hdr: Some(Hdr::Hlg),
            },
            &DecoderUsed::Software,
        );
        assert!(hlg.starts_with(DECODE_NOTE), "{hlg}");
        assert!(hlg.contains("10-bit"), "{hlg}");
        assert!(hlg.contains("HLG"), "{hlg}");
        assert!(hlg.contains("not tone-mapped"), "{hlg}");
        let pq = decode_note(
            VideoFormat {
                ten_bit: true,
                hdr: Some(Hdr::Pq),
            },
            &DecoderUsed::Software,
        );
        assert!(pq.contains("PQ"), "{pq}");
        let ten_sdr = decode_note(
            VideoFormat {
                ten_bit: true,
                hdr: None,
            },
            &DecoderUsed::Software,
        );
        assert!(
            ten_sdr.contains("10-bit") && !ten_sdr.contains("tone"),
            "{ten_sdr}"
        );
    }

    #[test]
    fn audio_at_48_khz_passes_through_unchanged() {
        let input: Vec<f32> = (0..1_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut pcm = Pcm::with_capacity(0);
        pcm.push(&input[..400], SAMPLE_RATE);
        pcm.push(&input[400..], SAMPLE_RATE);
        assert_eq!(pcm.finish(), input);
    }

    #[test]
    fn a_new_audio_rate_ends_the_old_stream() {
        let mut pcm = Pcm::with_capacity(0);
        pcm.push(&vec![0.5; 44_100], 44_100);
        pcm.push(&vec![0.5; 24_000], 24_000);
        let samples = pcm.finish();
        // One second of each, at 48 kHz.
        assert!(samples.len().abs_diff(96_000) <= 2, "{}", samples.len());
    }

    /// A 4x4 `yuv420p10le` frame of mid-grey (Y 512, U 512, V 512), with the
    /// given range in its metadata.
    fn grey_ten_bit_frame(range: Range) -> VideoFrame {
        ensure_ffmpeg().expect("ffmpeg");
        let mut frame = VideoFrame::new(Pixel::YUV420P10LE, 4, 4);
        frame.set_color_range(range);
        for index in 0..3 {
            for pair in frame.data_mut(index).chunks_exact_mut(2) {
                pair.copy_from_slice(&512u16.to_le_bytes());
            }
        }
        frame
    }

    #[test]
    fn a_ten_bit_frame_is_full_range_when_its_metadata_says_so() {
        let limited = grey_ten_bit_frame(Range::MPEG);
        let full = grey_ten_bit_frame(Range::JPEG);
        let limited_rgb = pixel_rgb(&yuv_view(&limited).expect("limited view"), 1, 1);
        let full_rgb = pixel_rgb(&yuv_view(&full).expect("full view"), 1, 1);
        // Grey 128 in full range stays 128; in limited range it is stretched.
        assert_eq!(full_rgb, [128, 128, 128]);
        assert_eq!(limited_rgb, yuv601_to_rgb(128, 128, 128, false));
        assert_ne!(limited_rgb, full_rgb);
    }

    #[test]
    fn a_p010_plane_reads_the_high_byte_of_each_sample() {
        // P010 keeps a 10-bit value in the top bits of 16: value << 6.
        let data = ten_bit_rows(
            &[&[64 << 6, 512 << 6, 940 << 6], &[1023 << 6, 3 << 6, 0]],
            8,
        );
        let plane = Plane::High16 {
            data: &data,
            stride: 8,
        };
        assert_eq!(plane.at(0, 0), 16);
        assert_eq!(plane.at(0, 1), 128);
        assert_eq!(plane.at(0, 2), 235);
        assert_eq!(plane.at(1, 0), 255);
        assert_eq!(plane.at(1, 1), 0);
        assert!(plane.covers(1, 3));
        assert!(!plane.covers(1, 4));
        assert!(!plane.covers(2, 0));
    }

    #[test]
    fn p010_and_yuv420p10le_give_the_same_sample_for_the_same_value() {
        for value in 0u16..1024 {
            let le = ten_bit_rows(&[&[value]], 2);
            let p010 = ten_bit_rows(&[&[value << 6]], 2);
            let from_le = Plane::Ten {
                data: &le,
                stride: 2,
            };
            let from_p010 = Plane::High16 {
                data: &p010,
                stride: 2,
            };
            assert_eq!(from_le.at(0, 0), from_p010.at(0, 0), "value {value}");
        }
    }

    #[test]
    fn p010_pixels_convert_like_the_same_eight_bit_values() {
        // draft12's red again, with P010's interleaved U and V.
        let y = ten_bit_rows(&[&[324 << 6, 324 << 6], &[324 << 6, 324 << 6]], 4);
        let uv = ten_bit_rows(&[&[360 << 6, 960 << 6]], 4);
        let view = YuvView {
            y: Plane::High16 {
                data: &y,
                stride: 4,
            },
            width: 2,
            height: 2,
            chroma: ChromaPlanes::Nv12 {
                uv: Plane::High16 {
                    data: &uv,
                    stride: 4,
                },
                full: false,
            },
        };
        assert_eq!(pixel_rgb(&view, 1, 1), yuv601_to_rgb(81, 90, 240, false));
    }

    /// A 4x4 `p010le` frame of mid-grey (Y, U, V all 512 << 6), with the given
    /// range in its metadata.
    fn grey_p010_frame(range: Range) -> VideoFrame {
        ensure_ffmpeg().expect("ffmpeg");
        let mut frame = VideoFrame::new(Pixel::P010LE, 4, 4);
        frame.set_color_range(range);
        for index in 0..2 {
            for pair in frame.data_mut(index).chunks_exact_mut(2) {
                pair.copy_from_slice(&(512u16 << 6).to_le_bytes());
            }
        }
        frame
    }

    #[test]
    fn a_p010_frame_is_read_and_follows_its_range_metadata() {
        let limited = grey_p010_frame(Range::MPEG);
        let full = grey_p010_frame(Range::JPEG);
        let limited_view = yuv_view(&limited).expect("p010 view");
        let full_view = yuv_view(&full).expect("p010 full view");
        assert_eq!(pixel_rgb(&full_view, 1, 1), [128, 128, 128]);
        assert_eq!(
            pixel_rgb(&limited_view, 1, 1),
            yuv601_to_rgb(128, 128, 128, false)
        );
        assert!(VideoFormat::of(&limited).ten_bit);
    }

    /// A luma-only picture of `width` x `height`: dark left half, bright right
    /// half, with the right half dark instead when `right_dark` is set.
    fn halves(width: usize, height: usize, right_dark: bool) -> Vec<u8> {
        let mut data = vec![0u8; width * height];
        for row in data.chunks_exact_mut(width) {
            for (x, sample) in row.iter_mut().enumerate() {
                *sample = if x < width / 2 || right_dark { 50 } else { 200 };
            }
        }
        data
    }

    /// A view with this luma and a flat chroma plane, for reading luma only.
    fn luma_view<'a>(y: Plane<'a>, width: u32, height: u32, chroma: &'a [u8; 1]) -> YuvView<'a> {
        let flat = Plane::Eight {
            data: chroma,
            stride: 1,
        };
        YuvView {
            y,
            width,
            height,
            chroma: ChromaPlanes::Planar {
                u: flat,
                v: flat,
                full: false,
            },
        }
    }

    fn score_of(sad: &mut SadState, data: &[u8], width: usize, height: usize) -> f64 {
        let y = Plane::Eight {
            data,
            stride: width,
        };
        let view = luma_view(y, width as u32, height as u32, &[128]);
        sad.push(&view).expect("score")
    }

    /// A 160x90 flat grey scene for 30 frames, then `strip` columns of it 100
    /// levels brighter for 50 more, fed through `observe` as a clip. Returns
    /// the score of the frame that changes and the events found.
    fn observe_strip(strip: usize) -> (f64, Vec<DetectedEvent>) {
        let (width, height) = (160usize, 90usize);
        let flat = vec![60u8; width * height];
        let mut changed = flat.clone();
        for row in changed.chunks_exact_mut(width) {
            for sample in &mut row[..strip] {
                *sample = 160;
            }
        }
        let mut sad = SadState::new();
        let mut change_score = 0.0;
        for index in 0..80u64 {
            let data = if index < 30 { &flat } else { &changed };
            let y = Plane::Eight {
                data,
                stride: width,
            };
            let view = luma_view(y, width as u32, height as u32, &[128]);
            let score = sad
                .observe(index, index * 1000 / 24, &view)
                .expect("observe");
            if index == 30 {
                change_score = score;
            }
        }
        (change_score, sad.finish_motion())
    }

    /// The detector is told which frames the scene score calls cuts: a change
    /// that the scene score calls a cut is no event, and the same kind of
    /// change just under the threshold is one.
    #[test]
    fn a_frame_the_scene_score_calls_a_cut_reaches_the_motion_detector_as_one() {
        let (score, events) = observe_strip(56);
        assert!(score > SCENE_SCORE_THRESHOLD, "score {score}");
        assert!(events.is_empty(), "an event on a cut frame: {events:?}");

        let (score, events) = observe_strip(32);
        assert!(score < SCENE_SCORE_THRESHOLD, "score {score}");
        // (A strip of a flat scene can also fit a scale; the overlay is the point.)
        let overlays: Vec<&DetectedEvent> = events
            .iter()
            .filter(|e| e.kind == crate::motion::MotionKind::OverlayChange)
            .collect();
        assert_eq!(overlays.len(), 1, "{events:?}");
        assert_eq!(overlays[0].frame_index, 30);
        assert_eq!(overlays[0].time_ms, 30 * 1000 / 24);
    }

    /// The same picture at another size is the same picture, and a change in
    /// the part of a larger frame the first frame's size would not reach counts.
    #[test]
    fn the_scene_score_compares_frames_of_different_sizes_by_position() {
        let mut sad = SadState::new();
        score_of(&mut sad, &halves(320, 180, false), 320, 180);
        let grown = score_of(&mut sad, &halves(640, 360, false), 640, 360);
        assert!(grown < 0.01, "same picture, larger: {grown}");
        let changed = score_of(&mut sad, &halves(640, 360, true), 640, 360);
        assert!(changed > 0.1, "right half changed: {changed}");

        let mut sad = SadState::new();
        score_of(&mut sad, &halves(640, 360, false), 640, 360);
        let shrunk = score_of(&mut sad, &halves(320, 180, false), 320, 180);
        assert!(shrunk < 0.01, "same picture, smaller: {shrunk}");
        let changed = score_of(&mut sad, &halves(320, 180, true), 320, 180);
        assert!(changed > 0.1, "right half changed: {changed}");
    }

    /// A 4x4 `nv12` frame of mid-grey, with the given range in its metadata.
    fn grey_nv12_frame(range: Range) -> VideoFrame {
        ensure_ffmpeg().expect("ffmpeg");
        let mut frame = VideoFrame::new(Pixel::NV12, 4, 4);
        frame.set_color_range(range);
        for index in 0..2 {
            frame.data_mut(index).fill(128);
        }
        frame
    }

    /// The media engine returns every 8-bit HEVC frame as `nv12`, and has no
    /// `yuvj420p` to say "full range", so the metadata is all there is. The
    /// software decoder reads the same stream as `yuvj420p`, which is full.
    #[test]
    fn an_nv12_frame_follows_its_range_metadata() {
        let limited = grey_nv12_frame(Range::MPEG);
        let full = grey_nv12_frame(Range::JPEG);
        let unspecified = grey_nv12_frame(Range::Unspecified);
        let full_view = yuv_view(&full).expect("nv12 full view");
        assert_eq!(pixel_rgb(&full_view, 1, 1), [128, 128, 128]);
        assert_eq!(
            pixel_rgb(&yuv_view(&limited).expect("nv12 view"), 1, 1),
            yuv601_to_rgb(128, 128, 128, false)
        );
        // No range in the metadata is limited, as for every other format.
        assert_eq!(
            pixel_rgb(&yuv_view(&unspecified).expect("nv12 view"), 1, 1),
            yuv601_to_rgb(128, 128, 128, false)
        );
    }

    /// Software gives `yuvj420p` for most full-range 8-bit video, but a frame
    /// can also be `yuv420p` with a full range in its metadata (VP9 and AV1
    /// files do this). Every format follows the same rule.
    #[test]
    fn a_planar_8_bit_frame_is_full_range_when_its_metadata_says_so() {
        ensure_ffmpeg().expect("ffmpeg");
        let grey = |pixel: Pixel, range: Range| {
            let mut frame = VideoFrame::new(pixel, 4, 4);
            frame.set_color_range(range);
            for index in 0..3 {
                frame.data_mut(index).fill(128);
            }
            pixel_rgb(&yuv_view(&frame).expect("view"), 1, 1)
        };
        assert_eq!(grey(Pixel::YUV420P, Range::JPEG), [128, 128, 128]);
        assert_eq!(grey(Pixel::YUVJ420P, Range::JPEG), [128, 128, 128]);
        // The pixel format alone says full range for yuvj420p.
        assert_eq!(grey(Pixel::YUVJ420P, Range::Unspecified), [128, 128, 128]);
        let limited = yuv601_to_rgb(128, 128, 128, false);
        assert_eq!(grey(Pixel::YUV420P, Range::MPEG), limited);
        assert_eq!(grey(Pixel::YUV420P, Range::Unspecified), limited);
    }

    #[test]
    fn the_software_note_does_not_say_the_media_engine_is_unavailable() {
        let note = decode_note(VideoFormat::default(), &DecoderUsed::Software);
        assert!(note.contains("software decode"), "{note}");
        assert!(!note.contains("VideoToolbox"), "{note}");
        assert!(!note.contains("hwdevice"), "{note}");
    }

    #[test]
    fn hevc_tries_hardware_and_everything_else_decodes_in_software() {
        use ffmpeg::codec::Id;
        let on_mac = cfg!(target_os = "macos");
        assert_eq!(
            choose_decoder(Id::HEVC, DecoderSetting::Auto),
            if on_mac {
                Plan::TryHardware
            } else {
                Plan::Software(DecoderUsed::Software)
            }
        );
        for codec in [Id::H264, Id::VP9, Id::AV1, Id::MPEG4] {
            assert_eq!(
                choose_decoder(codec, DecoderSetting::Auto),
                Plan::Software(DecoderUsed::Software),
                "{codec:?}"
            );
        }
    }

    #[test]
    fn software_can_be_forced() {
        use ffmpeg::codec::Id;
        assert_eq!(
            choose_decoder(Id::HEVC, DecoderSetting::Software),
            Plan::Software(DecoderUsed::Forced)
        );
        assert_eq!(
            choose_decoder(Id::H264, DecoderSetting::Software),
            Plan::Software(DecoderUsed::Forced)
        );
    }

    #[test]
    fn the_decode_note_names_the_decoder_that_ran() {
        let format = VideoFormat::default();
        assert_eq!(decode_note(format, &DecoderUsed::Software), DECODE_NOTE);

        let forced = decode_note(format, &DecoderUsed::Forced);
        assert!(forced.starts_with(DECODE_NOTE), "{forced}");
        assert!(forced.contains("--decoder software"), "{forced}");

        let hardware = decode_note(format, &DecoderUsed::Hardware);
        assert!(hardware.contains("VideoToolbox"), "{hardware}");
        assert!(
            !hardware.contains("frame-threaded software decode"),
            "{hardware}"
        );

        let fallback = decode_note(format, &DecoderUsed::Fallback("no device".to_string()));
        assert!(fallback.starts_with(DECODE_NOTE), "{fallback}");
        assert!(
            fallback.contains("was tried and not used: no device"),
            "{fallback}"
        );
    }

    #[test]
    fn the_hardware_note_keeps_the_ten_bit_and_hdr_sentences() {
        let note = decode_note(
            VideoFormat {
                ten_bit: true,
                hdr: Some(Hdr::Hlg),
            },
            &DecoderUsed::Hardware,
        );
        assert!(note.contains("VideoToolbox"), "{note}");
        assert!(note.contains("10-bit"), "{note}");
        assert!(note.contains("HLG"), "{note}");
    }

    /// A one-second, 160x90 clip the media engine cannot decode (motion JPEG
    /// has no VideoToolbox hardware path), made with the ffmpeg CLI.
    fn mjpeg_clip(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("reelctl-{name}-{}.mov", std::process::id()));
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=24:d=1"])
            .args(["-c:v", "mjpeg", "-q:v", "3"])
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg could not make the clip");
        path
    }

    #[test]
    fn a_failed_hardware_attempt_decodes_again_in_software() {
        ensure_ffmpeg().expect("ffmpeg");
        let path = mjpeg_clip("fallback");
        let forced = decode_planned(&path, Plan::Software(DecoderUsed::Forced)).expect("software");
        let fell_back = decode_planned(&path, Plan::TryHardware).expect("fallback");
        let _ = std::fs::remove_file(&path);

        match &fell_back.decoder {
            DecoderUsed::Fallback(reason) => assert!(!reason.is_empty()),
            other => panic!("expected a fallback, got {other:?}"),
        }
        assert_eq!(fell_back.frames.len(), forced.frames.len());
        assert_eq!(fell_back.frames.len(), 24);
        assert_eq!(fell_back.duration_ms, forced.duration_ms);
        for (a, b) in fell_back.frames.iter().zip(&forced.frames) {
            assert_eq!(a.time_ms, b.time_ms);
            assert_eq!(a.scene_score, b.scene_score);
        }
        let note = decode_note(fell_back.format, &fell_back.decoder);
        assert!(note.contains("was tried and not used"), "{note}");
    }

    /// Frames from the media engine are copied into one reused software frame.
    /// HDR video carries side data (mastering display, light level, SEI
    /// messages) on every frame, and copying properties into a frame that still
    /// holds the last frame's side data adds to it: the count must not grow.
    #[cfg(target_os = "macos")]
    #[test]
    fn side_data_does_not_pile_up_on_the_reused_frame() {
        ensure_ffmpeg().expect("ffmpeg");
        let path =
            std::env::temp_dir().join(format!("reelctl-sidedata-{}.mp4", std::process::id()));
        let params = "log-level=error:master-display=G(13250,34500)B(7500,3000)R(34000,16000)\
            WP(15635,16450)L(10000000,1):max-cll=1000,400:hdr10-opt=1:repeat-headers=1:\
            colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc";
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=320x180:r=24:d=2"])
            .args(["-c:v", "libx265", "-preset", "ultrafast"])
            .args(["-pix_fmt", "yuv420p10le", "-tag:v", "hvc1"])
            .args(["-x265-params", params])
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg could not make the clip");

        let device = HardwareDevice::create().expect("VideoToolbox device");
        let mut input = ffmpeg::format::input(&path).expect("open clip");
        let (index, params) = {
            let stream = input.streams().best(MediaType::Video).expect("video");
            (stream.index(), stream.parameters())
        };
        let video = open_video(params, Some(&device)).expect("open decoder");
        let mut receiver = Receiver::new(video, true);
        let mut most = 0;
        let mut frames = 0;
        let mut drain = |receiver: &mut Receiver| {
            while let Ok(Received::Frame) = receiver.receive() {
                most = most.max(hwdecode::side_data_count(receiver.frame()));
                frames += 1;
            }
        };
        for (stream, packet) in input.packets() {
            if stream.index() == index {
                send_video(&mut receiver.video, &packet).expect("send");
                drain(&mut receiver);
            }
        }
        receiver.video.send_eof().expect("eof");
        drain(&mut receiver);
        let _ = std::fs::remove_file(&path);

        assert_eq!(frames, 48);
        assert!(most < 10, "a frame held {most} side data entries");
    }

    /// Resident memory of this process in KiB, from `ps`.
    #[cfg(target_os = "macos")]
    fn resident_kib() -> u64 {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .expect("spawn ps");
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .expect("rss")
    }

    /// Each hardware decode creates a device and a pool of frames and releases
    /// them with `unsafe` code, so decode one file many times and require the
    /// process to stop growing. The clip is 1280x720 so that a leaked p010 frame
    /// per decode (2.6 MiB) would add about 120 MiB over the 45 measured runs.
    #[cfg(target_os = "macos")]
    #[test]
    fn repeated_hardware_decodes_do_not_grow_memory() {
        ensure_ffmpeg().expect("ffmpeg");
        let path = std::env::temp_dir().join(format!("reelctl-leak-{}.mp4", std::process::id()));
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=1280x720:r=24:d=2"])
            .args([
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p10le",
            ])
            .args(["-tag:v", "hvc1", "-x265-params", "log-level=error"])
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg could not make the clip");

        let mut after_five = 0;
        for run in 1..=50 {
            let reel = decode_planned(&path, Plan::TryHardware).expect("decode");
            assert_eq!(reel.decoder, DecoderUsed::Hardware, "run {run}");
            assert_eq!(reel.frames.len(), 48);
            if run == 5 {
                after_five = resident_kib();
            }
        }
        let after_fifty = resident_kib();
        let _ = std::fs::remove_file(&path);
        let growth_mib = after_fifty.saturating_sub(after_five) / 1024;
        assert!(
            growth_mib < 20,
            "resident memory grew {growth_mib} MiB between run 5 and run 50"
        );
    }
}
