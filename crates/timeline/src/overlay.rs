//! Overlays: clips of rendered video (a chart from `manimgx`) that play over the
//! main video for a span. They live on a second `Video` track marked
//! `metadata.reelctl.overlays`, as `Gap.1` and `Clip.2` in time order, and the render
//! composites them. The track is rebuilt from the list of overlays on every change.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, TimelineError};
use crate::otio::{
    reelctl, update_reelctl, Clip, ExternalReference, Gap, Item, MediaReference, TimeRange,
    Timeline, Track, OVERLAYS_MARKER, VIDEO_KIND,
};
use crate::tracks::{frame_ms, project_rate, round_frame, timeline_frames};

/// The key under `metadata.reelctl` of a clip, holding its overlay.
pub const OVERLAY_KEY: &str = "overlay";
/// The name of the overlay track. The marker, not the name, identifies it.
pub const OVERLAY_TRACK_NAME: &str = "reelctl overlays";
/// The colour the scenes are drawn on and the render keys out.
pub const KEY_COLOR: &str = "#ff00ff";
/// `colorkey` similarity and blend for [`KEY_COLOR`].
pub const KEY_SIMILARITY: &str = "0.2";
pub const KEY_BLEND: &str = "0.1";
/// The shortest and longest span of an overlay, in milliseconds.
pub const MIN_OVERLAY_MS: u64 = 500;
pub const MAX_OVERLAY_MS: u64 = 30_000;
/// The narrowest and widest overlay, as a percentage of the output width.
pub const MIN_WIDTH_PCT: u32 = 20;
pub const MAX_WIDTH_PCT: u32 = 100;
/// The gap between an overlay at the top or bottom and the frame's edge, as a
/// fraction of the frame's height.
pub const EDGE_MARGIN: &str = "0.08";

/// Where an overlay sits in the frame; always centred across.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    Top,
    #[default]
    Center,
    Bottom,
}

impl OverlayPosition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Center => "center",
            Self::Bottom => "bottom",
        }
    }

    /// The `y` expression of ffmpeg's `overlay` filter.
    pub fn y_expression(self) -> String {
        match self {
            Self::Top => format!("main_h*{EDGE_MARGIN}"),
            Self::Center => "(main_h-overlay_h)/2".to_string(),
            Self::Bottom => format!("main_h-overlay_h-main_h*{EDGE_MARGIN}"),
        }
    }
}

/// The rendered video of an overlay, as measured by the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayAsset {
    /// Absolute path of the file.
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub frames: i64,
    pub fps: f64,
}

/// What `add_overlay` is asked to put on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayRequest {
    /// Names the overlay: the same id is the same overlay.
    pub id: String,
    /// The validated spec, kept in the project as the caller wrote it.
    pub spec: Value,
    pub start_ms: u64,
    pub end_ms: u64,
    pub position: OverlayPosition,
    pub width_pct: u32,
    pub asset: OverlayAsset,
    /// Who drew it (the worker's version line).
    pub renderer: String,
}

/// An overlay as read back from a project.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub id: String,
    pub spec: Value,
    /// First frame of the span.
    pub start_frame: i64,
    /// One past the last frame of the span.
    pub end_frame: i64,
    pub position: OverlayPosition,
    pub width_pct: u32,
    pub asset: OverlayAsset,
    pub renderer: String,
}

/// The `metadata.reelctl.overlay` object.
#[derive(Serialize, Deserialize)]
struct OverlayMeta {
    id: String,
    spec: Value,
    position: OverlayPosition,
    width_pct: u32,
    key_color: String,
    renderer: String,
    asset: OverlayAsset,
}

/// What `add_overlay` did.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayReport {
    pub start_frame: i64,
    /// One past the last frame.
    pub end_frame: i64,
    /// The project already had this overlay at this span and was left as it was.
    pub unchanged: bool,
}

fn overlay_tracks(timeline: &Timeline) -> Vec<&Track> {
    timeline
        .tracks
        .children
        .iter()
        .filter(|track| track.is_overlay())
        .collect()
}

