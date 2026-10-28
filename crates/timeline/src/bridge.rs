//! `funnel attach-bridge`: cut the timeline at a tension point and end on a card
//! that shows where to go next.

use serde_json::{json, Map, Value};

use crate::error::{Result, TimelineError};
use crate::otio::{
    reelctl, update_reelctl, Clip, GeneratorReference, Item, MediaReference, TimeRange, Timeline,
    Track, VIDEO_KIND,
};
use crate::tracks::{
    ceil_frame, check_editable, external_url, floor_frame, frame_ms, item_range, split_at,
    timeline_frames, DEFAULT_FADE_MS, FADE_KEY,
};

/// `generator_kind` of the card clip on the video track.
pub const CARD_KIND: &str = "reelctl.card";
/// `generator_kind` of the matching silent clip on the audio track.
pub const SILENCE_KIND: &str = "reelctl.silence";
/// Card colours as `#rrggbb`: none of the synthetic fixtures contain this violet.
pub const CARD_BACKGROUND: &str = "#3a1c71";
pub const CARD_FOREGROUND: &str = "#ffffff";
/// The key under the timeline's `metadata.reelctl` that records the bridge.
pub const BRIDGE_KEY: &str = "bridge";
/// A pause is looked for this long after the cutoff point.
pub const PAUSE_SEARCH_MS: f64 = 400.0;

/// Arguments of `attach-bridge`.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeRequest {
    pub target_url: String,
    /// Where to cut, as a fraction of the timeline's length.
    pub cutoff_pct: f64,
    pub card_ms: u64,
}

/// A bridge as recorded in the project.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeSpec {
    pub target_url: String,
    pub cutoff_pct: f64,
    pub card_ms: u64,
    /// Timeline frame of the cut.
    pub cut_frame: i64,
}

/// What an `attach-bridge` run did.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeReport {
    pub spec: BridgeSpec,
    /// The cut was moved into a pause (or already fell in one).
    pub in_pause: bool,
    /// The project already had this bridge, and was left as it was.
    pub unchanged: bool,
}

/// The bridge recorded in the project, if any.
pub fn bridge_of(timeline: &Timeline) -> Option<BridgeSpec> {
    let map = reelctl(&timeline.metadata)?.get(BRIDGE_KEY)?.as_object()?;
    Some(BridgeSpec {
        target_url: map.get("url")?.as_str()?.to_string(),
        cutoff_pct: map.get("cutoff_pct")?.as_f64()?,
        card_ms: map.get("card_ms")?.as_u64()?,
        cut_frame: map.get("cut_frame")?.as_i64()?,
    })
}

/// The timeline frame that plays source `frame` of some clip, if one does.
fn source_to_timeline(video: &Track, frame: i64, rate: f64) -> Result<Option<i64>> {
    let mut position = 0;
    for item in &video.children {
        let (start, duration) = item_range(item, rate)?;
        if let Item::Clip(clip) = item {
            if external_url(clip).is_some() && start <= frame && frame < start + duration {
                return Ok(Some(position + frame - start));
            }
        }
        position += duration;
    }
    Ok(None)
}

/// The source frame played at timeline `frame`.
fn timeline_to_source(video: &Track, frame: i64, rate: f64) -> Result<Option<i64>> {
    let mut position = 0;
    for item in &video.children {
        let (start, duration) = item_range(item, rate)?;
        if let Item::Clip(clip) = item {
            if external_url(clip).is_some() && position <= frame && frame < position + duration {
                return Ok(Some(start + frame - position));
            }
        }
        position += duration;
    }
    Ok(None)
}

/// Where to cut: the cutoff point, or the start of a pause within
/// [`PAUSE_SEARCH_MS`] after it, or the point itself if it is already inside one.
/// `pauses` are `(start_ms, end_ms)` in the media's own time.
fn place_cut(
    video: &Track,
    point: i64,
    total: i64,
    pauses: &[(u64, u64)],
    rate: f64,
) -> Result<(i64, bool)> {
    if let Some(source) = timeline_to_source(video, point, rate)? {
        let inside = pauses.iter().any(|&(start, end)| {
            ceil_frame(start, rate) <= source && source < floor_frame(end, rate)
        });
        if inside {
            return Ok((point, true));
        }
    }
    let mut best: Option<i64> = None;
    for &(start, _) in pauses {
        let Some(at) = source_to_timeline(video, ceil_frame(start, rate), rate)? else {
            continue;
        };
        let after_ms = (at - point) as f64 * 1000.0 / rate;
        if at >= point && at < total && after_ms <= PAUSE_SEARCH_MS + 1e-6 {
            best = Some(best.map_or(at, |current| current.min(at)));
        }
    }
    Ok(match best {
        Some(at) => (at, true),
        None => (point, false),
    })
}

