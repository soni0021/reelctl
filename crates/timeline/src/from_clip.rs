//! `timeline from-clip`: the project `init` makes, trimmed to one clip pick from
//! `understanding.json`.

use serde_json::json;

use crate::error::{Result, TimelineError};
use crate::init::{init, InitSpec};
use crate::otio::{reelctl, update_reelctl, Item, TimeRange, Timeline};
use crate::tracks::{ceil_frame, floor_frame, frame_ms};

/// The key under the timeline's `metadata.reelctl` that records the clip.
pub const CLIP_KEY: &str = "clip";
/// The shortest clip, in frames: the same floor `punch-in` puts on a span.
pub const MIN_CLIP_FRAMES: i64 = 12;

/// One clip pick, as `understand` wrote it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipPick {
    /// Absolute path of the `understanding.json`.
    pub understanding_path: String,
    /// The clip's position in `clips`, counting from 1.
    pub index: usize,
    /// Cut points in the media's own time.
    pub start_ms: u64,
    pub end_ms: u64,
    pub hook_id: u32,
    pub last_id: u32,
}

/// The clip recorded in a project, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipRecord {
    pub understanding_path: String,
    pub index: usize,
    pub start_frame: i64,
    pub end_frame: i64,
}

/// Build the project for `spec`'s media with every track trimmed to `pick`. The
/// start snaps down and the end up to whole frames, so no word at an edge is lost.
pub fn from_clip(spec: &InitSpec, pick: &ClipPick) -> Result<Timeline> {
    if pick.index == 0 {
        return Err(TimelineError::invalid("clips are numbered from 1"));
    }
    if pick.end_ms <= pick.start_ms {
        return Err(TimelineError::invalid(format!(
            "clip {} ends at {} ms, not after its start at {} ms",
            pick.index, pick.end_ms, pick.start_ms
        )));
    }
    let mut timeline = init(spec)?;
    let start = floor_frame(pick.start_ms, spec.fps).max(0);
    let end = ceil_frame(pick.end_ms, spec.fps).min(spec.frames);
    if end - start < MIN_CLIP_FRAMES {
        return Err(TimelineError::invalid(format!(
            "clip {} covers {} frames of the video ({}..{} ms); at least {MIN_CLIP_FRAMES} are needed",
            pick.index,
            (end - start).max(0),
            pick.start_ms,
            pick.end_ms
        )));
    }
    let range = TimeRange::new(start as f64, (end - start) as f64, spec.fps);
    for track in &mut timeline.tracks.children {
        for item in &mut track.children {
            if let Item::Clip(_) = item {
                *item.source_range_mut() = Some(range.clone());
            }
        }
    }
    update_reelctl(&mut timeline.metadata, |map| {
        map.insert(
            CLIP_KEY.to_string(),
            json!({
                "understanding": pick.understanding_path,
                "index": pick.index,
                "start_ms": frame_ms(start, spec.fps),
                "end_ms": frame_ms(end, spec.fps),
                "start_frame": start,
                "end_frame": end,
                "hook_id": pick.hook_id,
                "last_id": pick.last_id,
            }),
        );
    });
    Ok(timeline)
}

