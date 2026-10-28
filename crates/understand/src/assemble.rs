//! The model's reply and the measured facts, put together as `understanding.json`.

use edit_dna::Cut;

use crate::backend::VlmReply;
use crate::clips::ClipOutcome;
use crate::types::{
    Beat, CutLabel, ModelRun, RawBeat, RunSettings, Segment, Source, Summary, Understanding,
    VisualEvent, VlmLabel, SCHEMA_VERSION,
};
use crate::{FRAME_CUT_OFFSET_MS, FRAME_MERGE_MS};

/// Everything `assemble` needs. All of it is measured or already checked.
#[derive(Debug, Clone)]
pub struct AssembleInput<'a> {
    pub source_path: &'a str,
    pub duration_ms: u64,
    pub model_id: &'a str,
    pub segments: &'a [Segment],
    /// `reelctl`'s cuts, for `vlm_label`.
    pub cuts: &'a [Cut],
    /// The times of the frames sent, ascending.
    pub frame_times_ms: &'a [u64],
    pub reply: &'a VlmReply,
    pub clips: ClipOutcome,
    pub settings: RunSettings,
    pub cost_upper_bound_usd: f64,
}

/// Map the reply to `understanding.json`: beats get their times from their segments,
/// each event's `t_s` snaps to the nearest sampled frame time (`seen_at_ms`), each cut
/// gets the `vlm_label` of the event seen in the frame at its time plus or minus
/// [`crate::FRAME_CUT_OFFSET_MS`], and every quote is copied from `segments`.
///
/// Where the contract leaves a choice, this is what happens:
/// - A beat is dropped when either of its segment IDs is not in `segments`, when
///   `start_segment` is after `end_segment`, or when the start time it gets is after
///   the end time it gets. The model supplies no time and no quote, so nothing is
///   repaired. The other beats keep the model's order.
/// - When `segments` repeats an ID, the first one is used.
/// - An event's `t_s` snaps to the nearest frame time; a tie goes to the earlier frame,
///   whatever order `frame_times_ms` is in. A time before the first frame or after the
///   last snaps to it, and so does an infinite one. An event with a `NaN` time, or with
///   no frame times to snap to, is dropped.
/// - `visual_events` is sorted by `seen_at_ms`; events at the same frame keep the
///   model's order, so "first in the model's order" is the first of them.
/// - A cut's frame is the nearest frame to its target (cut time plus the offset, then
///   minus it), kept only if it lies within [`crate::FRAME_MERGE_MS`] of the target
///   (100 ms counts). A frame without an event does not stop the search: the
///   after-frame is tried first, and the before-frame is used when it has no label.
///   The before-target is not clamped at 0, so a cut near the start cannot match a
///   frame that is further away than the window.
/// - Every cut gets an entry, in the order given.
pub fn assemble(input: AssembleInput<'_>) -> Understanding {
    let AssembleInput {
        source_path,
        duration_ms,
        model_id,
        segments,
        cuts,
        frame_times_ms,
        reply,
        clips,
        settings,
        cost_upper_bound_usd,
    } = input;
    let raw = &reply.reply;
    let summary = &raw.video_summary;

    let beats = raw
        .beats
        .iter()
        .filter_map(|beat| place_beat(beat, segments))
        .collect();

    let mut visual_events: Vec<VisualEvent> = raw
        .edit_events
        .iter()
        .filter_map(|event| {
            Some(VisualEvent {
                seen_at_ms: nearest_frame(frame_times_ms, event.t_s * 1_000.0)?,
                kind: event.kind,
                note: event.note.clone(),
            })
        })
        .collect();
    // A stable sort, so events at one frame stay in the model's order.
    visual_events.sort_by_key(|event| event.seen_at_ms);

    let cuts = cuts
        .iter()
        .map(|cut| CutLabel {
            frame_index: cut.frame_index,
            time_ms: cut.time_ms,
            vlm_label: cut_label(cut, frame_times_ms, &visual_events),
        })
        .collect();

    Understanding {
        schema_version: SCHEMA_VERSION,
        source: Source {
            path: source_path.to_string(),
            duration_ms,
        },
        model: ModelRun {
            id: model_id.to_string(),
            provider: reply.provider.clone(),
            quantization: reply.quantization.clone(),
            input_tokens: reply.input_tokens,
            output_tokens: reply.output_tokens,
            cost_usd: reply.cost_usd,
            cost_upper_bound_usd,
            elapsed_ms: reply.elapsed_ms,
        },
        settings,
        segments: segments.to_vec(),
        summary: Summary {
            topic: summary.topic.clone(),
            language: summary.language.clone(),
            speaker_count: summary.speaker_count,
            visual_formats: summary.visual_formats.clone(),
        },
        beats,
        visual_events,
        cuts,
        clips: clips.clips,
        rejected: clips.rejected,
    }
}

