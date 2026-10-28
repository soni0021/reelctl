//! Frames at chosen times, scaled to RGB, for a vision-language model.
//!
//! `decompile` reads every frame at a few sampled points; this reads a few frames
//! whole. It decodes the video once, in presentation order, and converts only the
//! frames it keeps, with libswscale (no process spawn). For each requested time it
//! keeps the first frame at or after it, so a frame is at most one frame period late.
//! A time past the last frame gets the last frame.
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
/// aspect ratio, rounded to even). The result has one entry per requested time, in
/// the order asked, duplicates included.
pub fn decode_frames_at(
    path: &Path,
    times_ms: &[u64],
    width: u32,
    setting: DecoderSetting,
) -> Result<Vec<SampledFrame>, DecompileError> {
    let _ = (path, times_ms, width, setting);
    Err(DecompileError::Message("not implemented".to_string()))
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
        let auto = decode_frames_at(&path, &[500, 1_500], 80, DecoderSetting::Auto).expect("auto");
        let _ = std::fs::remove_file(&path);
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