/// The clip a project was made from, if `from_clip` made it.
pub fn clip_record(timeline: &Timeline) -> Option<ClipRecord> {
    let map = reelctl(&timeline.metadata)?.get(CLIP_KEY)?.as_object()?;
    Some(ClipRecord {
        understanding_path: map.get("understanding")?.as_str()?.to_string(),
        index: usize::try_from(map.get("index")?.as_u64()?).ok()?,
        start_frame: map.get("start_frame")?.as_i64()?,
        end_frame: map.get("end_frame")?.as_i64()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracks::{check_editable, item_range, timeline_frames};
    use crate::{AUDIO_KIND, VIDEO_KIND};

    fn spec() -> InitSpec {
        InitSpec {
            media_path: "/media/lecture.mp4".to_string(),
            fps: 30.0,
            frames: 2700,
            width: 1280,
            height: 720,
            has_audio: true,
            dna: None,
        }
    }

    fn pick(start_ms: u64, end_ms: u64) -> ClipPick {
        ClipPick {
            understanding_path: "/out/understanding.json".to_string(),
            index: 2,
            start_ms,
            end_ms,
            hook_id: 14,
            last_id: 31,
        }
    }

    #[test]
    fn every_track_is_trimmed_to_the_clip_snapped_outward() {
        // 10.010 s is frame 300.3 and 40.020 s is frame 1200.6.
        let timeline = from_clip(&spec(), &pick(10_010, 40_020)).unwrap();
        let rate = check_editable(&timeline).unwrap();
        for kind in [VIDEO_KIND, AUDIO_KIND] {
            let track = timeline.track(kind).unwrap();
            assert_eq!(track.children.len(), 1);
            assert_eq!(item_range(&track.children[0], rate).unwrap(), (300, 901));
        }
        assert_eq!(timeline_frames(&timeline).unwrap(), 901);
    }

    #[test]
    fn the_clip_is_recorded_and_read_back() {
        let timeline = from_clip(&spec(), &pick(10_000, 40_000)).unwrap();
        assert_eq!(
            clip_record(&timeline),
            Some(ClipRecord {
                understanding_path: "/out/understanding.json".to_string(),
                index: 2,
                start_frame: 300,
                end_frame: 1200,
            })
        );
        let map = reelctl(&timeline.metadata).unwrap()[CLIP_KEY].clone();
        assert_eq!(map["hook_id"], 14);
        assert_eq!(map["last_id"], 31);
        assert_eq!(map["start_ms"], 10_000);
        assert_eq!(map["end_ms"], 40_000);
    }

    #[test]
    fn an_init_project_has_no_clip_record() {
        assert_eq!(clip_record(&init(&spec()).unwrap()), None);
    }

    #[test]
    fn the_end_is_clamped_to_the_media() {
        let timeline = from_clip(&spec(), &pick(80_000, 95_000)).unwrap();
        let rate = check_editable(&timeline).unwrap();
        let video = timeline.track(VIDEO_KIND).unwrap();
        assert_eq!(item_range(&video.children[0], rate).unwrap(), (2400, 300));
    }

    #[test]
    fn a_backwards_tiny_or_unnumbered_clip_is_refused() {
        assert!(from_clip(&spec(), &pick(20_000, 20_000))
            .unwrap_err()
            .to_string()
            .contains("not after its start"));
        assert!(from_clip(&spec(), &pick(20_000, 20_300))
            .unwrap_err()
            .to_string()
            .contains("at least 12"));
        assert!(from_clip(&spec(), &pick(95_000, 99_000))
            .unwrap_err()
            .to_string()
            .contains("at least 12"));
        let mut zero = pick(10_000, 20_000);
        zero.index = 0;
        assert!(from_clip(&spec(), &zero)
            .unwrap_err()
            .to_string()
            .contains("numbered from 1"));
    }

    // Edits on a clip that starts after 0. Every edit works in the media's own
    // frames, so a trimmed clip must give the same result as a whole-file project,
    // shifted by the clip's start.

    use crate::testutil::{dna, with_words};
    use crate::{
        attach_bridge, build_render_plan, cut_silence, punch_in, BridgeRequest, CutOptions,
        PunchRequest,
    };

    /// 10 s at 100 fps (a frame is 10 ms), trimmed to 3..8 s.
    fn trimmed() -> Timeline {
        let spec = InitSpec {
            media_path: "/media/take.mp4".to_string(),
            fps: 100.0,
            frames: 1000,
            width: 640,
            height: 360,
            has_audio: true,
            dna: None,
        };
        from_clip(&spec, &pick(3_000, 8_000)).unwrap()
    }

    fn kept(timeline: &Timeline) -> Vec<(i64, i64)> {
        timeline
            .track(VIDEO_KIND)
            .unwrap()
            .children
            .iter()
            .map(|item| item_range(item, 100.0).unwrap())
            .collect()
    }

    #[test]
    fn cut_silence_removes_only_the_gaps_inside_the_clip() {
        let mut timeline = trimmed();
        // Outside before, inside, straddling the end, and outside after.
        let gaps = dna(
            10_000,
            100.0,
            &[
                (1_000, 1_500),
                (4_000, 4_500),
                (7_800, 8_400),
                (9_000, 9_500),
            ],
        );
        let report = cut_silence(&mut timeline, &gaps, &CutOptions::default()).unwrap();
        let spans: Vec<(i64, i64)> = report
            .removed
            .iter()
            .map(|span| (span.start_frame, span.end_frame))
            .collect();
        assert_eq!(spans, vec![(400, 450), (780, 800)]);
        assert_eq!(kept(&timeline), vec![(300, 100), (450, 330)]);
        assert_eq!(timeline_frames(&timeline).unwrap(), 430);
        // The audio follows the video one for one.
        check_editable(&timeline).unwrap();
        // A second run removes nothing more.
        let again = cut_silence(&mut timeline, &gaps, &CutOptions::default()).unwrap();
        assert!(again.removed.is_empty());
    }

    #[test]
    fn filler_words_outside_the_clip_are_left_alone() {
        let mut timeline = trimmed();
        let words = with_words(
            dna(10_000, 100.0, &[]),
            &[
                ("um", 1_000, 1_200),
                ("so", 3_500, 3_700),
                ("uh", 5_000, 5_200),
                ("ok", 5_300, 5_600),
            ],
        );
        let report = cut_silence(&mut timeline, &words, &CutOptions::default()).unwrap();
        let reasons: Vec<(&str, i64)> = report
            .removed
            .iter()
            .map(|span| (span.reason.as_str(), span.start_frame))
            .collect();
        assert_eq!(reasons.len(), 1, "{reasons:?}");
        assert_eq!(reasons[0].0, "filler");
        assert!((470..=500).contains(&reasons[0].1), "{reasons:?}");
    }

    #[test]
    fn punch_in_is_in_timeline_time_whatever_the_clip_start() {
        let mut timeline = trimmed();
        let report = punch_in(
            &mut timeline,
            &PunchRequest {
                start_ms: 1_000,
                end_ms: 2_000,
                scale: 1.2,
            },
        )
        .unwrap();
        assert_eq!((report.start_frame, report.end_frame), (100, 200));
        // The clip is split at timeline frames 100 and 200, which are media frames
        // 400 and 500.
        assert_eq!(kept(&timeline), vec![(300, 100), (400, 100), (500, 300)]);
        check_editable(&timeline).unwrap();
    }

    #[test]
    fn the_bridge_cuts_at_a_fraction_of_the_clip_not_of_the_file() {
        let mut timeline = trimmed();
        let report = attach_bridge(
            &mut timeline,
            &BridgeRequest {
                target_url: "https://example.com/full".to_string(),
                cutoff_pct: 0.5,
                card_ms: 1_000,
            },
            &[],
        )
        .unwrap();
        assert_eq!(report.spec.cut_frame, 250);
        // 250 frames of the clip, from media frame 300, then the card.
        assert_eq!(kept(&timeline)[0], (300, 250));
        assert_eq!(timeline_frames(&timeline).unwrap(), 350);
    }

    #[test]
    fn the_render_trims_from_the_clip_start() {
        let plan = build_render_plan(&trimmed(), None).unwrap();
        assert!(
            plan.filter_graph
                .contains("trim=start_frame=300:end_frame=800"),
            "{}",
            plan.filter_graph
        );
        // 3..8 s of 48 kHz audio.
        assert!(
            plan.filter_graph
                .contains("atrim=start_sample=144000:end_sample=384000"),
            "{}",
            plan.filter_graph
        );
    }
}
