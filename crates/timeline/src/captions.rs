//! `overlay captions`: word captions from whisper's timings, placed on the edited
//! timeline and grouped, with a style. Pure: the style, the placement, the groups,
//! the project metadata, and which caption picture `render` shows on each frame.
//! The pictures themselves are drawn by `reelctl` (this ffmpeg build has no
//! `drawtext`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, TimelineError};
use crate::otio::{reelctl, update_reelctl, Item, Timeline, VIDEO_KIND};
use crate::tracks::{check_editable, external_url, item_range, round_frame};

/// The key under the timeline's `metadata.reelctl` that holds the captions.
pub const CAPTIONS_KEY: &str = "captions";

/// Where the captions sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptionPosition {
    Top,
    Middle,
    Bottom,
}

/// How the words are cased.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptionCase {
    AsIs,
    Upper,
    Lower,
}

/// A caption style, resolved: every field set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptionStyle {
    pub preset: String,
    /// The font file, or `None` for the system default `reelctl` looks for.
    pub font: Option<String>,
    /// Text height as a percent of the output height.
    pub font_size: f64,
    pub color: String,
    /// The spoken word's colour, or `None` for no highlight.
    pub highlight_color: Option<String>,
    pub outline_color: String,
    /// Outline width in pixels at 1080 px wide (scaled with the output).
    pub outline_width: u32,
    /// Shadow offset in pixels at 1080 px wide.
    pub shadow: u32,
    pub box_color: String,
    /// 0 is no box.
    pub box_opacity: f64,
    pub position: CaptionPosition,
    /// Distance from the `position` edge as a percent of the output height.
    pub margin: f64,
    pub words_per_group: u32,
    /// A group longer than this wraps to two lines.
    pub max_chars: u32,
    pub case: CaptionCase,
    /// A pause at least this long always starts a new group.
    pub min_gap_ms: u64,
}

/// Fields to change in a style: from a style file or from flags. Unknown fields in
/// a file are refused.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleOverrides {
    pub preset: Option<String>,
    pub font: Option<String>,
    pub font_size: Option<f64>,
    pub color: Option<String>,
    /// `"none"` turns the highlight off.
    pub highlight_color: Option<String>,
    pub outline_color: Option<String>,
    pub outline_width: Option<u32>,
    pub shadow: Option<u32>,
    pub box_color: Option<String>,
    pub box_opacity: Option<f64>,
    pub position: Option<CaptionPosition>,
    pub margin: Option<f64>,
    pub words_per_group: Option<u32>,
    pub max_chars: Option<u32>,
    pub case: Option<CaptionCase>,
    pub min_gap_ms: Option<u64>,
}

/// The preset names, in the order `--list-presets` prints them.
pub const PRESETS: [&str; 5] = ["bold", "clean", "boxed", "karaoke", "minimal"];

/// A preset's style.
pub fn preset(name: &str) -> Result<CaptionStyle> {
    let bold = CaptionStyle {
        preset: "bold".to_string(),
        font: None,
        font_size: 5.5,
        color: "#ffffff".to_string(),
        highlight_color: Some("#ffd60a".to_string()),
        outline_color: "#000000".to_string(),
        outline_width: 6,
        shadow: 0,
        box_color: "#000000".to_string(),
        box_opacity: 0.0,
        position: CaptionPosition::Bottom,
        margin: 22.0,
        words_per_group: 3,
        max_chars: 18,
        case: CaptionCase::Upper,
        min_gap_ms: 400,
    };
    let style = match name {
        "bold" => bold,
        "clean" => CaptionStyle {
            preset: name.to_string(),
            highlight_color: None,
            outline_width: 2,
            case: CaptionCase::AsIs,
            ..bold
        },
        "boxed" => CaptionStyle {
            preset: name.to_string(),
            highlight_color: None,
            outline_width: 0,
            box_opacity: 0.6,
            case: CaptionCase::AsIs,
            ..bold
        },
        "karaoke" => CaptionStyle {
            preset: name.to_string(),
            font_size: 8.0,
            words_per_group: 1,
            max_chars: 14,
            ..bold
        },
        "minimal" => CaptionStyle {
            preset: name.to_string(),
            font_size: 3.5,
            highlight_color: None,
            outline_width: 2,
            margin: 10.0,
            case: CaptionCase::AsIs,
            ..bold
        },
        other => {
            return Err(TimelineError::invalid(format!(
                "--preset \"{other}\" is not one of {}",
                PRESETS.join(", ")
            )))
        }
    };
    Ok(style)
}