/// The beat with its times taken from its segments, or `None` if it cannot be placed.
fn place_beat(beat: &RawBeat, segments: &[Segment]) -> Option<Beat> {
    if beat.start_segment > beat.end_segment {
        return None;
    }
    let start = segments.iter().find(|s| s.id == beat.start_segment)?;
    let end = segments.iter().find(|s| s.id == beat.end_segment)?;
    if start.start_ms > end.end_ms {
        return None;
    }
    Some(Beat {
        start_segment: beat.start_segment,
        end_segment: beat.end_segment,
        start_ms: start.start_ms,
        end_ms: end.end_ms,
        kind: beat.kind.clone(),
        summary: beat.summary.clone(),
        visual: beat.visual.clone(),
        insight_density: beat.insight_density,
    })
}

/// The sampled frame time nearest to `target_ms`, the earlier one on a tie. `None`
/// when there are no frames or the target is `NaN`.
fn nearest_frame(frames: &[u64], target_ms: f64) -> Option<u64> {
    if target_ms.is_nan() {
        return None;
    }
    let first = frames.iter().copied().min()? as f64;
    let last = frames.iter().copied().max()? as f64;
    // Clamping keeps an infinite target at the end it points to.
    let target = target_ms.clamp(first, last);
    let distance = |frame: u64| (frame as f64 - target).abs();
    frames
        .iter()
        .copied()
        .min_by(|&a, &b| distance(a).total_cmp(&distance(b)).then(a.cmp(&b)))
}

/// The label of the first event seen in the frame after the cut, else in the frame before.
fn cut_label(cut: &Cut, frames: &[u64], events: &[VisualEvent]) -> Option<VlmLabel> {
    let time = cut.time_ms as f64;
    let offset = FRAME_CUT_OFFSET_MS as f64;
    [time + offset, time - offset]
        .into_iter()
        .find_map(|target| {
            let frame = nearest_frame(frames, target)?;
            if (frame as f64 - target).abs() > FRAME_MERGE_MS as f64 {
                return None;
            }
            let event = events.iter().find(|event| event.seen_at_ms == frame)?;
            Some(VlmLabel {
                kind: event.kind,
                note: event.note.clone(),
            })
        })
}

#[cfg(test)]
mod tests {
    use edit_dna::SilenceGap;

    use super::*;
    use crate::types::{
        Clip, CutLabel, RawBeat, RawReply, RawSummary, RawVisualEvent, Rejected, SegmentRef,
        VisualEvent, VisualEventKind, VlmLabel,
    };
    use crate::{FRAME_CUT_OFFSET_MS, FRAME_MERGE_MS};

