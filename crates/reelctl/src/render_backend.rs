//! The seam between `reelctl render` and the program that does the rendering. ffmpeg
//! (the default) builds one filter graph; Smelter (optional) is driven over its
//! HTTP API. Both take the same project and give the same timeline, and each says
//! where it cannot (spec: `docs/specs/smelter-backend.md`, ADR-023).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;
use timeline::{CardSpec, RenderSize, Timeline};

use crate::card::{draw_card, parse_color};

/// `--backend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendChoice {
    /// One ffmpeg filter graph (the default).
    Ffmpeg,
    /// A separately installed Smelter, driven over its HTTP API.
    Smelter,
}

/// What a backend is asked to render.
pub struct RenderJob<'a> {
    pub project_path: &'a Path,
    pub project: &'a Timeline,
    pub out: &'a Path,
    pub size: Option<RenderSize>,
}

/// What a backend did, for the `render:` line.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOutcome {
    pub frames: i64,
    pub duration_ms: u64,
    pub size: RenderSize,
    /// Milliseconds the backend's own work took (ffmpeg, or Smelter).
    pub backend_ms: u128,
    /// Lines printed after the result: what the backend could not reproduce.
    pub notes: Vec<String>,
}

pub trait RenderBackend {
    /// The backend's name as `--backend` spells it; also the unit of the timing field
    /// (`ffmpeg_ms`, `smelter_ms`).
    fn name(&self) -> &'static str;

    /// Render `job.project` to `job.out`. The output is written beside `job.out` and
    /// renamed, and nothing is left behind on a failure.
    fn render(&self, job: &RenderJob<'_>) -> Result<RenderOutcome>;
}

/// The ffmpeg backend: the render `reelctl` has always had.
pub struct FfmpegBackend;

impl RenderBackend for FfmpegBackend {
    fn name(&self) -> &'static str {
        "ffmpeg"
    }

    fn render(&self, job: &RenderJob<'_>) -> Result<RenderOutcome> {
        crate::timeline_cmd::render_ffmpeg(job)
    }
}

/// The `render:` result line. The ffmpeg backend's is the one it printed before
/// there were two backends.
pub fn result_line(backend: &str, out: &Path, outcome: &RenderOutcome) -> String {
    format!(
        "render: out={} frames={} duration_ms={} size={}x{} {backend}_ms={}",
        out.display(),
        outcome.frames,
        outcome.duration_ms,
        outcome.size.width,
        outcome.size.height,
        outcome.backend_ms
    )
}

/// Draw each card into `dir` as `card0.png`, ...: the PNG the render shows, the size of
/// the output. Returns the files and a note for each card whose text was cut.
pub fn draw_card_files(
    cards: &[CardSpec],
    size: RenderSize,
    dir: &Path,
) -> Result<(Vec<PathBuf>, Vec<String>)> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let mut paths = Vec::with_capacity(cards.len());
    let mut notes = Vec::new();
    for (index, card) in cards.iter().enumerate() {
        let drawn = draw_card(
            &card.text,
            parse_color(&card.background)?,
            parse_color(&card.foreground)?,
            size.width,
            size.height,
        );
        if drawn.truncated() {
            notes.push(format!(
                "render: the card text was too long for the frame; it shows {} of {} characters and ends in ...",
                drawn.shown.chars().count(),
                drawn.total
            ));
        }
        let path = dir.join(format!("card{index}.png"));
        drawn
            .image
            .save(&path)
            .with_context(|| format!("write {}", path.display()))?;
        paths.push(path);
    }
    Ok((paths, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_line_keeps_the_ffmpeg_format_and_names_the_backend() {
        let outcome = RenderOutcome {
            frames: 72,
            duration_ms: 3000,
            size: RenderSize {
                width: 640,
                height: 360,
            },
            backend_ms: 412,
            notes: Vec::new(),
        };
        assert_eq!(
            result_line("ffmpeg", Path::new("/tmp/o.mp4"), &outcome),
            "render: out=/tmp/o.mp4 frames=72 duration_ms=3000 size=640x360 ffmpeg_ms=412"
        );
        assert!(result_line("smelter", Path::new("o.mp4"), &outcome).ends_with(" smelter_ms=412"));
    }

    #[test]
    fn cards_are_drawn_at_the_output_size_and_a_long_text_is_a_note() {
        let dir = std::env::temp_dir().join(format!("reelctl-cards-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("dir");
        let card = |text: &str| CardSpec {
            text: text.to_string(),
            background: "#3a1c71".to_string(),
            foreground: "#ffffff".to_string(),
            frames: 10,
        };
        let size = RenderSize {
            width: 160,
            height: 90,
        };
        let long = "A".repeat(4000);
        let (paths, notes) =
            draw_card_files(&[card("a.co/x"), card(&long)], size, &dir).expect("cards");
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("card0.png") && paths[1].ends_with("card1.png"));
        let image = image::open(&paths[0]).expect("png");
        assert_eq!((image.width(), image.height()), (160, 90));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("too long"));
        let _ = fs::remove_dir_all(&dir);
    }
}