/// Whether the project has an overlay track.
pub fn has_overlays(timeline: &Timeline) -> bool {
    timeline.tracks.children.iter().any(Track::is_overlay)
}

/// Refuse a project that has overlays: the edits that move the main tracks would
/// leave an overlay at the wrong time.
pub fn refuse_overlays(timeline: &Timeline) -> Result<()> {
    if has_overlays(timeline) {
        return Err(TimelineError::invalid(
            "the project has overlays, and this edit would move them off their span; make this edit first, then add overlays (or remove them with `overlay remove`)",
        ));
    }
    Ok(())
}

/// Every overlay of the project in time order.
pub fn overlays(timeline: &Timeline) -> Result<Vec<Overlay>> {
    let tracks = overlay_tracks(timeline);
    let Some(track) = tracks.first() else {
        return Ok(Vec::new());
    };
    if tracks.len() > 1 {
        return Err(TimelineError::invalid(format!(
            "the project has {} overlay tracks; one is supported",
            tracks.len()
        )));
    }
    let rate = project_rate(timeline)?;
    let mut found: Vec<Overlay> = Vec::new();
    let mut position = 0;
    for item in &track.children {
        let (start, duration) = crate::tracks::item_range(item, rate)?;
        if start != 0 && matches!(item, Item::Clip(_)) {
            return Err(TimelineError::invalid(
                "an overlay clip must start at frame 0 of its video",
            ));
        }
        if let Item::Clip(clip) = item {
            let meta = reelctl(&clip.metadata)
                .and_then(|map| map.get(OVERLAY_KEY))
                .ok_or_else(|| {
                    TimelineError::invalid(format!(
                        "overlay clip \"{}\" has no metadata.reelctl.{OVERLAY_KEY}",
                        clip.name
                    ))
                })?;
            let meta: OverlayMeta = serde_json::from_value(meta.clone()).map_err(|error| {
                TimelineError::invalid(format!(
                    "overlay clip \"{}\": metadata.reelctl.{OVERLAY_KEY}: {error}",
                    clip.name
                ))
            })?;
            found.push(Overlay {
                id: meta.id,
                spec: meta.spec,
                start_frame: position,
                end_frame: position + duration,
                position: meta.position,
                width_pct: meta.width_pct,
                asset: meta.asset,
                renderer: meta.renderer,
            });
        }
        position += duration;
    }
    Ok(found)
}

/// Where an overlay would go: its frames, and whether the project already has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub start_frame: i64,
    pub end_frame: i64,
    pub existing: bool,
}

/// Check that `id` can be put on `start_ms..end_ms` of the timeline, before anything
/// is rendered. The span is rounded to the nearest frame. It must be 500 ms to 30 s,
/// inside the timeline, and free of other overlays; the same id on the same frames
/// is `existing`.
pub fn plan_overlay(timeline: &Timeline, id: &str, start_ms: u64, end_ms: u64) -> Result<Slot> {
    let rate = project_rate(timeline)?;
    if end_ms <= start_ms
        || end_ms - start_ms < MIN_OVERLAY_MS
        || end_ms - start_ms > MAX_OVERLAY_MS
    {
        return Err(TimelineError::invalid(format!(
            "the span {start_ms}..{end_ms} ms must be {MIN_OVERLAY_MS} to {MAX_OVERLAY_MS} ms long"
        )));
    }
    let start_frame = round_frame(start_ms, rate);
    let end_frame = round_frame(end_ms, rate);
    let total = timeline_frames(timeline)?;
    if end_frame > total || end_frame <= start_frame {
        return Err(TimelineError::invalid(format!(
            "the span {start_ms}..{end_ms} ms is not inside the timeline, which is {} ms long",
            frame_ms(total, rate)
        )));
    }
    let mut existing = false;
    for other in overlays(timeline)? {
        let same_frames = other.start_frame == start_frame && other.end_frame == end_frame;
        if other.id == id {
            if !same_frames {
                return Err(TimelineError::invalid(format!(
                    "overlay {id} is already on frames {}..{}",
                    other.start_frame, other.end_frame
                )));
            }
            existing = true;
        } else if other.start_frame < end_frame && start_frame < other.end_frame {
            return Err(TimelineError::invalid(format!(
                "the span {start_ms}..{end_ms} ms overlaps overlay {} (frames {}..{}); overlays may not overlap",
                other.id, other.start_frame, other.end_frame
            )));
        }
    }
    Ok(Slot {
        start_frame,
        end_frame,
        existing,
    })
}

