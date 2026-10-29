//! `timeline init`: one clip spanning the whole media, on a video track and, when
//! the file has audio, an audio track.

use serde_json::{json, Map, Value};

use crate::error::{Result, TimelineError};
use crate::otio::{
    reelctl, update_reelctl, Clip, ExternalReference, Item, MediaReference, TimeRange, Timeline,
    Track, AUDIO_KIND, VIDEO_KIND,
};
use crate::render::MAX_SIDE;

/// The key under `metadata.reelctl` of a timeline that records its edit DNA.
pub const DNA_KEY: &str = "dna";
/// The key under the reference's `metadata.reelctl` for what `init` measured.
pub const MEDIA_KEY: &str = "media";

/// What was measured from the media file.
#[derive(Debug, Clone, PartialEq)]
pub struct InitSpec {
    /// Absolute path of the media, as it will be written to `target_url`.
    pub media_path: String,
    /// Frame rate of the video.
    pub fps: f64,
    /// Number of video frames.
    pub frames: i64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    /// The edit DNA of this file, when one was given.
    pub dna: Option<DnaRef>,
}

/// The edit DNA a project was made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnaRef {
    /// Absolute path of the `edit_dna.json`.
    pub path: String,
    /// Its `duration_ms`, kept so a later command can tell the wrong file.
    pub duration_ms: u64,
}

/// The size, rate and audio of the media, as `init` recorded them.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub path: String,
    pub fps: f64,
    pub frames: i64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
}

/// Build the project for one media file.
pub fn init(spec: &InitSpec) -> Result<Timeline> {
    if !spec.fps.is_finite() || spec.fps <= 0.0 {
        return Err(TimelineError::invalid(format!(
            "the frame rate {} is not usable",
            spec.fps
        )));
    }
    if spec.frames <= 0 {
        return Err(TimelineError::invalid(
            "the media has no video frames to edit",
        ));
    }
    if spec.frames > crate::tracks::MAX_FRAMES {
        return Err(TimelineError::invalid(format!(
            "the media has {} video frames; at most {} can be edited",
            spec.frames,
            crate::tracks::MAX_FRAMES
        )));
    }
    let stem = std::path::Path::new(&spec.media_path)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("take")
        .to_string();
    let range = TimeRange::new(0.0, spec.frames as f64, spec.fps);
    let reference = |has_audio: bool| {
        let mut external = ExternalReference::new(&spec.media_path, range.clone());
        update_reelctl(&mut external.metadata, |map| {
            map.insert(
                MEDIA_KEY.to_string(),
                json!({
                    "width": spec.width,
                    "height": spec.height,
                    "fps": spec.fps,
                    "frames": spec.frames,
                    "has_audio": has_audio,
                }),
            );
        });
        MediaReference::External(external)
    };
    let mut timeline = Timeline::new(&stem);
    let mut video = Track::new("Video", VIDEO_KIND);
    video.children.push(Item::Clip(Clip::new(
        &stem,
        range.clone(),
        reference(spec.has_audio),
    )));
    timeline.tracks.children.push(video);
    if spec.has_audio {
        let mut audio = Track::new("Audio", AUDIO_KIND);
        audio.children.push(Item::Clip(Clip::new(
            &stem,
            range.clone(),
            reference(spec.has_audio),
        )));
        timeline.tracks.children.push(audio);
    }
    if let Some(dna) = &spec.dna {
        set_dna_ref(&mut timeline, dna);
    }
    Ok(timeline)
}

/// Record which `edit_dna.json` belongs to the project's media.
pub fn set_dna_ref(timeline: &mut Timeline, dna: &DnaRef) {
    update_reelctl(&mut timeline.metadata, |map| {
        map.insert(
            DNA_KEY.to_string(),
            json!({ "path": dna.path, "duration_ms": dna.duration_ms }),
        );
    });
}

/// The edit DNA recorded in the project, if any.
pub fn dna_ref(timeline: &Timeline) -> Option<DnaRef> {
    let map = reelctl(&timeline.metadata)?.get(DNA_KEY)?.as_object()?;
    Some(DnaRef {
        path: map.get("path")?.as_str()?.to_string(),
        duration_ms: map.get("duration_ms")?.as_u64()?,
    })
}