fn check_color(field: &str, value: &str) -> Result<()> {
    let hex = value.strip_prefix('#').unwrap_or("");
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(TimelineError::invalid(format!(
            "caption style {field}: \"{value}\" is not a #rrggbb colour"
        )))
    }
}

fn check_range<T: PartialOrd + std::fmt::Display>(
    field: &str,
    value: T,
    low: T,
    high: T,
) -> Result<()> {
    if value >= low && value <= high {
        Ok(())
    } else {
        Err(TimelineError::invalid(format!(
            "caption style {field}: {value} is outside {low} to {high}"
        )))
    }
}

impl CaptionStyle {
    /// Every field in range, or an error that names the field.
    pub fn check(&self) -> Result<()> {
        if !PRESETS.contains(&self.preset.as_str()) {
            return Err(TimelineError::invalid(format!(
                "caption style preset: \"{}\" is not one of {}",
                self.preset,
                PRESETS.join(", ")
            )));
        }
        if !self.font_size.is_finite() {
            return Err(TimelineError::invalid(
                "caption style font_size is not a number",
            ));
        }
        check_range("font_size", self.font_size, 2.0, 12.0)?;
        check_color("color", &self.color)?;
        if let Some(highlight) = &self.highlight_color {
            check_color("highlight_color", highlight)?;
        }
        check_color("outline_color", &self.outline_color)?;
        check_range("outline_width", self.outline_width, 0, 12)?;
        check_range("shadow", self.shadow, 0, 12)?;
        check_color("box_color", &self.box_color)?;
        if !self.box_opacity.is_finite() || !self.margin.is_finite() {
            return Err(TimelineError::invalid(
                "caption style box_opacity and margin must be numbers",
            ));
        }
        check_range("box_opacity", self.box_opacity, 0.0, 1.0)?;
        check_range("margin", self.margin, 0.0, 40.0)?;
        check_range("words_per_group", self.words_per_group, 1, 5)?;
        check_range("max_chars", self.max_chars, 8, 40)?;
        check_range("min_gap_ms", self.min_gap_ms, 100, 2000)?;
        if let Some(font) = &self.font {
            if font.trim().is_empty() {
                return Err(TimelineError::invalid("caption style font is empty"));
            }
        }
        Ok(())
    }

    /// `self` with `overrides` applied (a preset in them starts over from that
    /// preset), checked.
    pub fn with(self, overrides: &StyleOverrides) -> Result<Self> {
        let mut style = match &overrides.preset {
            Some(name) => preset(name)?,
            None => self,
        };
        macro_rules! take {
            ($($field:ident),*) => {$(
                if let Some(value) = &overrides.$field {
                    style.$field = value.clone();
                }
            )*};
        }
        take!(
            font_size,
            color,
            outline_color,
            outline_width,
            shadow,
            box_color,
            box_opacity,
            position,
            margin,
            words_per_group,
            max_chars,
            case,
            min_gap_ms
        );
        if let Some(font) = &overrides.font {
            style.font = Some(font.clone());
        }
        if let Some(highlight) = &overrides.highlight_color {
            style.highlight_color = (highlight != "none").then(|| highlight.clone());
        }
        style.check()?;
        Ok(style)
    }
}

/// A word placed on the edited timeline, in timeline frames `start..end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedWord {
    pub text: String,
    pub start: i64,
    pub end: i64,
}

/// One caption: a few words shown together from the first one's start to the last
/// one's end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptionGroup {
    pub start: i64,
    pub end: i64,
    pub words: Vec<PlacedWord>,
}

impl CaptionGroup {
    /// The group's text in one or two lines (wrapped at `max_chars`), cased.
    pub fn lines(&self, style: &CaptionStyle) -> Vec<Vec<String>> {
        let words: Vec<String> = self
            .words
            .iter()
            .map(|word| cased(&word.text, style.case))
            .collect();
        let total: usize = words.iter().map(|word| word.chars().count()).sum::<usize>()
            + words.len().saturating_sub(1);
        if total <= style.max_chars as usize || words.len() < 2 {
            return vec![words];
        }
        // Split where the two lines are closest in length.
        let mut best = (usize::MAX, 1);
        for split in 1..words.len() {
            let left: usize = words[..split].iter().map(|w| w.chars().count() + 1).sum();
            let right: usize = words[split..].iter().map(|w| w.chars().count() + 1).sum();
            let difference = left.abs_diff(right);
            if difference < best.0 {
                best = (difference, split);
            }
        }
        vec![words[..best.1].to_vec(), words[best.1..].to_vec()]
    }
}