/// Put an overlay on the timeline. The same overlay (same id, same frames) is left as
/// it is and reported `unchanged`.
pub fn add_overlay(timeline: &mut Timeline, request: &OverlayRequest) -> Result<OverlayReport> {
    if !(MIN_WIDTH_PCT..=MAX_WIDTH_PCT).contains(&request.width_pct) {
        return Err(TimelineError::invalid(format!(
            "the overlay width {}% must be {MIN_WIDTH_PCT} to {MAX_WIDTH_PCT}",
            request.width_pct
        )));
    }
    let slot = plan_overlay(timeline, &request.id, request.start_ms, request.end_ms)?;
    if slot.existing {
        return Ok(OverlayReport {
            start_frame: slot.start_frame,
            end_frame: slot.end_frame,
            unchanged: true,
        });
    }
    let mut all = overlays(timeline)?;
    all.push(Overlay {
        id: request.id.clone(),
        spec: request.spec.clone(),
        start_frame: slot.start_frame,
        end_frame: slot.end_frame,
        position: request.position,
        width_pct: request.width_pct,
        asset: request.asset.clone(),
        renderer: request.renderer.clone(),
    });
    set_overlays(timeline, all)?;
    Ok(OverlayReport {
        start_frame: slot.start_frame,
        end_frame: slot.end_frame,
        unchanged: false,
    })
}

/// Take an overlay off the timeline. `false` if there was none with that id. With
/// the last one gone the overlay track goes too, so the project is as it was.
pub fn remove_overlay(timeline: &mut Timeline, id: &str) -> Result<bool> {
    let mut all = overlays(timeline)?;
    let before = all.len();
    all.retain(|overlay| overlay.id != id);
    if all.len() == before {
        return Ok(false);
    }
    set_overlays(timeline, all)?;
    Ok(true)
}