    fn seg(id: u32, start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            id,
            start_ms,
            end_ms,
            text: text.to_string(),
            ends_sentence: text.ends_with('.'),
        }
    }

    fn five_segments() -> Vec<Segment> {
        vec![
            seg(1, 0, 2_000, "Hello there."),
            seg(2, 2_500, 4_000, "Today we talk about"),
            seg(3, 4_500, 6_000, "the heart."),
            seg(4, 7_000, 9_000, "It has four chambers."),
            seg(5, 9_500, 11_000, "That is all."),
        ]
    }

    fn cut(frame_index: u64, time_ms: u64) -> Cut {
        Cut {
            frame_index,
            time_ms,
            interval_ms: 1_000,
            scene_score: 0.5,
        }
    }

    fn beat(start_segment: u32, end_segment: u32) -> RawBeat {
        RawBeat {
            start_segment,
            end_segment,
            kind: "explanation".to_string(),
            summary: format!("beat {start_segment}-{end_segment}"),
            visual: "a slide".to_string(),
            insight_density: 3,
        }
    }

    fn event(t_s: f64, kind: VisualEventKind, note: &str) -> RawVisualEvent {
        RawVisualEvent {
            t_s,
            kind,
            note: note.to_string(),
        }
    }

    fn empty_raw_reply() -> RawReply {
        RawReply {
            video_summary: RawSummary {
                topic: "the heart".to_string(),
                language: "en".to_string(),
                speaker_count: 1,
                visual_formats: vec!["slides".to_string()],
            },
            beats: Vec::new(),
            edit_events: Vec::new(),
            reel_candidates: Vec::new(),
        }
    }

    fn vlm_reply(raw: RawReply) -> VlmReply {
        VlmReply {
            reply: raw,
            provider: "Alibaba".to_string(),
            quantization: Some("fp8".to_string()),
            input_tokens: 41_000,
            output_tokens: 2_100,
            cost_usd: Some(0.0123),
            elapsed_ms: 38_500,
        }
    }

    fn settings() -> RunSettings {
        RunSettings {
            every_s: 5,
            sheet: "2x2".to_string(),
            segment_pause_ms: 400,
            quant: vec!["fp8".to_string(), "bf16".to_string()],
        }
    }

    /// Owns everything an `AssembleInput` borrows, so a test can change one part.
    struct Case {
        segments: Vec<Segment>,
        cuts: Vec<Cut>,
        frames: Vec<u64>,
        reply: VlmReply,
        clips: ClipOutcome,
    }

    impl Case {
        fn new() -> Self {
            Self {
                segments: five_segments(),
                cuts: Vec::new(),
                frames: vec![0, 5_000, 10_000],
                reply: vlm_reply(empty_raw_reply()),
                clips: ClipOutcome::default(),
            }
        }

        fn with_events(mut self, events: Vec<RawVisualEvent>) -> Self {
            self.reply.reply.edit_events = events;
            self
        }

        fn with_beats(mut self, beats: Vec<RawBeat>) -> Self {
            self.reply.reply.beats = beats;
            self
        }

        fn with_frames(mut self, frames: &[u64]) -> Self {
            self.frames = frames.to_vec();
            self
        }

        fn with_cuts(mut self, cuts: Vec<Cut>) -> Self {
            self.cuts = cuts;
            self
        }

        fn run(&self) -> Understanding {
            assemble(AssembleInput {
                source_path: "lecture.mp4",
                duration_ms: 1_716_000,
                model_id: "qwen/qwen3.5-9b",
                segments: &self.segments,
                cuts: &self.cuts,
                frame_times_ms: &self.frames,
                reply: &self.reply,
                clips: self.clips.clone(),
                settings: settings(),
                cost_upper_bound_usd: 0.31,
            })
        }
    }

    fn seen_at(u: &Understanding) -> Vec<u64> {
        u.visual_events.iter().map(|e| e.seen_at_ms).collect()
    }

    /// The snapped time of a single event reported at `t_s`, if it is kept. The case
    /// also carries a beat, so a run that returns nothing at all cannot pass.
    fn snap(frames: &[u64], t_s: f64) -> Vec<u64> {
        let case = Case::new()
            .with_frames(frames)
            .with_beats(vec![beat(1, 1)])
            .with_events(vec![event(t_s, VisualEventKind::HardCut, "x")]);
        let u = case.run();
        assert_eq!(u.beats.len(), 1);
        seen_at(&u)
    }

    #[test]
    fn beat_times_come_from_their_segments() {
        let u = Case::new().with_beats(vec![beat(2, 4)]).run();
        assert_eq!(u.beats.len(), 1);
        let b = &u.beats[0];
        assert_eq!((b.start_segment, b.end_segment), (2, 4));
        assert_eq!(b.start_ms, 2_500, "start of the start segment");
        assert_eq!(b.end_ms, 9_000, "end of the end segment");
        assert_eq!(b.summary, "beat 2-4");
        assert_eq!(b.insight_density, 3);
    }

    #[test]
    fn a_beat_on_a_single_segment_spans_that_segment() {
        let u = Case::new().with_beats(vec![beat(3, 3)]).run();
        assert_eq!((u.beats[0].start_ms, u.beats[0].end_ms), (4_500, 6_000));
    }

    #[test]
    fn a_beat_with_a_missing_segment_id_is_dropped() {
        let u = Case::new()
            .with_beats(vec![
                beat(1, 2),
                beat(9, 9),
                beat(3, 9),
                beat(0, 3),
                beat(4, 5),
            ])
            .run();
        let ids: Vec<(u32, u32)> = u
            .beats
            .iter()
            .map(|b| (b.start_segment, b.end_segment))
            .collect();
        assert_eq!(ids, vec![(1, 2), (4, 5)], "the rest keep the model's order");
    }

    #[test]
    fn a_beat_that_starts_after_it_ends_is_dropped() {
        let u = Case::new().with_beats(vec![beat(4, 2), beat(2, 4)]).run();
        assert_eq!(u.beats.len(), 1);
        assert_eq!(u.beats[0].start_segment, 2);
    }

    #[test]
    fn a_beat_is_dropped_when_the_segment_times_run_backwards() {
        let mut case = Case::new().with_beats(vec![beat(1, 2), beat(2, 3)]);
        // Segment 2 sits before segment 1 in time, so beat 1-2 starts after it ends.
        case.segments = vec![
            seg(1, 5_000, 6_000, "late."),
            seg(2, 1_000, 2_000, "early."),
            seg(3, 7_000, 8_000, "last."),
        ];
        let u = case.run();
        assert_eq!(u.beats.len(), 1);
        assert_eq!((u.beats[0].start_ms, u.beats[0].end_ms), (1_000, 8_000));
    }

    #[test]
    fn an_event_snaps_to_the_nearest_frame() {
        let frames = [1_000, 2_000, 3_000];
        assert_eq!(snap(&frames, 1.4), vec![1_000]);
        assert_eq!(snap(&frames, 1.6), vec![2_000]);
        assert_eq!(snap(&frames, 2.9), vec![3_000]);
    }

    #[test]
    fn a_tie_goes_to_the_earlier_frame() {
        let frames = [1_000, 2_000, 3_000];
        assert_eq!(snap(&frames, 1.5), vec![1_000]);
        assert_eq!(snap(&frames, 2.5), vec![2_000]);
    }

    #[test]
    fn a_tie_goes_to_the_earlier_frame_whatever_the_frame_order() {
        assert_eq!(snap(&[3_000, 1_000, 2_000], 1.5), vec![1_000]);
    }

    #[test]
    fn an_event_on_a_frame_time_keeps_it() {
        let frames = [1_000, 2_000, 3_000];
        assert_eq!(snap(&frames, 2.0), vec![2_000]);
        assert_eq!(snap(&frames, 1.0), vec![1_000]);
        assert_eq!(snap(&frames, 3.0), vec![3_000]);
    }

    #[test]
    fn an_event_before_the_first_frame_snaps_to_it() {
        let frames = [1_000, 2_000, 3_000];
        assert_eq!(snap(&frames, 0.2), vec![1_000]);
        assert_eq!(snap(&frames, -4.0), vec![1_000]);
    }

    #[test]
    fn an_event_after_the_last_frame_snaps_to_it() {
        let frames = [1_000, 2_000, 3_000];
        assert_eq!(snap(&frames, 9.0), vec![3_000]);
        assert_eq!(snap(&frames, f64::INFINITY), vec![3_000]);
    }

    #[test]
    fn an_event_with_no_finite_time_is_dropped() {
        let frames = [1_000, 2_000];
        assert_eq!(snap(&frames, f64::NAN), Vec::<u64>::new());
    }

    #[test]
    fn with_no_frames_the_events_are_dropped() {
        let case = Case::new()
            .with_frames(&[])
            .with_beats(vec![beat(1, 2)])
            .with_events(vec![event(3.0, VisualEventKind::HardCut, "x")])
            .with_cuts(vec![cut(7, 3_000)]);
        let u = case.run();
        assert!(u.visual_events.is_empty());
        assert_eq!(u.beats.len(), 1, "beats do not need frames");
        assert_eq!(u.cuts.len(), 1, "every cut keeps its entry");
        assert_eq!(u.cuts[0].vlm_label, None);
    }

    #[test]
    fn events_are_sorted_by_seen_time_and_keep_the_model_order_within_a_frame() {
        let u = Case::new()
            .with_frames(&[0, 5_000, 10_000])
            .with_events(vec![
                event(9.8, VisualEventKind::SlideChange, "late"),
                event(5.1, VisualEventKind::PunchIn, "first at five"),
                event(0.2, VisualEventKind::HardCut, "early"),
                event(4.9, VisualEventKind::ZoomOut, "second at five"),
            ])
            .run();
        let notes: Vec<&str> = u.visual_events.iter().map(|e| e.note.as_str()).collect();
        assert_eq!(
            notes,
            vec!["early", "first at five", "second at five", "late"]
        );
        assert_eq!(seen_at(&u), vec![0, 5_000, 5_000, 10_000]);
    }

    /// Frames every 5 s plus the two around a cut at 613.2 s (300 ms either side).
    fn lecture_frames() -> Vec<u64> {
        let mut frames: Vec<u64> = (0..=124).map(|i| i * 5_000).collect();
        frames.extend([612_900, 613_500]);
        frames.sort_unstable();
        frames
    }

    fn lecture_case(close: Option<f64>) -> Case {
        let mut events = vec![event(
            604.0,
            VisualEventKind::OverlayOn,
            "a pop-up opens over the slide",
        )];
        if let Some(t_s) = close {
            events.push(event(t_s, VisualEventKind::OverlayOff, "the pop-up closes"));
        }
        Case::new()
            .with_frames(&lecture_frames())
            .with_events(events)
            .with_cuts(vec![cut(15_330, 613_200)])
    }

    fn close_label() -> Option<VlmLabel> {
        Some(VlmLabel {
            kind: VisualEventKind::OverlayOff,
            note: "the pop-up closes".to_string(),
        })
    }

    #[test]
    fn a_cut_gets_the_event_seen_in_the_frame_after_it() {
        let u = lecture_case(Some(613.5)).run();
        assert_eq!(seen_at(&u), vec![605_000, 613_500]);
        assert_eq!(u.visual_events[0].kind, VisualEventKind::OverlayOn);
        assert_eq!(
            u.cuts,
            vec![CutLabel {
                frame_index: 15_330,
                time_ms: 613_200,
                vlm_label: close_label(),
            }]
        );
    }

    #[test]
    fn a_cut_gets_the_event_seen_in_the_frame_before_it() {
        let u = lecture_case(Some(612.9)).run();
        assert_eq!(seen_at(&u), vec![605_000, 612_900]);
        assert_eq!(u.cuts[0].vlm_label, close_label());
    }

    #[test]
    fn a_cut_with_no_event_in_its_frames_gets_null() {
        let u = lecture_case(None).run();
        assert_eq!(seen_at(&u), vec![605_000]);
        assert_eq!(u.cuts.len(), 1);
        assert_eq!(u.cuts[0].vlm_label, None);
        let json = serde_json::to_value(&u).expect("serializes");
        assert!(json["cuts"][0]["vlm_label"].is_null());
    }

    #[test]
    fn an_event_seen_at_a_nearby_five_second_frame_does_not_label_the_cut() {
        // 615 s is a sampled frame but 1.8 s from the cut's after-frame.
        let u = lecture_case(Some(615.0)).run();
        assert_eq!(seen_at(&u), vec![605_000, 615_000]);
        assert_eq!(u.cuts[0].vlm_label, None);
    }

    #[test]
    fn the_after_frame_is_checked_before_the_before_frame() {
        let case = lecture_case(None).with_events(vec![
            event(612.9, VisualEventKind::OverlayOn, "before"),
            event(613.5, VisualEventKind::OverlayOff, "after"),
        ]);
        let u = case.run();
        let label = u.cuts[0].vlm_label.clone().expect("a label");
        assert_eq!(label.note, "after");
        assert_eq!(label.kind, VisualEventKind::OverlayOff);
    }

    #[test]
    fn the_before_frame_is_used_when_the_after_frame_has_no_event() {
        let case = lecture_case(None).with_events(vec![
            event(612.9, VisualEventKind::OverlayOn, "before"),
            event(620.0, VisualEventKind::HardCut, "elsewhere"),
        ]);
        let u = case.run();
        assert_eq!(
            u.cuts[0].vlm_label,
            Some(VlmLabel {
                kind: VisualEventKind::OverlayOn,
                note: "before".to_string()
            })
        );
    }

    #[test]
    fn events_sharing_the_frame_give_the_one_first_in_the_models_order() {
        let case = lecture_case(None).with_events(vec![
            event(613.4, VisualEventKind::CaptionChange, "model's first"),
            event(613.6, VisualEventKind::OverlayOff, "model's second"),
        ]);
        let u = case.run();
        assert_eq!(seen_at(&u), vec![613_500, 613_500]);
        assert_eq!(
            u.cuts[0].vlm_label.clone().map(|l| l.note),
            Some("model's first".to_string())
        );
    }

    /// One cut at `cut_ms`, one sampled frame at `frame_ms`, one event seen at it.
    fn window_label(cut_ms: u64, frame_ms: u64) -> Option<VlmLabel> {
        let case = Case::new()
            .with_frames(&[frame_ms])
            .with_events(vec![event(
                frame_ms as f64 / 1_000.0,
                VisualEventKind::SlideChange,
                "seen",
            )])
            .with_cuts(vec![cut(1, cut_ms)]);
        let u = case.run();
        assert_eq!(seen_at(&u), vec![frame_ms]);
        u.cuts[0].vlm_label.clone()
    }

    #[test]
    fn a_frame_is_accepted_within_the_merge_window_of_the_target() {
        let cut_ms = 50_000;
        let after = cut_ms + FRAME_CUT_OFFSET_MS;
        let before = cut_ms - FRAME_CUT_OFFSET_MS;
        for frame in [after, after + FRAME_MERGE_MS, after - FRAME_MERGE_MS] {
            assert!(window_label(cut_ms, frame).is_some(), "after-side {frame}");
        }
        for frame in [before, before + FRAME_MERGE_MS, before - FRAME_MERGE_MS] {
            assert!(window_label(cut_ms, frame).is_some(), "before-side {frame}");
        }
    }

    #[test]
    fn a_frame_beyond_the_merge_window_of_the_target_is_not_accepted() {
        let cut_ms = 50_000;
        let after = cut_ms + FRAME_CUT_OFFSET_MS;
        let before = cut_ms - FRAME_CUT_OFFSET_MS;
        for frame in [
            after + FRAME_MERGE_MS + 1,
            after - FRAME_MERGE_MS - 1,
            before + FRAME_MERGE_MS + 1,
            before - FRAME_MERGE_MS - 1,
        ] {
            assert_eq!(window_label(cut_ms, frame), None, "frame {frame}");
        }
    }

    #[test]
    fn a_frame_on_the_cut_itself_is_not_the_cuts_frame() {
        assert_eq!(window_label(50_000, 50_000), None);
    }

    #[test]
    fn a_cut_near_the_start_does_not_wrap_its_before_target_to_zero() {
        // Before-target is -200 ms; a frame at 0 is 200 ms away, not 0.
        assert_eq!(window_label(100, 0), None);
        assert!(window_label(200, 0).is_some(), "-100 ms is 100 ms from 0");
    }

    #[test]
    fn every_cut_gets_an_entry_in_the_input_order() {
        let case = lecture_case(Some(613.5)).with_cuts(vec![
            cut(900, 700_000),
            cut(15_330, 613_200),
            cut(5, 1_000),
        ]);
        let u = case.run();
        let times: Vec<(u64, u64)> = u.cuts.iter().map(|c| (c.frame_index, c.time_ms)).collect();
        assert_eq!(times, vec![(900, 700_000), (15_330, 613_200), (5, 1_000)]);
        assert_eq!(u.cuts[0].vlm_label, None);
        assert_eq!(u.cuts[1].vlm_label, close_label());
    }

    #[test]
    fn the_run_details_pass_through() {
        let mut case = Case::new();
        case.reply.reply.video_summary = RawSummary {
            topic: "NEET biology".to_string(),
            language: "hinglish".to_string(),
            speaker_count: 2,
            visual_formats: vec!["slides".to_string(), "talking head".to_string()],
        };
        let u = case.run();
        assert_eq!(u.schema_version, crate::types::SCHEMA_VERSION);
        assert_eq!(u.source.path, "lecture.mp4");
        assert_eq!(u.source.duration_ms, 1_716_000);
        assert_eq!(u.model.id, "qwen/qwen3.5-9b");
        assert_eq!(u.model.provider, "Alibaba");
        assert_eq!(u.model.quantization.as_deref(), Some("fp8"));
        assert_eq!(u.model.input_tokens, 41_000);
        assert_eq!(u.model.output_tokens, 2_100);
        assert_eq!(u.model.cost_usd, Some(0.0123));
        assert_eq!(u.model.cost_upper_bound_usd, 0.31);
        assert_eq!(u.model.elapsed_ms, 38_500);
        assert_eq!(u.settings, settings());
        assert_eq!(u.segments, five_segments());
        assert_eq!(u.summary.topic, "NEET biology");
        assert_eq!(u.summary.language, "hinglish");
        assert_eq!(u.summary.speaker_count, 2);
        assert_eq!(u.summary.visual_formats, vec!["slides", "talking head"]);
    }

    #[test]
    fn an_unreported_cost_and_quantization_stay_none() {
        let mut case = Case::new();
        case.reply.cost_usd = None;
        case.reply.quantization = None;
        let u = case.run();
        assert_eq!(u.model.id, "qwen/qwen3.5-9b");
        assert_eq!(u.model.cost_usd, None);
        assert_eq!(u.model.quantization, None);
        assert_eq!(u.model.cost_upper_bound_usd, 0.31);
    }

    fn sample_clip(rank: u32, hook: u32, payoff: u32, last: u32, segs: &[Segment]) -> Clip {
        let text_of = |id: u32| SegmentRef {
            id,
            text: segs
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.text.clone())
                .unwrap_or_default(),
        };
        Clip {
            rank,
            title: format!("clip {rank}"),
            start_ms: 2_400,
            end_ms: 9_150,
            length_ms: 6_750,
            tight_length_ms: 6_000,
            dead_air: vec![SilenceGap {
                start_ms: 6_000,
                end_ms: 7_000,
                duration_ms: 1_000,
            }],
            hook: text_of(hook),
            payoff: text_of(payoff),
            last: text_of(last),
            payoff_at_ms: 3_000,
            open_loop: "what comes next".to_string(),
            visual_suggestion: "zoom on the diagram".to_string(),
            why: "strong open loop".to_string(),
            adjustments: vec!["dropped segment 1".to_string()],
            flags: vec!["tight length 6000 ms is under 30000 ms".to_string()],
        }
    }

    fn case_with_clips() -> Case {
        let mut case = Case::new();
        case.clips = ClipOutcome {
            clips: vec![
                sample_clip(1, 2, 4, 5, &case.segments),
                sample_clip(3, 3, 4, 4, &case.segments),
            ],
            rejected: vec![Rejected {
                rank: 2,
                title: "bad pick".to_string(),
                reason: "segment 99 is out of range".to_string(),
            }],
        };
        case
    }

    #[test]
    fn clips_and_rejected_pass_through_unchanged() {
        let case = case_with_clips();
        let u = case.run();
        assert_eq!(u.clips.len(), 2);
        assert_eq!(u.clips, case.clips.clips);
        assert_eq!(u.rejected, case.clips.rejected);
    }

    #[test]
    fn every_quoted_text_equals_its_segment_text() {
        let u = case_with_clips().run();
        assert!(!u.clips.is_empty());
        for clip in &u.clips {
            for quote in [&clip.hook, &clip.payoff, &clip.last] {
                let segment = u
                    .segments
                    .iter()
                    .find(|s| s.id == quote.id)
                    .expect("the quoted segment exists");
                assert_eq!(quote.text, segment.text);
            }
        }
    }

    #[test]
    fn the_output_survives_a_serde_round_trip() {
        let mut case = lecture_case(Some(613.5))
            .with_beats(vec![beat(1, 3), beat(2, 5)])
            .with_cuts(vec![cut(15_330, 613_200), cut(3, 40)]);
        case.clips = case_with_clips().clips;
        let u = case.run();
        assert!(!u.beats.is_empty() && !u.visual_events.is_empty() && !u.clips.is_empty());
        let json = serde_json::to_string_pretty(&u).expect("serializes");
        let back: Understanding = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, u);
    }

    #[test]
    fn an_empty_reply_gives_an_empty_but_valid_understanding() {
        let mut case = Case::new().with_frames(&[]);
        case.segments = Vec::new();
        let u = case.run();
        assert_eq!(u.schema_version, crate::types::SCHEMA_VERSION);
        assert_eq!(u.source.path, "lecture.mp4");
        assert_eq!(u.model.id, "qwen/qwen3.5-9b");
        assert!(u.segments.is_empty());
        assert!(u.beats.is_empty());
        assert!(u.visual_events.is_empty());
        assert!(u.cuts.is_empty());
        assert!(u.clips.is_empty());
        assert!(u.rejected.is_empty());
        let json = serde_json::to_string(&u).expect("serializes");
        let back: Understanding = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, u);
    }

    #[test]
    fn visual_event_fields_other_than_time_are_the_models() {
        let u = Case::new()
            .with_events(vec![event(5.0, VisualEventKind::BRollOn, "a stock clip")])
            .run();
        assert_eq!(
            u.visual_events,
            vec![VisualEvent {
                seen_at_ms: 5_000,
                kind: VisualEventKind::BRollOn,
                note: "a stock clip".to_string()
            }]
        );
    }
}
