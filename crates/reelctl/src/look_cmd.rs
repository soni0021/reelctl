//! `reelctl look`: a `.cube` 3D LUT that moves a draft's colour toward an outlier's.
//!
//! It samples evenly spaced frames from both videos, matches each channel's
//! cumulative distribution (per-channel histogram matching), and writes the cube.
//! It never uses the timed `decompile` path, so it adds nothing to `signal_ms`.
//! Exit codes: 2 for a bad argument or a file that cannot be read (before anything is
//! decoded), 1 for a decode failure.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::{Args, CommandFactory};
use decompile::{decode_frames_at, probe_duration_ms, DecoderSetting};
use look::{match_curve, sample_times_ms, Cube, Curve, Histograms, CUBE_SIZES};

use super::{Cli, DecoderArg};

/// Frames are decoded at this width; the colour statistics do not need more.
const FRAME_WIDTH: u32 = 320;
/// The most frames taken from one video.
const MAX_SAMPLES: usize = 600;

#[derive(Args)]
pub struct LookArgs {
    /// The video to grade (the one the LUT will be applied to).
    #[arg(long)]
    draft: PathBuf,
    /// The video whose colour is wanted.
    #[arg(long)]
    outlier: PathBuf,
    /// Where to write the `.cube` file (Adobe/Resolve format).
    #[arg(long)]
    out: PathBuf,
    /// Cube edge: 17, 33 or 65 nodes.
    #[arg(long, default_value_t = 33)]
    size: usize,
    /// Frames taken from each video, evenly spaced (1 to 600).
    #[arg(long, default_value_t = 60)]
    samples: usize,
    /// Video decoder, as for `decompile`.
    #[arg(long, value_enum, default_value_t = DecoderArg::Auto)]
    decoder: DecoderArg,
}

/// An argument error: clap's usage message for this subcommand, exit code 2.
fn usage_error(message: String) -> ! {
    let mut command = Cli::command();
    match command.find_subcommand_mut("look") {
        Some(sub) => sub.error(ErrorKind::ValueValidation, message).exit(),
        None => command.error(ErrorKind::ValueValidation, message).exit(),
    }
}

/// Exit 2 unless `path` is a file this process can open.
fn require_readable(flag: &str, path: &Path) {
    let problem = match fs::metadata(path) {
        Ok(meta) if !meta.is_file() => Some("is not a file".to_string()),
        Ok(_) => File::open(path).err().map(|err| err.to_string()),
        Err(err) => Some(err.to_string()),
    };
    if let Some(problem) = problem {
        usage_error(format!("{flag} {}: {problem}", path.display()));
    }
}

/// The colour histograms of `samples` frames spread over the video at `path`.
fn sampled_histograms(path: &Path, samples: usize, setting: DecoderSetting) -> Result<Histograms> {
    let duration_ms = probe_duration_ms(path)
        .with_context(|| format!("read the duration of {}", path.display()))?;
    let times = sample_times_ms(duration_ms, samples);
    let frames = decode_frames_at(path, &times, FRAME_WIDTH, setting)
        .with_context(|| format!("decode frames of {}", path.display()))?;
    let mut histograms = Histograms::default();
    for frame in &frames {
        histograms.add_rgb(&frame.rgb);
    }
    Ok(histograms)
}

/// A file's name without its directory.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A shift to two decimals, without a "-0.00".
fn shift_text(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    format!("{:.2}", if rounded == 0.0 { 0.0 } else { rounded })
}

pub fn look(args: LookArgs) -> Result<()> {
    if !CUBE_SIZES.contains(&args.size) {
        usage_error(format!("--size must be 17, 33 or 65, got {}", args.size));
    }
    if !(1..=MAX_SAMPLES).contains(&args.samples) {
        usage_error(format!(
            "--samples must be from 1 to {MAX_SAMPLES}, got {}",
            args.samples
        ));
    }
    require_readable("--draft", &args.draft);
    require_readable("--outlier", &args.outlier);

    let setting = args.decoder.setting();
    let draft = sampled_histograms(&args.draft, args.samples, setting)?;
    let outlier = sampled_histograms(&args.outlier, args.samples, setting)?;

    let [draft_r, draft_g, draft_b] = &draft.channels;
    let [outlier_r, outlier_g, outlier_b] = &outlier.channels;
    let curves: [Curve; 3] = [
        match_curve(draft_r, outlier_r),
        match_curve(draft_g, outlier_g),
        match_curve(draft_b, outlier_b),
    ];
    let title = format!(
        "reelctl look: {} to the colour of {}",
        file_name(&args.draft),
        file_name(&args.outlier)
    );
    let cube = Cube::from_curves(&title, args.size, &curves).context("build the cube")?;
    let shift = cube
        .mean_shift(&draft)
        .context("the draft gave no pixels")?;

    fs::write(&args.out, cube.to_cube_string())
        .with_context(|| format!("write {}", args.out.display()))?;
    out!(
        "size={} samples_draft={} samples_outlier={} mean_shift_r={} mean_shift_g={} mean_shift_b={} out={}",
        args.size,
        args.samples,
        args.samples,
        shift_text(shift[0]),
        shift_text(shift[1]),
        shift_text(shift[2]),
        args.out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{file_name, shift_text};

    #[test]
    fn a_shift_has_two_decimals_and_never_prints_a_negative_zero() {
        assert_eq!(shift_text(12.3456), "12.35");
        assert_eq!(shift_text(-7.004), "-7.00");
        assert_eq!(shift_text(-0.004), "0.00");
        assert_eq!(shift_text(-0.0), "0.00");
        assert_eq!(shift_text(0.0), "0.00");
    }

    #[test]
    fn the_title_uses_file_names_only() {
        assert_eq!(file_name(Path::new("/tmp/a/b/draft.mp4")), "draft.mp4");
        assert_eq!(file_name(Path::new("draft.mp4")), "draft.mp4");
    }
}