fn card(kind: &str, name: &str, frames: i64, rate: f64, parameters: Map<String, Value>) -> Item {
    let range = TimeRange::new(0.0, frames as f64, rate);
    let reference = GeneratorReference::new(kind, parameters, range.clone());
    Item::Clip(Clip::new(name, range, MediaReference::Generator(reference)))
}

/// Cut the timeline at `cutoff_pct` of its length (see [`place_cut`]), drop what
/// follows from the output (the media is not touched), and end on a card of
/// `card_ms` that shows `target_url`. `pauses` are the silence gaps of the media's
/// edit DNA, `(start_ms, end_ms)`; give none to cut at the point.
pub fn attach_bridge(
    timeline: &mut Timeline,
    request: &BridgeRequest,
    pauses: &[(u64, u64)],
) -> Result<BridgeReport> {
    let rate = check_editable(timeline)?;
    if request.target_url.trim().is_empty() {
        return Err(TimelineError::invalid("--target-url is empty"));
    }
    if !request.cutoff_pct.is_finite() || request.cutoff_pct <= 0.0 || request.cutoff_pct >= 1.0 {
        return Err(TimelineError::invalid(format!(
            "--cutoff-pct {} must be above 0 and below 1",
            request.cutoff_pct
        )));
    }
    if request.card_ms == 0 {
        return Err(TimelineError::invalid("--card-ms must be above 0"));
    }
    if let Some(existing) = bridge_of(timeline) {
        if existing.target_url == request.target_url
            && existing.card_ms == request.card_ms
            && existing.cutoff_pct == request.cutoff_pct
        {
            return Ok(BridgeReport {
                spec: existing,
                in_pause: false,
                unchanged: true,
            });
        }
        return Err(TimelineError::invalid(format!(
            "the project already ends on a bridge to \"{}\" ({} ms card, cutoff {}); make a new project with `reelctl timeline init` to change it",
            existing.target_url, existing.card_ms, existing.cutoff_pct
        )));
    }

    let total = timeline_frames(timeline)?;
    let point = floor_frame_of(total, request.cutoff_pct);
    if point <= 0 || point >= total {
        return Err(TimelineError::invalid(format!(
            "the timeline is {total} frames; {} of it is not a place to cut",
            request.cutoff_pct
        )));
    }
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let (cut, in_pause) = place_cut(video, point, total, pauses, rate)?;

    let card_frames = ((request.card_ms as f64) * rate / 1000.0).round().max(1.0) as i64;
    let mut work = timeline.clone();
    for track in &mut work.tracks.children {
        split_at(track, cut, rate)?;
        let mut position = 0;
        let mut keep = 0;
        for item in &track.children {
            if position >= cut {
                break;
            }
            position += item_range(item, rate)?.1;
            keep += 1;
        }
        track.children.truncate(keep);
        // The footage ends in a cut: fade the audio out unless a fade is set.
        if let Some(Item::Clip(last)) = track.children.last_mut() {
            let plays_media = external_url(last).is_some();
            let has_fade = reelctl(&last.metadata)
                .and_then(|map| map.get(FADE_KEY))
                .is_some();
            if plays_media && !has_fade {
                update_reelctl(&mut last.metadata, |map| {
                    map.insert(FADE_KEY.to_string(), json!(DEFAULT_FADE_MS));
                });
            }
        }
        let is_video = track.kind == VIDEO_KIND;
        let mut parameters = Map::new();
        let item = if is_video {
            parameters.insert("text".to_string(), json!(request.target_url));
            parameters.insert("background".to_string(), json!(CARD_BACKGROUND));
            parameters.insert("foreground".to_string(), json!(CARD_FOREGROUND));
            card(CARD_KIND, "bridge card", card_frames, rate, parameters)
        } else {
            card(
                SILENCE_KIND,
                "bridge card (silence)",
                card_frames,
                rate,
                parameters,
            )
        };
        let mut item = item;
        update_reelctl(item.metadata_mut(), |map| {
            map.insert(
                BRIDGE_KEY.to_string(),
                json!({ "url": request.target_url, "card_ms": request.card_ms }),
            );
        });
        track.children.push(item);
    }
    update_reelctl(&mut work.metadata, |map| {
        map.insert(
            BRIDGE_KEY.to_string(),
            json!({
                "url": request.target_url,
                "cutoff_pct": request.cutoff_pct,
                "card_ms": request.card_ms,
                "cut_frame": cut,
                "cut_ms": frame_ms(cut, rate),
                "frames_before": total,
                "in_pause": in_pause,
            }),
        );
    });
    check_editable(&work)?;
    *timeline = work;
    Ok(BridgeReport {
        spec: BridgeSpec {
            target_url: request.target_url.clone(),
            cutoff_pct: request.cutoff_pct,
            card_ms: request.card_ms,
            cut_frame: cut,
        },
        in_pause,
        unchanged: false,
    })
}

