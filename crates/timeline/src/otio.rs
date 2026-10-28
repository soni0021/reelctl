//! The OpenTimelineIO JSON subset: `Timeline.1`, `Stack.1`, `Track.1`, `Clip.2`,
//! `Gap.1`, `ExternalReference.1`, `GeneratorReference.1`, `RationalTime.1`, and
//! `TimeRange.1`. Fields this crate does not model are kept in `extra` and written
//! back unchanged.

use std::collections::BTreeMap;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::error::{Result, TimelineError};

pub const TIMELINE_SCHEMA: &str = "Timeline.1";
pub const STACK_SCHEMA: &str = "Stack.1";
pub const TRACK_SCHEMA: &str = "Track.1";
pub const CLIP_SCHEMA: &str = "Clip.2";
pub const GAP_SCHEMA: &str = "Gap.1";
pub const EXTERNAL_SCHEMA: &str = "ExternalReference.1";
pub const GENERATOR_SCHEMA: &str = "GeneratorReference.1";
pub const RATIONAL_TIME_SCHEMA: &str = "RationalTime.1";
pub const TIME_RANGE_SCHEMA: &str = "TimeRange.1";
/// The key of the reference-library default media reference.
pub const DEFAULT_MEDIA_KEY: &str = "DEFAULT_MEDIA";
/// The metadata key under which this tool records its own data.
pub const REELCTL_KEY: &str = "reelctl";

/// `Track.1` kinds.
pub const VIDEO_KIND: &str = "Video";
pub const AUDIO_KIND: &str = "Audio";

fn rational_time_schema() -> String {
    RATIONAL_TIME_SCHEMA.to_string()
}

fn time_range_schema() -> String {
    TIME_RANGE_SCHEMA.to_string()
}

/// A time as `value` ticks at `rate` ticks per second.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RationalTime {
    #[serde(rename = "OTIO_SCHEMA", default = "rational_time_schema")]
    schema: String,
    pub rate: f64,
    pub value: f64,
}

impl RationalTime {
    pub fn new(value: f64, rate: f64) -> Self {
        Self {
            schema: rational_time_schema(),
            rate,
            value,
        }
    }
}

/// A start and a duration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    #[serde(rename = "OTIO_SCHEMA", default = "time_range_schema")]
    schema: String,
    pub duration: RationalTime,
    pub start_time: RationalTime,
}

impl TimeRange {
    pub fn new(start: f64, duration: f64, rate: f64) -> Self {
        Self {
            schema: time_range_schema(),
            duration: RationalTime::new(duration, rate),
            start_time: RationalTime::new(start, rate),
        }
    }
}

/// A file on disk. `metadata.reelctl` holds the size and rate `init` measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalReference {
    #[serde(rename = "OTIO_SCHEMA")]
    schema: String,
    #[serde(default)]
    pub name: String,
    pub target_url: String,
    #[serde(default)]
    pub available_range: Option<TimeRange>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ExternalReference {
    pub fn new(target_url: &str, available_range: TimeRange) -> Self {
        Self {
            schema: EXTERNAL_SCHEMA.to_string(),
            name: String::new(),
            target_url: target_url.to_string(),
            available_range: Some(available_range),
            metadata: Map::new(),
            extra: Map::new(),
        }
    }
}

/// A clip made by a program rather than read from a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratorReference {
    #[serde(rename = "OTIO_SCHEMA")]
    schema: String,
    #[serde(default)]
    pub name: String,
    pub generator_kind: String,
    #[serde(default)]
    pub parameters: Map<String, Value>,
    #[serde(default)]
    pub available_range: Option<TimeRange>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl GeneratorReference {
    pub fn new(kind: &str, parameters: Map<String, Value>, available_range: TimeRange) -> Self {
        Self {
            schema: GENERATOR_SCHEMA.to_string(),
            name: String::new(),
            generator_kind: kind.to_string(),
            parameters,
            available_range: Some(available_range),
            metadata: Map::new(),
            extra: Map::new(),
        }
    }
}

/// A clip's media. A reference of another kind (a missing-media placeholder, say)
/// is kept as it was read and cannot be edited or rendered.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaReference {
    External(ExternalReference),
    Generator(GeneratorReference),
    Other(Value),
}

impl Serialize for MediaReference {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::External(reference) => reference.serialize(serializer),
            Self::Generator(reference) => reference.serialize(serializer),
            Self::Other(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MediaReference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let schema = value
            .get("OTIO_SCHEMA")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if schema.starts_with("ExternalReference.") {
            serde_json::from_value(value)
                .map(Self::External)
                .map_err(D::Error::custom)
        } else if schema.starts_with("GeneratorReference.") {
            serde_json::from_value(value)
                .map(Self::Generator)
                .map_err(D::Error::custom)
        } else {
            Ok(Self::Other(value))
        }
    }
}

