//! Frames at chosen times, scaled to RGB, for a vision-language model.
//!
//! `decompile` reads every frame at a few sampled points; this reads a few frames
//! whole. It decodes the video once, in presentation order, and converts only the
//! frames it keeps, with libswscale (no process spawn). For each requested time it
//! keeps the first frame at or after it, so a frame is at most one frame period late.
//! A time past the last frame gets the last frame, so the newest frame that was not
//! scaled is kept as one raw copy (about one frame of memory, one copy per frame).
//!
//! Colors follow libswscale's defaults (BT.601), and HDR is not tone-mapped, as in
//! ADR-011; that is enough for a model to read slides and faces.

use super::*;

/// One requested time and the frame kept for it.
#[derive(Debug, Clone)]
pub struct SampledFrame {
    /// The time that was asked for.
    pub requested_ms: u64,
    /// The time of the frame kept, on the same clock as `decompile`'s cuts.
    pub time_ms: u64,
    pub width: u32,
    pub height: u32,
    /// `width * height * 3` bytes, RGB, rows packed.
    pub rgb: Vec<u8>,
}

/// Decode the frames at `times_ms`, scaled to `width` pixels wide (height keeps the
/// aspect ratio, rounded up to even). The result has one entry per requested time,
/// in the order asked, duplicates included. HEVC tries the media engine first, as
/// `decompile` does, and falls back to software on any failure.
pub fn decode_frames_at(
    path: &Path,
    times_ms: &[u64],
    width: u32,
    setting: DecoderSetting,
) -> Result<Vec<SampledFrame>, DecompileError> {
    decode_frames_at_with_decoder(path, times_ms, width, setting).map(|(frames, _)| frames)
}

/// `decode_frames_at`, and which decoder produced the frames.
pub(crate) fn decode_frames_at_with_decoder(
    path: &Path,
    times_ms: &[u64],
    width: u32,
    setting: DecoderSetting,
) -> Result<(Vec<SampledFrame>, DecoderUsed), DecompileError> {
    ensure_ffmpeg().map_err(|err| stage("ffmpeg init", err))?;
    if times_ms.is_empty() {
        return Ok((Vec::new(), DecoderUsed::Software));
    }
    match choose_decoder(video_codec(path)?, setting) {
        Plan::Software(used) => Ok((sample_with(path, None, times_ms, width)?, used)),
        Plan::TryHardware => {
            let attempt = HardwareDevice::create().and_then(|device| {
                sample_with(path, Some(&device), times_ms, width).map_err(|err| err.to_string())
            });
            match attempt {
                Ok(frames) => Ok((frames, DecoderUsed::Hardware)),
                Err(reason) => Ok((
                    sample_with(path, None, times_ms, width)?,
                    DecoderUsed::Fallback(reason),
                )),
            }
        }
    }
}

/// Whether a frame shown at `time_ms` is too far before `target` to be worth
/// scaling: more than two frame periods early. A later frame will serve it unless this
/// turns out to be the last one, which is why [`Sampler::keep_raw`] keeps a copy.
fn far_before(target: u64, time_ms: u64, frame_ms: u64) -> bool {
    target > time_ms && target - time_ms > frame_ms.saturating_mul(2)
}

/// The latest frame converted, for times past the last frame.
struct Latest {
    time_ms: u64,
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

/// Picks frames for the requested times and scales them.
struct Sampler<'a> {
    times_ms: &'a [u64],
    /// Indices into `times_ms`, by time.
    order: Vec<usize>,
    next: usize,
    results: Vec<Option<SampledFrame>>,
    width: u32,
    frame_ms: u64,
    latest: Option<Latest>,
    /// A copy of the newest frame, unscaled, when it was too far before the next
    /// time to be scaled. It is newer than `latest` and is scaled only if no later
    /// frame arrives, so a time past the end still gets the last frame.
    last_raw: Option<(u64, VideoFrame)>,
    /// The scaler and the source it was made for.
    scaler: Option<((Pixel, u32, u32), ffmpeg::software::scaling::Context)>,
    out: VideoFrame,
}