/// Replace the overlay track with one built from `all`.
fn set_overlays(timeline: &mut Timeline, mut all: Vec<Overlay>) -> Result<()> {
    let rate = project_rate(timeline)?;
    all.sort_by_key(|overlay| overlay.start_frame);
    timeline.tracks.children.retain(|track| !track.is_overlay());
    if all.is_empty() {
        return Ok(());
    }
    let mut track = Track::new(OVERLAY_TRACK_NAME, VIDEO_KIND);
    update_reelctl(&mut track.metadata, |map| {
        map.insert(OVERLAYS_MARKER.to_string(), Value::Bool(true));
    });
    let mut cursor = 0;
    for overlay in &all {
        if overlay.start_frame > cursor {
            track.children.push(Item::Gap(Gap::new(TimeRange::new(
                0.0,
                (overlay.start_frame - cursor) as f64,
                rate,
            ))));
        }
        let frames = overlay.end_frame - overlay.start_frame;
        let available = (overlay.asset.frames as f64 * rate / overlay.asset.fps).round();
        let mut reference =
            ExternalReference::new(&overlay.asset.path, TimeRange::new(0.0, available, rate));
        reference.name = format!("overlay {}", overlay.id);
        let mut clip = Clip::new(
            &format!("overlay {}", overlay.id),
            TimeRange::new(0.0, frames as f64, rate),
            MediaReference::External(reference),
        );
        let meta = OverlayMeta {
            id: overlay.id.clone(),
            spec: overlay.spec.clone(),
            position: overlay.position,
            width_pct: overlay.width_pct,
            key_color: KEY_COLOR.to_string(),
            renderer: overlay.renderer.clone(),
            asset: overlay.asset.clone(),
        };
        let meta = serde_json::to_value(meta)?;
        update_reelctl(&mut clip.metadata, |map| {
            map.insert(OVERLAY_KEY.to_string(), meta);
        });
        track.children.push(Item::Clip(clip));
        cursor = overlay.end_frame;
    }
    timeline.tracks.children.push(track);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut::{cut_silence, CutOptions};
    use crate::punch::{punch_in, PunchRequest};
    use crate::testutil::{dna, project};
    use crate::tracks::{check_editable, single_media};
    use serde_json::json;

    fn request(id: &str, start_ms: u64, end_ms: u64) -> OverlayRequest {
        OverlayRequest {
            id: id.to_string(),
            spec: json!({"kind": "bar", "title": id}),
            start_ms,
            end_ms,
            position: OverlayPosition::Center,
            width_pct: 90,
            asset: OverlayAsset {
                path: format!("/assets/{id}.mp4"),
                width: 576,
                height: 432,
                frames: 120,
                fps: 30.0,
            },
            renderer: "manimgx 0.0.0".to_string(),
        }
    }

    /// 10 s at 30 fps.
    fn ten_seconds() -> Timeline {
        project(30.0, 300)
    }

    fn track_shape(timeline: &Timeline) -> Vec<(char, i64)> {
        let rate = project_rate(timeline).expect("rate");
        let track = timeline
            .tracks
            .children
            .iter()
            .find(|track| track.is_overlay())
            .expect("overlay track");
        track
            .children
            .iter()
            .map(|item| {
                let (_, frames) = crate::tracks::item_range(item, rate).expect("range");
                (
                    if matches!(item, Item::Gap(_)) {
                        'g'
                    } else {
                        'c'
                    },
                    frames,
                )
            })
            .collect()
    }

    #[test]
    fn an_overlay_is_a_gap_and_a_clip_on_a_marked_second_video_track() {
        let mut timeline = ten_seconds();
        let report = add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        assert_eq!(
            report,
            OverlayReport {
                start_frame: 60,
                end_frame: 180,
                unchanged: false
            }
        );
        assert_eq!(track_shape(&timeline), vec![('g', 60), ('c', 120)]);
        let track = timeline.tracks.children.last().expect("track");
        assert_eq!(track.kind, VIDEO_KIND);
        assert_eq!(track.name, OVERLAY_TRACK_NAME);
        assert!(track.is_overlay());
        assert_eq!(timeline.tracks.children.len(), 3);
        // The main tracks are still the first video and audio tracks.
        assert!(!timeline.track(VIDEO_KIND).expect("video").is_overlay());
        assert_eq!(check_editable(&timeline).expect("editable"), 30.0);
        assert_eq!(single_media(&timeline).expect("media"), "/media/take.mp4");
        let all = overlays(&timeline).expect("overlays");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "a");
        assert_eq!((all[0].start_frame, all[0].end_frame), (60, 180));
        assert_eq!(all[0].asset.path, "/assets/a.mp4");
        assert_eq!(all[0].width_pct, 90);
        assert_eq!(all[0].spec["title"], "a");
    }

    #[test]
    fn the_name_does_not_make_a_track_an_overlay_track_only_the_marker_does() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        let last = timeline.tracks.children.last_mut().expect("track");
        last.metadata.clear();
        assert!(!last.is_overlay());
        assert!(!has_overlays(&timeline));
    }

    #[test]
    fn the_same_overlay_twice_is_one_overlay_and_the_same_bytes() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        let once = timeline.to_json().expect("json");
        let again = add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("again");
        assert!(again.unchanged);
        assert_eq!(timeline.to_json().expect("json"), once);
        assert_eq!(overlays(&timeline).expect("overlays").len(), 1);
    }

    #[test]
    fn the_same_id_on_other_frames_is_refused() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        let error = add_overlay(&mut timeline, &request("a", 3_000, 7_000)).expect_err("moved");
        assert!(
            error.to_string().contains("already on frames 60..180"),
            "{error}"
        );
    }

    #[test]
    fn overlapping_overlays_are_refused_and_touching_ones_are_not() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        for (start, end) in [
            (1_000, 2_500),
            (5_000, 8_000),
            (3_000, 4_000),
            (1_000, 7_000),
        ] {
            let error = add_overlay(&mut timeline, &request("b", start, end)).expect_err("overlap");
            assert!(error.to_string().contains("overlaps overlay a"), "{error}");
        }
        add_overlay(&mut timeline, &request("b", 6_000, 8_000)).expect("touching after");
        add_overlay(&mut timeline, &request("c", 1_000, 2_000)).expect("touching before");
        assert_eq!(
            track_shape(&timeline),
            vec![('g', 30), ('c', 30), ('c', 120), ('c', 60)]
        );
        let order: Vec<String> = overlays(&timeline)
            .expect("overlays")
            .into_iter()
            .map(|overlay| overlay.id)
            .collect();
        assert_eq!(order, ["c", "a", "b"]);
    }

    #[test]
    fn a_span_is_bounded_in_length_and_inside_the_timeline() {
        let mut timeline = ten_seconds();
        for (start, end, what) in [
            (2_000, 2_499, "499 ms"),
            (6_000, 5_000, "backwards"),
            (2_000, 2_000, "empty"),
            (0, 30_001, "30001 ms"),
            (9_000, 10_500, "past the end"),
        ] {
            let error = add_overlay(&mut timeline, &request("a", start, end)).expect_err(what);
            assert!(!error.to_string().is_empty());
        }
        add_overlay(&mut timeline, &request("a", 2_000, 2_500)).expect("500 ms is the minimum");
        add_overlay(&mut timeline, &request("b", 9_000, 10_000)).expect("ending with the timeline");
        let mut long = project(30.0, 2_000);
        add_overlay(&mut long, &request("c", 0, 30_000)).expect("30 s is the maximum");
        assert_eq!(overlays(&timeline).expect("overlays").len(), 2);
    }

    #[test]
    fn a_width_outside_the_range_is_refused() {
        let mut timeline = ten_seconds();
        for width in [19, 101, 0] {
            let mut bad = request("a", 2_000, 6_000);
            bad.width_pct = width;
            assert!(add_overlay(&mut timeline, &bad).is_err(), "{width}");
        }
        for width in [20, 100] {
            let mut good = request(&format!("w{width}"), 2_000, 6_000);
            good.width_pct = width;
            let mut fresh = ten_seconds();
            add_overlay(&mut fresh, &good).expect("in range");
        }
    }

    #[test]
    fn removing_the_last_overlay_gives_the_project_back_as_it_was() {
        let mut timeline = ten_seconds();
        let before = timeline.to_json().expect("json");
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        add_overlay(&mut timeline, &request("b", 7_000, 9_000)).expect("add");
        assert!(remove_overlay(&mut timeline, "a").expect("remove"));
        assert_eq!(track_shape(&timeline), vec![('g', 210), ('c', 60)]);
        assert!(!remove_overlay(&mut timeline, "a").expect("again"));
        assert!(remove_overlay(&mut timeline, "b").expect("remove"));
        assert_eq!(timeline.to_json().expect("json"), before);
        assert!(!has_overlays(&timeline));
    }

    #[test]
    fn cut_punch_and_bridge_refuse_a_project_with_overlays() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        let gaps = dna(10_000, 30.0, &[(1_000, 1_500)]);
        let error = cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect_err("cut");
        assert!(error.to_string().contains("overlays"), "{error}");
        let punch = PunchRequest {
            start_ms: 2_000,
            end_ms: 4_000,
            scale: 1.18,
        };
        assert!(punch_in(&mut timeline, &punch).is_err());
        let bridge = crate::bridge::BridgeRequest {
            target_url: "example.com".to_string(),
            cutoff_pct: 0.72,
            card_ms: 2_500,
        };
        assert!(crate::bridge::attach_bridge(&mut timeline, &bridge, &[]).is_err());
        assert_eq!(overlays(&timeline).expect("overlays").len(), 1);
    }

    #[test]
    fn a_malformed_overlay_clip_is_an_error_that_names_it() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        let track = timeline.tracks.children.last_mut().expect("track");
        if let Some(Item::Clip(clip)) = track.children.get_mut(1) {
            clip.metadata.clear();
        }
        let error = overlays(&timeline).expect_err("no metadata");
        assert!(error.to_string().contains("overlay a"), "{error}");
    }

    #[test]
    fn positions_name_their_y_expressions() {
        assert_eq!(OverlayPosition::Top.y_expression(), "main_h*0.08");
        assert_eq!(
            OverlayPosition::Center.y_expression(),
            "(main_h-overlay_h)/2"
        );
        assert_eq!(
            OverlayPosition::Bottom.y_expression(),
            "main_h-overlay_h-main_h*0.08"
        );
        assert_eq!(OverlayPosition::default(), OverlayPosition::Center);
        assert_eq!(OverlayPosition::Top.as_str(), "top");
        assert_eq!(OverlayPosition::Center.as_str(), "center");
        assert_eq!(OverlayPosition::Bottom.as_str(), "bottom");
    }

    #[test]
    fn the_span_is_rounded_to_the_nearest_frame() {
        let mut timeline = project(30.0, 300);
        // 2010 ms is 60.3 frames: frame 60; 6020 ms is 180.6: frame 181.
        let report = add_overlay(&mut timeline, &request("a", 2_010, 6_020)).expect("add");
        assert_eq!((report.start_frame, report.end_frame), (60, 181));
    }

    #[test]
    fn the_project_file_round_trips_with_the_overlay_track() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        let text = timeline.to_json().expect("json");
        let again = Timeline::from_json(&text).expect("parse");
        assert_eq!(again.to_json().expect("json"), text);
        assert_eq!(overlays(&again).expect("overlays").len(), 1);
    }

    #[test]
    fn a_project_with_two_overlay_tracks_is_refused() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        let second = timeline.tracks.children.last().expect("track").clone();
        timeline.tracks.children.push(second);
        let error = overlays(&timeline).expect_err("two tracks");
        assert!(error.to_string().contains("2 overlay tracks"), "{error}");
    }

    #[test]
    fn the_longest_span_is_judged_by_its_length_not_by_where_it_sits() {
        // 20 s to 25 s is 5 s long: the sum of the two times is over 30 s, the length
        // is not.
        let mut long = project(30.0, 2_000);
        add_overlay(&mut long, &request("a", 20_000, 25_000)).expect("5 s late in a long video");
        assert!(add_overlay(&mut long, &request("b", 30_000, 60_001)).is_err());
        add_overlay(&mut long, &request("c", 30_000, 60_000)).expect("30 s exactly");
    }

    #[test]
    fn the_same_id_on_the_same_start_but_another_end_is_not_the_same_overlay() {
        let mut timeline = ten_seconds();
        add_overlay(&mut timeline, &request("a", 2_000, 6_000)).expect("add");
        let error = add_overlay(&mut timeline, &request("a", 2_000, 7_000)).expect_err("longer");
        assert!(
            error.to_string().contains("already on frames 60..180"),
            "{error}"
        );
        let error = add_overlay(&mut timeline, &request("a", 1_000, 6_000)).expect_err("earlier");
        assert!(
            error.to_string().contains("already on frames 60..180"),
            "{error}"
        );
    }

    #[test]
    fn the_chart_videos_available_range_is_in_project_frames() {
        // A 96-frame video at 24 fps lasts 4 s: 120 frames of a 30 fps project.
        let mut timeline = ten_seconds();
        let mut asked = request("a", 2_000, 6_000);
        asked.asset.frames = 96;
        asked.asset.fps = 24.0;
        add_overlay(&mut timeline, &asked).expect("add");
        let track = timeline.tracks.children.last().expect("track");
        let Some(Item::Clip(clip)) = track.children.get(1) else {
            panic!("a clip after the gap")
        };
        let Ok(MediaReference::External(reference)) = clip.active_reference() else {
            panic!("an external reference")
        };
        let range = reference.available_range.as_ref().expect("available range");
        assert_eq!(range.duration.value, 120.0);
        assert_eq!(range.duration.rate, 30.0);
        assert_eq!(range.start_time.value, 0.0);
        assert_eq!(reference.target_url, "/assets/a.mp4");
    }
}