/// A piece of media on a track, from `source_range` of its active reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    #[serde(rename = "OTIO_SCHEMA")]
    schema: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub source_range: Option<TimeRange>,
    pub media_references: BTreeMap<String, MediaReference>,
    pub active_media_reference_key: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Clip {
    pub fn new(name: &str, source_range: TimeRange, reference: MediaReference) -> Self {
        let mut media_references = BTreeMap::new();
        media_references.insert(DEFAULT_MEDIA_KEY.to_string(), reference);
        Self {
            schema: CLIP_SCHEMA.to_string(),
            name: name.to_string(),
            source_range: Some(source_range),
            media_references,
            active_media_reference_key: DEFAULT_MEDIA_KEY.to_string(),
            metadata: Map::new(),
            extra: Map::new(),
        }
    }

    /// The reference named by `active_media_reference_key`.
    pub fn active_reference(&self) -> Result<&MediaReference> {
        self.media_references
            .get(&self.active_media_reference_key)
            .ok_or_else(|| {
                TimelineError::invalid(format!(
                    "clip \"{}\": active_media_reference_key \"{}\" names no media reference",
                    self.name, self.active_media_reference_key
                ))
            })
    }
}

/// Empty time on a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    #[serde(rename = "OTIO_SCHEMA")]
    schema: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub source_range: Option<TimeRange>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Gap {
    pub fn new(source_range: TimeRange) -> Self {
        Self {
            schema: GAP_SCHEMA.to_string(),
            name: String::new(),
            source_range: Some(source_range),
            metadata: Map::new(),
            extra: Map::new(),
        }
    }
}

/// What a track holds. A transition or a nested stack is an error to read: this
/// tool cannot place it, and dropping it would change the user's project.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Clip(Clip),
    Gap(Gap),
}

impl Serialize for Item {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Clip(clip) => clip.serialize(serializer),
            Self::Gap(gap) => gap.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let schema = value
            .get("OTIO_SCHEMA")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if schema == CLIP_SCHEMA {
            serde_json::from_value(value)
                .map(Self::Clip)
                .map_err(D::Error::custom)
        } else if schema.starts_with("Gap.") {
            serde_json::from_value(value)
                .map(Self::Gap)
                .map_err(D::Error::custom)
        } else {
            Err(D::Error::custom(format!(
                "unsupported track item \"{schema}\" (this tool reads {CLIP_SCHEMA} and {GAP_SCHEMA})"
            )))
        }
    }
}

impl Item {
    pub fn source_range(&self) -> Option<&TimeRange> {
        match self {
            Self::Clip(clip) => clip.source_range.as_ref(),
            Self::Gap(gap) => gap.source_range.as_ref(),
        }
    }

    pub fn source_range_mut(&mut self) -> &mut Option<TimeRange> {
        match self {
            Self::Clip(clip) => &mut clip.source_range,
            Self::Gap(gap) => &mut gap.source_range,
        }
    }

    pub fn metadata_mut(&mut self) -> &mut Map<String, Value> {
        match self {
            Self::Clip(clip) => &mut clip.metadata,
            Self::Gap(gap) => &mut gap.metadata,
        }
    }

    pub fn metadata(&self) -> &Map<String, Value> {
        match self {
            Self::Clip(clip) => &clip.metadata,
            Self::Gap(gap) => &gap.metadata,
        }
    }
}

/// A sequence of items of one kind (`Video` or `Audio`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    #[serde(rename = "OTIO_SCHEMA")]
    schema: String,
    #[serde(default)]
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub children: Vec<Item>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Track {
    pub fn new(name: &str, kind: &str) -> Self {
        Self {
            schema: TRACK_SCHEMA.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            children: Vec::new(),
            metadata: Map::new(),
            extra: Map::new(),
        }
    }
}