impl<'a> Sampler<'a> {
    fn new(times_ms: &'a [u64], width: u32, frame_ms: u64) -> Self {
        let mut order: Vec<usize> = (0..times_ms.len()).collect();
        order.sort_by_key(|&index| times_ms[index]);
        Self {
            times_ms,
            order,
            next: 0,
            results: (0..times_ms.len()).map(|_| None).collect(),
            width: width.max(2),
            frame_ms,
            latest: None,
            last_raw: None,
            scaler: None,
            out: VideoFrame::empty(),
        }
    }

    fn done(&self) -> bool {
        self.next >= self.order.len()
    }

    fn target(&self) -> Option<u64> {
        self.order
            .get(self.next)
            .and_then(|&index| self.times_ms.get(index))
            .copied()
    }

    /// Scale `frame` (shown at `time_ms`) if it is at or near a pending time. A frame
    /// far before one is only kept unscaled, in case it is the last frame.
    fn observe(&mut self, frame: &VideoFrame, time_ms: u64) -> Result<(), DecompileError> {
        let Some(target) = self.target() else {
            return Ok(());
        };
        if far_before(target, time_ms, self.frame_ms) {
            self.keep_raw(frame, time_ms);
            return Ok(());
        }
        self.last_raw = None;
        let (width, height, rgb) = self.scale(frame)?;
        while let Some(target) = self.target() {
            if target > time_ms {
                break;
            }
            let index = self.order.get(self.next).copied().unwrap_or(0);
            if let Some(slot) = self.results.get_mut(index) {
                *slot = Some(SampledFrame {
                    requested_ms: target,
                    time_ms,
                    width,
                    height,
                    rgb: rgb.clone(),
                });
            }
            self.next += 1;
        }
        if !self.done() {
            self.latest = Some(Latest {
                time_ms,
                width,
                height,
                rgb,
            });
        }
        Ok(())
    }

    /// Remember `frame` unscaled as the newest one, reusing the last copy's
    /// buffers when it has the same shape.
    fn keep_raw(&mut self, frame: &VideoFrame, time_ms: u64) {
        match self.last_raw.as_mut() {
            Some((kept_ms, kept))
                if kept.format() == frame.format()
                    && kept.width() == frame.width()
                    && kept.height() == frame.height() =>
            {
                crate::hwdecode::copy_frame(kept, frame);
                *kept_ms = time_ms;
            }
            _ => self.last_raw = Some((time_ms, frame.clone())),
        }
    }