fn cased(text: &str, case: CaptionCase) -> String {
    match case {
        CaptionCase::AsIs => text.to_string(),
        CaptionCase::Upper => text.to_uppercase(),
        CaptionCase::Lower => text.to_lowercase(),
    }
}

/// The words of `words` (in the media's milliseconds) placed on the edited
/// timeline. A word that an edit cut in two is kept on the side with most of its
/// length; a word that is not in any footage (cut out, or after the bridge) is
/// dropped. Returns the placed words in timeline order and how many were dropped.
pub fn place_words(
    timeline: &Timeline,
    words: &[edit_dna::Word],
) -> Result<(Vec<PlacedWord>, usize)> {
    let rate = check_editable(timeline)?;
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let mut pieces = Vec::new();
    let mut position = 0;
    for item in &video.children {
        let (start, duration) = item_range(item, rate)?;
        if matches!(item, Item::Clip(clip) if external_url(clip).is_some()) {
            pieces.push((position, start, duration));
        }
        position += duration;
    }
    let mut placed = Vec::new();
    let mut dropped = 0;
    for word in words {
        let (from, to) = (
            round_frame(word.start_ms, rate),
            round_frame(word.end_ms, rate).max(round_frame(word.start_ms, rate) + 1),
        );
        let best = pieces
            .iter()
            .map(|&(at, start, duration)| {
                let overlap = (to.min(start + duration) - from.max(start)).max(0);
                (overlap, at, start, duration)
            })
            .filter(|(overlap, ..)| *overlap > 0)
            .max_by_key(|(overlap, ..)| *overlap);
        match best {
            Some((_, at, start, duration)) => placed.push(PlacedWord {
                text: word.text.trim().to_string(),
                start: at + from.max(start) - start,
                end: at + to.min(start + duration) - start,
            }),
            None => dropped += 1,
        }
    }
    placed.retain(|word| !word.text.is_empty());
    placed.sort_by_key(|word| (word.start, word.end));
    Ok((placed, dropped))
}

/// Timeline frames where the picture jumps (a join where the footage is not
/// continuous, or a card): a caption group never spans one.
fn hard_cuts(timeline: &Timeline) -> Result<Vec<i64>> {
    Ok(crate::joins(timeline)?
        .into_iter()
        .filter(|join| match (&join.before, &join.after) {
            (Some((a, last)), Some((b, first))) => a != b || *first != last + 1,
            _ => true,
        })
        .map(|join| join.frame)
        .collect())
}

/// Group `words` (timeline order) by `style`: at most `words_per_group` words,
/// never across a hard cut or a pause of `min_gap_ms` or more.
pub fn group_words(
    words: &[PlacedWord],
    cuts: &[i64],
    rate: f64,
    style: &CaptionStyle,
) -> Vec<CaptionGroup> {
    let gap = round_frame(style.min_gap_ms, rate);
    let mut groups: Vec<CaptionGroup> = Vec::new();
    for word in words {
        let start_new = match groups.last() {
            None => true,
            Some(group) => {
                let last = group.words.last().map_or(group.end, |word| word.end);
                group.words.len() >= style.words_per_group as usize
                    || word.start - last >= gap
                    || cuts
                        .iter()
                        .any(|&cut| group.start < cut && cut <= word.start)
            }
        };
        if start_new {
            groups.push(CaptionGroup {
                start: word.start,
                end: word.end,
                words: vec![word.clone()],
            });
        } else if let Some(group) = groups.last_mut() {
            group.end = group.end.max(word.end);
            group.words.push(word.clone());
        }
    }
    // A group never runs into the next one.
    for index in 1..groups.len() {
        let next = groups[index].start;
        let previous = &mut groups[index - 1];
        previous.end = previous.end.min(next);
    }
    groups
}