/// The tracks of a timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stack {
    #[serde(rename = "OTIO_SCHEMA")]
    schema: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub children: Vec<Track>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A whole project: one stack of tracks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    #[serde(rename = "OTIO_SCHEMA")]
    schema: String,
    #[serde(default)]
    pub name: String,
    pub tracks: Stack,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Timeline {
    pub fn new(name: &str) -> Self {
        Self {
            schema: TIMELINE_SCHEMA.to_string(),
            name: name.to_string(),
            tracks: Stack {
                schema: STACK_SCHEMA.to_string(),
                name: "tracks".to_string(),
                children: Vec::new(),
                metadata: Map::new(),
                extra: Map::new(),
            },
            metadata: Map::new(),
            extra: Map::new(),
        }
    }

    /// Parse `.otio` text.
    pub fn from_json(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        let schema = value
            .get("OTIO_SCHEMA")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !schema.starts_with("Timeline.") {
            return Err(TimelineError::invalid(format!(
                "not an OTIO timeline: OTIO_SCHEMA is \"{schema}\", expected \"{TIMELINE_SCHEMA}\""
            )));
        }
        Ok(serde_json::from_value(value)?)
    }

    /// Pretty JSON with a trailing newline. The same project always gives the
    /// same text.
    pub fn to_json(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }

    /// The first track of a kind.
    pub fn track(&self, kind: &str) -> Option<&Track> {
        self.tracks.children.iter().find(|track| track.kind == kind)
    }

    pub fn track_mut(&mut self, kind: &str) -> Option<&mut Track> {
        self.tracks
            .children
            .iter_mut()
            .find(|track| track.kind == kind)
    }
}

/// `metadata.reelctl`, when present.
pub fn reelctl(metadata: &Map<String, Value>) -> Option<&Map<String, Value>> {
    metadata.get(REELCTL_KEY).and_then(Value::as_object)
}

/// Run `change` on `metadata.reelctl`, creating it (or replacing a value that is not
/// an object) first.
pub fn update_reelctl<R>(
    metadata: &mut Map<String, Value>,
    change: impl FnOnce(&mut Map<String, Value>) -> R,
) -> R {
    let mut inner = match metadata.remove(REELCTL_KEY) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let result = change(&mut inner);
    metadata.insert(REELCTL_KEY.to_string(), Value::Object(inner));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the reference library (python OpenTimelineIO 0.18.1) writes for a clip
    /// with an external reference, a gap, and a generator clip; plus fields this
    /// crate does not model (`color`, `markers`, `x_custom`).
    const REFERENCE: &str = include_str!("../tests/reference.otio");

    #[test]
    fn a_file_from_the_reference_library_is_read() {
        let timeline = Timeline::from_json(REFERENCE).expect("parse");
        let video = timeline.track(VIDEO_KIND).expect("video track");
        assert_eq!(video.children.len(), 3);
        match &video.children[0] {
            Item::Clip(clip) => {
                let range = clip.source_range.as_ref().expect("range");
                assert_eq!(range.start_time.value, 10.0);
                assert_eq!(range.duration.value, 100.0);
                match clip.active_reference().expect("reference") {
                    MediaReference::External(external) => {
                        assert_eq!(external.target_url, "/x/a.mp4");
                    }
                    other => panic!("expected an external reference, got {other:?}"),
                }
            }
            other => panic!("expected a clip, got {other:?}"),
        }
        assert!(matches!(video.children[1], Item::Gap(_)));
        match &video.children[2] {
            Item::Clip(clip) => assert!(matches!(
                clip.active_reference().expect("reference"),
                MediaReference::Generator(_)
            )),
            other => panic!("expected a clip, got {other:?}"),
        }
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        let timeline = Timeline::from_json(REFERENCE).expect("parse");
        let text = timeline.to_json().expect("write");
        let before: Value = serde_json::from_str(REFERENCE).expect("json");
        let after: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(before, after, "a read and write changed the file");
        assert!(text.contains("x_custom"));
        assert!(text.contains("\"markers\""));
    }

    #[test]
    fn writing_twice_gives_the_same_text() {
        let timeline = Timeline::from_json(REFERENCE).expect("parse");
        let once = timeline.to_json().expect("write");
        let again = Timeline::from_json(&once)
            .expect("parse")
            .to_json()
            .expect("write");
        assert_eq!(once, again);
    }

    #[test]
    fn an_unsupported_item_is_an_error_that_names_it() {
        let text = REFERENCE.replacen("\"Gap.1\"", "\"Transition.1\"", 1);
        let error = Timeline::from_json(&text).expect_err("transition");
        assert!(error.to_string().contains("Transition.1"), "{error}");
    }

    #[test]
    fn json_that_is_not_a_timeline_is_refused() {
        let error = Timeline::from_json("{\"OTIO_SCHEMA\":\"Clip.2\"}").expect_err("clip");
        assert!(error.to_string().contains("Timeline.1"), "{error}");
        assert!(Timeline::from_json("not json").is_err());
    }
}
