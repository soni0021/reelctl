//! How long a video is, without decoding it.

use super::*;

/// The length of the file's video in milliseconds: the container's own duration
/// when it states one, else the end of the last video packet, or for a raw
/// elementary stream with no timestamps its packet count over the frame rate (a
/// demux, not a decode). 0 only for a file with no video packets.
pub fn probe_duration_ms(path: &Path) -> Result<u64, DecompileError> {
    ensure_ffmpeg()?;
    let mut input = ffmpeg::format::input(path).map_err(|err| stage("open input", err))?;
    let ticks = input.duration();
    if ticks > 0 {
        return Ok((ticks / 1_000) as u64);
    }
    let (index, time_base, start, fps) = {
        let stream = input
            .streams()
            .best(MediaType::Video)
            .ok_or_else(|| DecompileError::NoVideo(path.display().to_string()))?;
        (
            stream.index(),
            stream.time_base(),
            stream.start_time().max(0),
            positive_rational(stream.avg_frame_rate())
                .or_else(|| positive_rational(stream.rate()))
                .unwrap_or(Rational::new(30, 1)),
        )
    };
    let mut end = 0i64;
    let mut packets = 0u64;
    for (stream, packet) in input.packets() {
        if stream.index() != index {
            continue;
        }
        packets += 1;
        if let Some(at) = packet.pts().or_else(|| packet.dts()) {
            end = end.max(at.saturating_add(packet.duration().max(0)));
        }
    }
    let timed = ticks_to_ms(end.saturating_sub(start), time_base);
    if timed > 0 {
        return Ok(timed);
    }
    // A raw stream has no timestamps: one packet is one frame, as in `decode_with`.
    Ok(index_to_ms(packets, fps))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(name: &str, args: &[&str], seconds: u32) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("reelctl-probe-{}-{name}", std::process::id()));
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i"])
            .arg(format!("color=c=green:s=160x90:r=24:d={seconds}"))
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .args(args)
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg could not make the clip");
        path
    }

    #[test]
    fn a_container_states_its_duration() {
        let path = make("mp4.mp4", &[], 3);
        let ms = probe_duration_ms(&path).expect("duration");
        let _ = std::fs::remove_file(&path);
        assert!((2_900..=3_100).contains(&ms), "{ms}");
    }

    #[test]
    fn a_raw_stream_is_measured_from_its_packets() {
        let path = make("raw.h264", &["-f", "h264"], 3);
        let ms = probe_duration_ms(&path).expect("duration");
        let _ = std::fs::remove_file(&path);
        // A raw stream states no rate; FFmpeg assumes 25 fps for these 24 fps frames
        // (72 / 25 = 2.88 s), the same rate `decompile` times its frames by.
        assert!((2_800..=3_100).contains(&ms), "{ms}");
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let missing = std::env::temp_dir().join("reelctl-probe-missing.mp4");
        assert!(probe_duration_ms(&missing).is_err());
    }
}