    /// Scale to RGB at the target width: `(width, height, packed rows)`.
    fn scale(&mut self, frame: &VideoFrame) -> Result<(u32, u32, Vec<u8>), DecompileError> {
        let (src_w, src_h, format) = (frame.width(), frame.height(), frame.format());
        if src_w == 0 || src_h == 0 {
            return Err(DecompileError::Message(
                "a decoded frame has no size".into(),
            ));
        }
        let width = self.width;
        // Rounded up to even, so every pixel pair is whole.
        let height = ((u64::from(src_h) * u64::from(width) / u64::from(src_w)) as u32 + 1) & !1;
        let height = height.max(2);
        let key = (format, src_w, src_h);
        if self.scaler.as_ref().map(|(made, _)| *made) != Some(key) {
            let context = ffmpeg::software::scaling::Context::get(
                format,
                src_w,
                src_h,
                Pixel::RGB24,
                width,
                height,
                ffmpeg::software::scaling::Flags::AREA,
            )
            .map_err(|err| stage("scale a frame", err))?;
            self.scaler = Some((key, context));
            self.out = VideoFrame::empty();
        }
        let Some((_, context)) = self.scaler.as_mut() else {
            return Err(DecompileError::Message("no scaler".into()));
        };
        context
            .run(frame, &mut self.out)
            .map_err(|err| stage("scale a frame", err))?;
        let stride = self.out.stride(0);
        let data = self.out.data(0);
        let row = width as usize * 3;
        let mut rgb = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let start = y * stride;
            let line = data.get(start..start + row).ok_or_else(|| {
                DecompileError::Message("a scaled frame is shorter than its size".into())
            })?;
            rgb.extend_from_slice(line);
        }
        Ok((width, height, rgb))
    }

    /// Give every time still pending the latest frame kept, and collect the results.
    fn finish(mut self, path: &Path) -> Result<Vec<SampledFrame>, DecompileError> {
        if let Some((time_ms, raw)) = self.last_raw.take() {
            let (width, height, rgb) = self.scale(&raw)?;
            self.latest = Some(Latest {
                time_ms,
                width,
                height,
                rgb,
            });
        }
        while let Some(target) = self.target() {
            let latest = self
                .latest
                .as_ref()
                .ok_or_else(|| DecompileError::NoFrames(path.display().to_string()))?;
            let index = self.order.get(self.next).copied().unwrap_or(0);
            if let Some(slot) = self.results.get_mut(index) {
                *slot = Some(SampledFrame {
                    requested_ms: target,
                    time_ms: latest.time_ms,
                    width: latest.width,
                    height: latest.height,
                    rgb: latest.rgb.clone(),
                });
            }
            self.next += 1;
        }
        self.results
            .into_iter()
            .map(|slot| slot.ok_or_else(|| DecompileError::NoFrames(path.display().to_string())))
            .collect()
    }
}