/// `floor(total * pct)`, robust to the product landing a hair below a whole number.
fn floor_frame_of(total: i64, pct: f64) -> i64 {
    (total as f64 * pct + 1e-9).floor() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut::{cut_silence, CutOptions};
    use crate::otio::AUDIO_KIND;
    use crate::testutil::{dna, project};

    fn request(url: &str) -> BridgeRequest {
        BridgeRequest {
            target_url: url.to_string(),
            cutoff_pct: 0.72,
            card_ms: 2_500,
        }
    }

    fn ranges(timeline: &Timeline, kind: &str) -> Vec<(i64, i64)> {
        timeline
            .track(kind)
            .expect("track")
            .children
            .iter()
            .map(|item| item_range(item, 100.0).expect("range"))
            .collect()
    }

    #[test]
    fn the_cut_is_at_72_percent_and_a_card_follows() {
        let mut timeline = project(100.0, 1000);
        let report = attach_bridge(&mut timeline, &request("example.com/go"), &[]).expect("bridge");
        assert_eq!(report.spec.cut_frame, 720);
        assert!(!report.in_pause && !report.unchanged);
        assert_eq!(ranges(&timeline, VIDEO_KIND), vec![(0, 720), (0, 250)]);
        assert_eq!(ranges(&timeline, AUDIO_KIND), vec![(0, 720), (0, 250)]);
        assert_eq!(timeline_frames(&timeline).expect("frames"), 970);
    }

    #[test]
    fn the_clip_before_the_card_gets_an_audio_fade_unless_it_has_one() {
        let mut timeline = project(100.0, 1000);
        attach_bridge(&mut timeline, &request("a.co"), &[]).expect("bridge");
        for kind in [VIDEO_KIND, AUDIO_KIND] {
            let track = timeline.track(kind).expect("track");
            let Item::Clip(last) = &track.children[0] else {
                panic!("clip expected")
            };
            let fade = reelctl(&last.metadata).and_then(|map| map.get("fade_ms"));
            assert_eq!(fade.and_then(|v| v.as_u64()), Some(30), "{kind}");
        }
        let mut timeline = project(100.0, 1000);
        let gaps = dna(10_000, 100.0, &[(2_000, 2_500)]);
        let options = CutOptions {
            fade_ms: 50,
            ..CutOptions::default()
        };
        cut_silence(&mut timeline, &gaps, &options).expect("cut");
        attach_bridge(&mut timeline, &request("a.co"), &[]).expect("bridge");
        let video = timeline.track(VIDEO_KIND).expect("video");
        let Item::Clip(last) = &video.children[video.children.len() - 2] else {
            panic!("clip expected")
        };
        let fade = reelctl(&last.metadata).and_then(|map| map.get("fade_ms"));
        assert_eq!(fade.and_then(|v| v.as_u64()), Some(50));
    }

    #[test]
    fn the_percentage_is_the_argument() {
        let mut timeline = project(100.0, 1000);
        let mut half = request("example.com");
        half.cutoff_pct = 0.5;
        let report = attach_bridge(&mut timeline, &half, &[]).expect("bridge");
        assert_eq!(report.spec.cut_frame, 500);
    }

    #[test]
    fn the_card_is_a_generator_that_carries_the_url() {
        let mut timeline = project(100.0, 1000);
        attach_bridge(&mut timeline, &request("example.com/go"), &[]).expect("bridge");
        let video = timeline.track(VIDEO_KIND).expect("video");
        let Item::Clip(card) = &video.children[1] else {
            panic!("clip expected")
        };
        let MediaReference::Generator(generator) = card.active_reference().expect("ref") else {
            panic!("generator expected")
        };
        assert_eq!(generator.generator_kind, CARD_KIND);
        assert_eq!(generator.parameters["text"], "example.com/go");
        let audio = timeline.track(AUDIO_KIND).expect("audio");
        let Item::Clip(silence) = &audio.children[1] else {
            panic!("clip expected")
        };
        let MediaReference::Generator(generator) = silence.active_reference().expect("ref") else {
            panic!("generator expected")
        };
        assert_eq!(generator.generator_kind, SILENCE_KIND);
        let spec = bridge_of(&timeline).expect("recorded");
        assert_eq!(spec.target_url, "example.com/go");
        assert_eq!(spec.card_ms, 2_500);
    }

    #[test]
    fn a_pause_within_400_ms_after_the_point_takes_the_cut() {
        // The point is frame 720 = 7200 ms. 7600 ms is exactly 400 ms later.
        let mut timeline = project(100.0, 1000);
        let report =
            attach_bridge(&mut timeline, &request("a.co"), &[(7_600, 8_000)]).expect("bridge");
        assert_eq!(report.spec.cut_frame, 760);
        assert!(report.in_pause);
    }

    #[test]
    fn a_pause_more_than_400_ms_after_the_point_is_ignored() {
        let mut timeline = project(100.0, 1000);
        let report =
            attach_bridge(&mut timeline, &request("a.co"), &[(7_610, 8_000)]).expect("bridge");
        assert_eq!(report.spec.cut_frame, 720);
        assert!(!report.in_pause);
    }

    #[test]
    fn a_pause_before_the_point_is_ignored() {
        let mut timeline = project(100.0, 1000);
        let report =
            attach_bridge(&mut timeline, &request("a.co"), &[(6_000, 6_500)]).expect("bridge");
        assert_eq!(report.spec.cut_frame, 720);
    }

    #[test]
    fn a_point_inside_a_pause_stays_where_it_is() {
        let mut timeline = project(100.0, 1000);
        let report =
            attach_bridge(&mut timeline, &request("a.co"), &[(7_000, 7_500)]).expect("bridge");
        assert_eq!(report.spec.cut_frame, 720);
        assert!(report.in_pause);
    }

    #[test]
    fn the_nearest_of_several_pauses_wins() {
        let mut timeline = project(100.0, 1000);
        let pauses = [(7_500, 7_900), (7_300, 7_400)];
        let report = attach_bridge(&mut timeline, &request("a.co"), &pauses).expect("bridge");
        assert_eq!(report.spec.cut_frame, 730);
    }

    #[test]
    fn pauses_are_found_through_earlier_cuts() {
        // 100 frames are cut out at media 2000..3000 ms, so the timeline is 900
        // frames and the point is 648. Media time is the timeline's plus 100
        // frames after the cut: a pause at media 7500 ms (frame 750) plays at 650.
        let mut timeline = project(100.0, 1000);
        let gaps = dna(10_000, 100.0, &[(2_000, 3_000)]);
        cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
        let report =
            attach_bridge(&mut timeline, &request("a.co"), &[(7_500, 7_900)]).expect("bridge");
        assert_eq!(report.spec.cut_frame, 650);
    }

    #[test]
    fn a_second_run_with_the_same_arguments_changes_nothing() {
        let mut timeline = project(100.0, 1000);
        attach_bridge(&mut timeline, &request("a.co"), &[]).expect("bridge");
        let once = timeline.to_json().expect("json");
        let again = attach_bridge(&mut timeline, &request("a.co"), &[]).expect("again");
        assert!(again.unchanged);
        assert_eq!(timeline.to_json().expect("json"), once);
    }

    #[test]
    fn a_different_bridge_on_a_bridged_project_is_refused() {
        let mut timeline = project(100.0, 1000);
        attach_bridge(&mut timeline, &request("a.co"), &[]).expect("bridge");
        let before = timeline.to_json().expect("json");
        let error = attach_bridge(&mut timeline, &request("b.co"), &[]).expect_err("second");
        assert!(error.to_string().contains("a.co"), "{error}");
        assert_eq!(timeline.to_json().expect("json"), before);
    }

    #[test]
    fn bad_arguments_are_refused() {
        for percent in [0.0, 1.0, 1.5, -0.1, f64::NAN] {
            let mut timeline = project(100.0, 1000);
            let mut bad = request("a.co");
            bad.cutoff_pct = percent;
            let error = attach_bridge(&mut timeline, &bad, &[]).expect_err("pct");
            assert!(error.to_string().contains("--cutoff-pct"), "{error}");
        }
        let mut timeline = project(100.0, 1000);
        let mut bad = request("a.co");
        bad.card_ms = 0;
        assert!(attach_bridge(&mut timeline, &bad, &[]).is_err());
        assert!(attach_bridge(&mut timeline, &request("  "), &[]).is_err());
    }
}