/// The captions recorded in a project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Captions {
    pub style: CaptionStyle,
    pub groups: Vec<CaptionGroup>,
    /// Words that were not placed (cut out or after the bridge).
    pub dropped: usize,
}

/// What `add_captions` did.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptionsReport {
    pub groups: usize,
    pub words: usize,
    pub dropped: usize,
    pub unchanged: bool,
}

/// Place and group the words and record them with the style, replacing any earlier
/// captions. The style is checked first.
pub fn add_captions(
    timeline: &mut Timeline,
    words: &[edit_dna::Word],
    style: &CaptionStyle,
) -> Result<CaptionsReport> {
    style.check()?;
    let rate = check_editable(timeline)?;
    let (placed, dropped) = place_words(timeline, words)?;
    if placed.is_empty() {
        return Err(TimelineError::invalid(
            "no word of the DNA is in the project's footage; there is nothing to caption",
        ));
    }
    let groups = group_words(&placed, &hard_cuts(timeline)?, rate, style);
    let captions = Captions {
        style: style.clone(),
        groups,
        dropped,
    };
    let unchanged = captions_of(timeline)?.as_ref() == Some(&captions);
    let value = serde_json::to_value(&captions)?;
    update_reelctl(&mut timeline.metadata, |map| {
        map.insert(CAPTIONS_KEY.to_string(), value);
    });
    Ok(CaptionsReport {
        groups: captions.groups.len(),
        words: placed.len(),
        dropped,
        unchanged,
    })
}

/// The captions recorded in the project, if any. A record that does not parse is
/// an error: rendering without it would drop the captions without a word.
pub fn captions_of(timeline: &Timeline) -> Result<Option<Captions>> {
    let Some(value) = reelctl(&timeline.metadata).and_then(|map| map.get(CAPTIONS_KEY)) else {
        return Ok(None);
    };
    let captions: Captions = serde_json::from_value(value.clone()).map_err(|error| {
        TimelineError::invalid(format!(
            "metadata.reelctl.{CAPTIONS_KEY} is not captions this tool wrote: {error}"
        ))
    })?;
    captions.style.check()?;
    Ok(Some(captions))
}

pub fn has_captions(timeline: &Timeline) -> bool {
    reelctl(&timeline.metadata).is_some_and(|map| map.contains_key(CAPTIONS_KEY))
}

/// An edit that moves or removes frames refuses a captioned project: the captions'
/// frames would no longer match.
pub fn refuse_captions(timeline: &Timeline) -> Result<()> {
    if has_captions(timeline) {
        return Err(TimelineError::invalid(
            "the project has captions; add captions last (or take them out with `reelctl overlay remove --captions`)",
        ));
    }
    Ok(())
}

/// Take the captions out. Returns whether there were any.
pub fn remove_captions(timeline: &mut Timeline) -> bool {
    let had = has_captions(timeline);
    if let Some(Value::Object(map)) = timeline.metadata.get_mut(crate::REELCTL_KEY) {
        map.remove(CAPTIONS_KEY);
    }
    had
}

/// One caption picture: a group, with one of its words highlighted (or none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptionState {
    pub group: usize,
    pub highlight: Option<usize>,
}