fn sample_with(
    path: &Path,
    device: Option<&HardwareDevice>,
    times_ms: &[u64],
    width: u32,
) -> Result<Vec<SampledFrame>, DecompileError> {
    let mut input = ffmpeg::format::input(path).map_err(|err| stage("open input", err))?;
    let video_index = input
        .streams()
        .best(MediaType::Video)
        .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?
        .index();
    let (time_base, start_time, fps) = {
        let stream = input
            .streams()
            .find(|stream| stream.index() == video_index)
            .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?;
        let fps = positive_rational(stream.avg_frame_rate())
            .or_else(|| positive_rational(stream.rate()))
            .unwrap_or(Rational::new(30, 1));
        (stream.time_base(), stream.start_time(), fps)
    };
    let video = {
        let stream = input
            .streams()
            .find(|stream| stream.index() == video_index)
            .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?;
        open_video(stream.parameters(), device).map_err(|err| stage("open video decoder", err))?
    };
    let mut receiver = Receiver::new(video, device.is_some());
    let mut sampler = Sampler::new(times_ms, width, frame_duration_ms(fps));
    let mut index = 0u64;

    'packets: for (stream, packet) in input.packets() {
        if stream.index() != video_index {
            continue;
        }
        send_video(&mut receiver.video, &packet).map_err(|err| stage("send video packet", err))?;
        while let Received::Frame = receiver
            .receive()
            .map_err(|err| stage("decode video frame", err))?
        {
            let time_ms = frame_time_ms(receiver.frame(), index, time_base, start_time, fps);
            index += 1;
            sampler.observe(receiver.frame(), time_ms)?;
            if sampler.done() {
                break 'packets;
            }
        }
    }
    if !sampler.done() {
        match receiver.video.send_eof() {
            Ok(()) => {}
            Err(err) if is_retry(&err) => {}
            Err(err) => return Err(stage("flush video decoder", err)),
        }
        while let Received::Frame = receiver
            .receive()
            .map_err(|err| stage("decode video frame", err))?
        {
            let time_ms = frame_time_ms(receiver.frame(), index, time_base, start_time, fps);
            index += 1;
            sampler.observe(receiver.frame(), time_ms)?;
            if sampler.done() {
                break;
            }
        }
    }
    sampler.finish(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clip made with the ffmpeg CLI: one second red, then one second blue,
    /// 24 fps, 160x90, in the given codec arguments.
    fn red_then_blue(name: &str, codec: &[&str]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("reelctl-sample-{name}-{}.mp4", std::process::id()));
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "color=c=red:s=160x90:r=24:d=1"])
            .args(["-f", "lavfi", "-i", "color=c=blue:s=160x90:r=24:d=1"])
            .args([
                "-filter_complex",
                "[0:v][1:v]concat=n=2:v=1:a=0[v]",
                "-map",
                "[v]",
            ])
            .args(codec)
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg could not make the clip");
        path
    }

    fn h264() -> Vec<&'static str> {
        vec![
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "12",
        ]
    }

    fn centre(frame: &SampledFrame) -> [u8; 3] {
        let at = ((frame.height / 2 * frame.width + frame.width / 2) * 3) as usize;
        [frame.rgb[at], frame.rgb[at + 1], frame.rgb[at + 2]]
    }

    fn near(actual: [u8; 3], expected: [u8; 3], tolerance: u8) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| a.abs_diff(e) <= tolerance)
    }

    #[test]
    fn frames_come_back_in_the_order_asked_with_their_colors() {
        let path = red_then_blue("order", &h264());
        // 1.5 s is blue, 0.5 s is red, and a repeated time is kept.
        let frames = decode_frames_at(&path, &[1_500, 500, 1_500], 80, DecoderSetting::Software)
            .expect("frames");
        let _ = std::fs::remove_file(&path);
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames.iter().map(|f| f.requested_ms).collect::<Vec<_>>(),
            [1_500, 500, 1_500]
        );
        assert!(
            near(centre(&frames[0]), [0, 0, 255], 12),
            "{:?}",
            centre(&frames[0])
        );
        assert!(
            near(centre(&frames[1]), [255, 0, 0], 12),
            "{:?}",
            centre(&frames[1])
        );
        assert_eq!(frames[0].rgb, frames[2].rgb);
    }

    #[test]
    fn a_frame_is_the_first_at_or_after_the_time() {
        let path = red_then_blue("after", &h264());
        // 24 fps: frames every 41.67 ms. 1,001 ms is in frame 24 (1,000 ms), whose
        // successor is at 1,041 ms, so the kept frame is the one at 1,041 ms (blue).
        let frames = decode_frames_at(&path, &[0, 1_001, 990], 80, DecoderSetting::Software)
            .expect("frames");
        let _ = std::fs::remove_file(&path);
        assert_eq!(frames[0].time_ms, 0);
        assert!(
            frames[1].time_ms >= 1_001 && frames[1].time_ms < 1_001 + 42,
            "{}",
            frames[1].time_ms
        );
        assert!(
            frames[2].time_ms >= 990 && frames[2].time_ms < 990 + 42,
            "{}",
            frames[2].time_ms
        );
        for frame in &frames {
            assert!(frame.time_ms >= frame.requested_ms, "{frame:?}");
        }
    }

    #[test]
    fn a_time_past_the_last_frame_gets_the_last_frame() {
        let path = red_then_blue("past", &h264());
        let frames =
            decode_frames_at(&path, &[1_990, 5_000], 80, DecoderSetting::Software).expect("frames");
        let _ = std::fs::remove_file(&path);
        assert_eq!(frames.len(), 2);
        assert!(near(centre(&frames[1]), [0, 0, 255], 12));
        assert!(frames[1].time_ms >= frames[0].time_ms);
        assert!(frames[1].time_ms <= 2_000, "{}", frames[1].time_ms);
    }

    #[test]
    fn several_times_past_the_end_all_get_the_last_decoded_frame() {
        let path = red_then_blue("pastmany", &h264());
        // 2 s at 24 fps: 48 frames, the last shown at 1,958 ms.
        let frames = decode_frames_at(
            &path,
            &[500, 5_000, 6_000, 7_000],
            80,
            DecoderSetting::Software,
        )
        .expect("frames");
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            frames.iter().map(|f| f.requested_ms).collect::<Vec<_>>(),
            [500, 5_000, 6_000, 7_000]
        );
        assert!(near(centre(&frames[0]), [255, 0, 0], 12));
        assert!(frames[0].time_ms >= 500 && frames[0].time_ms < 500 + 42);
        for frame in &frames[1..] {
            assert!(near(centre(frame), [0, 0, 255], 12), "{:?}", centre(frame));
            assert!(
                (1_950..=1_960).contains(&frame.time_ms),
                "a time past the end got the frame at {} ms",
                frame.time_ms
            );
        }
        assert_eq!(frames[1].rgb, frames[3].rgb);
    }

    #[test]
    fn one_time_past_the_end_is_not_an_error() {
        let path = red_then_blue("pastone", &h264());
        let frames = decode_frames_at(&path, &[5_000], 80, DecoderSetting::Software)
            .expect("a time past the end still gets a frame");
        let _ = std::fs::remove_file(&path);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].requested_ms, 5_000);
        assert!(
            (1_950..=1_960).contains(&frames[0].time_ms),
            "{}",
            frames[0].time_ms
        );
        assert!(near(centre(&frames[0]), [0, 0, 255], 12));
    }

    #[test]
    fn a_frame_is_far_before_a_time_only_beyond_two_frame_periods() {
        // 42 ms frames: exactly two periods (84 ms) early is still worth scaling.
        assert!(!far_before(1_000, 916, 42));
        assert!(far_before(1_000, 915, 42));
        // At or after the target is never early.
        assert!(!far_before(1_000, 1_000, 42));
        assert!(!far_before(1_000, 1_500, 42));
        // A huge frame period does not overflow.
        assert!(!far_before(1_000, 0, u64::MAX));
    }

    #[test]
    fn the_size_keeps_the_aspect_ratio_with_an_even_height() {
        let path = red_then_blue("size", &h264());
        let frames = decode_frames_at(&path, &[0], 80, DecoderSetting::Software).expect("frames");
        let _ = std::fs::remove_file(&path);
        assert_eq!((frames[0].width, frames[0].height), (80, 46));
        assert_eq!(frames[0].rgb.len(), 80 * 46 * 3);
    }

    #[test]
    fn no_times_decode_nothing_and_no_video_is_an_error() {
        let path = red_then_blue("none", &h264());
        assert!(decode_frames_at(&path, &[], 80, DecoderSetting::Software)
            .expect("frames")
            .is_empty());
        let _ = std::fs::remove_file(&path);
        let missing = std::env::temp_dir().join("reelctl-sample-missing.mp4");
        assert!(decode_frames_at(&missing, &[0], 80, DecoderSetting::Software).is_err());
    }

    #[test]
    fn ten_bit_hevc_gives_the_same_colors_and_the_media_engine_agrees() {
        let hevc = [
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p10le",
            "-tag:v",
            "hvc1",
            "-x265-params",
            "log-level=error:keyint=12",
        ];
        let path = red_then_blue("ten", &hevc);
        let soft =
            decode_frames_at(&path, &[500, 1_500], 80, DecoderSetting::Software).expect("software");
        let (auto, used) =
            decode_frames_at_with_decoder(&path, &[500, 1_500], 80, DecoderSetting::Auto)
                .expect("auto");
        let _ = std::fs::remove_file(&path);
        if cfg!(target_os = "macos") {
            // A silent fallback to software would match software by construction.
            let note = decode_note(VideoFormat::default(), &used);
            assert!(
                used == DecoderUsed::Hardware && note.contains("Apple media engine"),
                "the media engine did not run: {used:?}: {note}"
            );
        }
        assert!(
            near(centre(&soft[0]), [255, 0, 0], 16),
            "{:?}",
            centre(&soft[0])
        );
        assert!(
            near(centre(&soft[1]), [0, 0, 255], 16),
            "{:?}",
            centre(&soft[1])
        );
        for (a, s) in auto.iter().zip(&soft) {
            assert_eq!(a.time_ms, s.time_ms);
            assert!(
                near(centre(a), centre(s), 3),
                "{:?} vs {:?}",
                centre(a),
                centre(s)
            );
        }
    }
}