/// What `init` measured about the project's media. Errors if the media is not an
/// `init` one (a file from another tool has no size to render with).
pub fn media_info(timeline: &Timeline) -> Result<MediaInfo> {
    let path = crate::tracks::single_media(timeline)?.to_string();
    for track in timeline
        .tracks
        .children
        .iter()
        .filter(|track| !track.is_overlay())
    {
        for item in &track.children {
            let Item::Clip(clip) = item else { continue };
            let Ok(MediaReference::External(external)) = clip.active_reference() else {
                continue;
            };
            let measured: Option<&Map<String, Value>> = reelctl(&external.metadata)
                .and_then(|map| map.get(MEDIA_KEY))
                .and_then(Value::as_object);
            let Some(measured) = measured else { continue };
            let number = |key: &str| measured.get(key).and_then(Value::as_f64);
            let (Some(fps), Some(frames), Some(width), Some(height)) = (
                number("fps"),
                number("frames"),
                number("width"),
                number("height"),
            ) else {
                continue;
            };
            let sides = 1.0..=f64::from(MAX_SIDE);
            if !sides.contains(&width) || !sides.contains(&height) {
                return Err(TimelineError::invalid(format!(
                    "\"{path}\": metadata.reelctl.{MEDIA_KEY} says {width}x{height}; each side must be 1 to {MAX_SIDE} pixels"
                )));
            }
            if !fps.is_finite() || fps <= 0.0 || !(1.0..=f64::from(i32::MAX)).contains(&frames) {
                return Err(TimelineError::invalid(format!(
                    "\"{path}\": metadata.reelctl.{MEDIA_KEY} says {frames} frames at {fps} fps; both must be above 0"
                )));
            }
            return Ok(MediaInfo {
                path,
                fps,
                frames: frames as i64,
                width: width as u32,
                height: height as u32,
                has_audio: measured
                    .get("has_audio")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
    }
    Err(TimelineError::invalid(format!(
        "\"{path}\" has no size or frame rate recorded under metadata.reelctl.{MEDIA_KEY}; make the project with `reelctl timeline init`"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracks::{check_editable, timeline_frames};

    fn spec(has_audio: bool) -> InitSpec {
        InitSpec {
            media_path: "/media/take one.mp4".to_string(),
            fps: 29.97002997,
            frames: 360,
            width: 1080,
            height: 1920,
            has_audio,
            dna: Some(DnaRef {
                path: "/media/take.edit_dna.json".to_string(),
                duration_ms: 12012,
            }),
        }
    }

    #[test]
    fn init_makes_one_clip_per_track_spanning_the_media() {
        let timeline = init(&spec(true)).expect("init");
        assert_eq!(timeline.name, "take one");
        assert_eq!(timeline.tracks.children.len(), 2);
        assert_eq!(check_editable(&timeline).expect("editable"), 29.97002997);
        assert_eq!(timeline_frames(&timeline).expect("frames"), 360);
        for track in &timeline.tracks.children {
            assert_eq!(track.children.len(), 1);
        }
    }

    #[test]
    fn a_file_without_audio_has_no_audio_track() {
        let timeline = init(&spec(false)).expect("init");
        assert_eq!(timeline.tracks.children.len(), 1);
        assert!(!media_info(&timeline).expect("info").has_audio);
    }

    #[test]
    fn init_records_the_dna_and_the_media_it_measured() {
        let timeline = init(&spec(true)).expect("init");
        assert_eq!(
            dna_ref(&timeline),
            Some(DnaRef {
                path: "/media/take.edit_dna.json".to_string(),
                duration_ms: 12012
            })
        );
        let info = media_info(&timeline).expect("info");
        assert_eq!(
            (info.width, info.height, info.frames, info.has_audio),
            (1080, 1920, 360, true)
        );
        assert_eq!(info.path, "/media/take one.mp4");
    }

    #[test]
    fn init_without_a_dna_records_none() {
        let mut no_dna = spec(true);
        no_dna.dna = None;
        let timeline = init(&no_dna).expect("init");
        assert_eq!(dna_ref(&timeline), None);
    }

    #[test]
    fn init_is_deterministic() {
        let a = init(&spec(true)).expect("init").to_json().expect("json");
        let b = init(&spec(true)).expect("init").to_json().expect("json");
        assert_eq!(a, b);
    }

    #[test]
    fn init_refuses_a_media_with_no_frames_or_rate() {
        let mut bad = spec(true);
        bad.frames = 0;
        assert!(init(&bad).is_err());
        let mut bad = spec(true);
        bad.fps = 0.0;
        assert!(init(&bad).is_err());
        let mut bad = spec(true);
        bad.frames = crate::tracks::MAX_FRAMES + 1;
        let error = init(&bad).expect_err("too long");
        assert!(error.to_string().contains("2147483647"), "{error}");
    }

    #[test]
    fn a_foreign_project_has_no_measured_size() {
        let timeline = Timeline::from_json(include_str!("../tests/reference.otio")).expect("parse");
        let error = media_info(&timeline).expect_err("no size");
        assert!(error.to_string().contains("/x/a.mp4"), "{error}");
    }
}