/// The pictures `render` needs and which one each timeline frame shows (`None`:
/// no caption). With a highlight, a group has one picture per word: the word from
/// its start to the next word's start (the first from the group's start).
pub fn caption_frames(captions: &Captions, frames: i64) -> (Vec<CaptionState>, Vec<Option<usize>>) {
    let mut states = Vec::new();
    let mut shown: Vec<Option<usize>> = vec![None; usize::try_from(frames.max(0)).unwrap_or(0)];
    let paint = |from: i64, to: i64, state: usize, shown: &mut Vec<Option<usize>>| {
        for frame in from.max(0)..to.min(frames) {
            shown[frame as usize] = Some(state);
        }
    };
    for (index, group) in captions.groups.iter().enumerate() {
        if captions.style.highlight_color.is_some() {
            for (k, word) in group.words.iter().enumerate() {
                let from = if k == 0 { group.start } else { word.start };
                let to = group.words.get(k + 1).map_or(group.end, |next| next.start);
                states.push(CaptionState {
                    group: index,
                    highlight: Some(k),
                });
                paint(from, to, states.len() - 1, &mut shown);
            }
        } else {
            states.push(CaptionState {
                group: index,
                highlight: None,
            });
            paint(group.start, group.end, states.len() - 1, &mut shown);
        }
    }
    (states, shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{dna, project, with_words};
    use crate::{append_bridge, cut_silence, CutOptions};

    fn words(list: &[(&str, u64, u64)]) -> Vec<edit_dna::Word> {
        with_words(dna(10_000, 100.0, &[]), list).words
    }

    #[test]
    fn presets_resolve_and_overrides_apply_in_order() {
        let bold = preset("bold").unwrap();
        bold.check().unwrap();
        for name in PRESETS {
            preset(name).unwrap().check().unwrap();
        }
        let file: StyleOverrides =
            serde_json::from_str(r##"{"color": "#ff0000", "position": "top", "case": "as-is"}"##)
                .unwrap();
        let flags = StyleOverrides {
            color: Some("#00ff00".to_string()),
            highlight_color: Some("none".to_string()),
            ..StyleOverrides::default()
        };
        let style = bold.with(&file).unwrap().with(&flags).unwrap();
        assert_eq!(style.color, "#00ff00");
        assert_eq!(style.position, CaptionPosition::Top);
        assert_eq!(style.case, CaptionCase::AsIs);
        assert_eq!(style.highlight_color, None);
        // A preset in the overrides starts from that preset.
        let boxed = preset("bold")
            .unwrap()
            .with(&StyleOverrides {
                preset: Some("boxed".to_string()),
                ..StyleOverrides::default()
            })
            .unwrap();
        assert_eq!(boxed.box_opacity, 0.6);
    }

    #[test]
    fn a_bad_field_names_itself() {
        let unknown: std::result::Result<StyleOverrides, _> =
            serde_json::from_str(r##"{"colour": "#ffffff"}"##);
        assert!(unknown.unwrap_err().to_string().contains("colour"));
        let cases = [
            (
                StyleOverrides {
                    font_size: Some(20.0),
                    ..Default::default()
                },
                "font_size",
            ),
            (
                StyleOverrides {
                    color: Some("red".into()),
                    ..Default::default()
                },
                "color",
            ),
            (
                StyleOverrides {
                    words_per_group: Some(0),
                    ..Default::default()
                },
                "words_per_group",
            ),
            (
                StyleOverrides {
                    box_opacity: Some(1.5),
                    ..Default::default()
                },
                "box_opacity",
            ),
            (
                StyleOverrides {
                    preset: Some("loud".into()),
                    ..Default::default()
                },
                "loud",
            ),
        ];
        for (overrides, field) in cases {
            let error = preset("bold")
                .unwrap()
                .with(&overrides)
                .unwrap_err()
                .to_string();
            assert!(error.contains(field), "{error}");
        }
    }

    #[test]
    fn words_follow_the_edits_and_a_cut_word_keeps_its_longer_side() {
        // 10 s at 100 fps; a silence 3..5 s is cut out.
        let mut timeline = project(100.0, 1000);
        cut_silence(
            &mut timeline,
            &dna(10_000, 100.0, &[(3_000, 5_000)]),
            &CutOptions::default(),
        )
        .unwrap();
        let list = words(&[
            ("one", 1_000, 1_400),
            // Mostly before the cut: kept before it, ending at the join.
            ("two", 2_800, 3_100),
            // Wholly inside the cut: dropped.
            ("gone", 3_500, 3_900),
            // After the cut: moved 2 s earlier.
            ("three", 6_000, 6_400),
        ]);
        let (placed, dropped) = place_words(&timeline, &list).unwrap();
        assert_eq!(dropped, 1);
        let spans: Vec<(&str, i64, i64)> = placed
            .iter()
            .map(|w| (w.text.as_str(), w.start, w.end))
            .collect();
        assert_eq!(
            spans,
            vec![("one", 100, 140), ("two", 280, 300), ("three", 400, 440)]
        );
    }

    #[test]
    fn words_on_the_card_or_past_the_bridge_are_dropped() {
        let mut timeline = project(100.0, 500);
        append_bridge(&mut timeline, "https://example.com", 2_000).unwrap();
        let (placed, dropped) = place_words(
            &timeline,
            &words(&[("in", 1_000, 1_200), ("out", 6_000, 6_200)]),
        )
        .unwrap();
        assert_eq!(placed.len(), 1);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn groups_stop_at_their_size_a_pause_and_a_cut() {
        let style = preset("bold").unwrap();
        let w = |text: &str, start: i64, end: i64| PlacedWord {
            text: text.to_string(),
            start,
            end,
        };
        let list = vec![
            w("a", 0, 20),
            w("b", 20, 40),
            w("c", 40, 60),
            w("d", 60, 80),   // a fourth word: new group
            w("e", 130, 150), // 50 frames (500 ms) after d: new group
            w("f", 150, 170),
            w("g", 175, 190), // a cut at 172: new group
        ];
        let groups = group_words(&list, &[172], 100.0, &style);
        let texts: Vec<Vec<&str>> = groups
            .iter()
            .map(|group| group.words.iter().map(|word| word.text.as_str()).collect())
            .collect();
        assert_eq!(
            texts,
            vec![vec!["a", "b", "c"], vec!["d"], vec!["e", "f"], vec!["g"]]
        );
        assert_eq!((groups[0].start, groups[0].end), (0, 60));
    }

    #[test]
    fn a_long_group_wraps_into_two_balanced_lines_cased() {
        let style = preset("bold").unwrap();
        let group = CaptionGroup {
            start: 0,
            end: 10,
            words: ["understanding", "the", "market"]
                .iter()
                .map(|text| PlacedWord {
                    text: text.to_string(),
                    start: 0,
                    end: 10,
                })
                .collect(),
        };
        assert_eq!(
            group.lines(&style),
            vec![
                vec!["UNDERSTANDING".to_string()],
                vec!["THE".to_string(), "MARKET".to_string()]
            ]
        );
    }

    #[test]
    fn each_frame_shows_the_word_being_spoken() {
        let mut style = preset("bold").unwrap();
        let groups = vec![CaptionGroup {
            start: 10,
            end: 40,
            words: vec![
                PlacedWord {
                    text: "a".into(),
                    start: 10,
                    end: 18,
                },
                PlacedWord {
                    text: "b".into(),
                    start: 20,
                    end: 40,
                },
            ],
        }];
        let captions = Captions {
            style: style.clone(),
            groups: groups.clone(),
            dropped: 0,
        };
        let (states, shown) = caption_frames(&captions, 50);
        assert_eq!(states.len(), 2);
        assert_eq!(shown[9], None);
        assert_eq!(shown[10], Some(0));
        assert_eq!(shown[19], Some(0));
        assert_eq!(shown[20], Some(1));
        assert_eq!(shown[39], Some(1));
        assert_eq!(shown[40], None);
        style.highlight_color = None;
        let (states, shown) = caption_frames(
            &Captions {
                style,
                groups,
                dropped: 0,
            },
            50,
        );
        assert_eq!(
            states,
            vec![CaptionState {
                group: 0,
                highlight: None
            }]
        );
        assert!(shown[10..40].iter().all(|state| *state == Some(0)));
    }

    #[test]
    fn captions_are_recorded_read_back_refused_by_edits_and_removed() {
        let mut timeline = project(100.0, 1000);
        let list = words(&[("hello", 1_000, 1_400), ("there", 1_400, 1_800)]);
        let style = preset("bold").unwrap();
        let report = add_captions(&mut timeline, &list, &style).unwrap();
        assert_eq!(
            (
                report.groups,
                report.words,
                report.dropped,
                report.unchanged
            ),
            (1, 2, 0, false)
        );
        assert!(
            add_captions(&mut timeline, &list, &style)
                .unwrap()
                .unchanged
        );
        let captions = captions_of(&timeline).unwrap().unwrap();
        assert_eq!(captions.groups[0].words[1].text, "there");
        let error = cut_silence(
            &mut timeline,
            &dna(10_000, 100.0, &[]),
            &CutOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("add captions last"), "{error}");
        assert!(remove_captions(&mut timeline));
        assert!(!remove_captions(&mut timeline));
        assert_eq!(captions_of(&timeline).unwrap(), None);
    }

    #[test]
    fn nothing_to_caption_is_an_error() {
        let mut timeline = project(100.0, 100);
        let error = add_captions(
            &mut timeline,
            &words(&[("late", 5_000, 5_400)]),
            &preset("bold").unwrap(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("nothing to caption"), "{error}");
    }
}
