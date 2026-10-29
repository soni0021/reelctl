//! The typed chart spec: parsing, validation, canonical text, the overlay id, number
//! formatting and axis ranges. Pure.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use timeline::{OverlayPosition, MAX_OVERLAY_MS, MAX_WIDTH_PCT, MIN_OVERLAY_MS, MIN_WIDTH_PCT};

use crate::error::{Result, SceneError};

/// The largest spec file, in bytes.
pub const MAX_SPEC_BYTES: u64 = 64 * 1024;
pub const MAX_TITLE_CHARS: usize = 60;
pub const MAX_LABEL_CHARS: usize = 24;
pub const MAX_UNIT_CHARS: usize = 8;
/// The largest magnitude of a value.
pub const MAX_VALUE: f64 = 1e12;
pub const DEFAULT_WIDTH_PCT: u32 = 90;

fn default_width() -> u32 {
    DEFAULT_WIDTH_PCT
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChartKind {
    #[serde(rename = "bar")]
    Bar,
    #[serde(rename = "line")]
    Line,
    #[serde(rename = "number-callout")]
    NumberCallout,
}

impl ChartKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Line => "line",
            Self::NumberCallout => "number-callout",
        }
    }

    /// The number of points a series of this kind may have.
    fn points(self) -> std::ops::RangeInclusive<usize> {
        match self {
            Self::Bar => 1..=8,
            Self::Line => 2..=24,
            Self::NumberCallout => 1..=1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub label: String,
    pub value: f64,
}

/// A chart overlay, as the agent writes it. Field order is the canonical order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChartSpec {
    pub kind: ChartKind,
    pub title: String,
    pub series: Vec<Point>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default)]
    pub position: OverlayPosition,
    #[serde(default = "default_width")]
    pub width_pct: u32,
}

/// A character that cannot be in chart text: a control character, or one that
/// reorders what is drawn around it.
fn forbidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200E}' | '\u{200F}' | '\u{061C}')
        || ('\u{202A}'..='\u{202E}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

fn check_text(what: &str, text: &str, max: usize) -> Result<()> {
    let length = text.chars().count();
    if text.trim().is_empty() || length > max {
        return Err(SceneError::usage(format!(
            "{what} must be 1 to {max} characters, and not only spaces (it is {length})"
        )));
    }
    if let Some(bad) = text.chars().find(|c| forbidden(*c)) {
        return Err(SceneError::usage(format!(
            "{what} has the character U+{:04X}, a control or text-direction character; chart text may not have those",
            u32::from(bad)
        )));
    }
    Ok(())
}

impl ChartSpec {
    /// Parse and validate the text of a spec file.
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() as u64 > MAX_SPEC_BYTES {
            return Err(SceneError::usage(format!(
                "the chart spec is {} bytes; at most {MAX_SPEC_BYTES}",
                text.len()
            )));
        }
        let spec: Self = serde_json::from_str(text)
            .map_err(|error| SceneError::usage(format!("not a chart spec: {error}")))?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<()> {
        check_text("title", &self.title, MAX_TITLE_CHARS)?;
        if let Some(unit) = &self.unit {
            check_text("unit", unit, MAX_UNIT_CHARS)?;
        }
        let allowed = self.kind.points();
        if !allowed.contains(&self.series.len()) {
            return Err(SceneError::usage(format!(
                "a {} chart needs {} to {} points in series; this has {}",
                self.kind.as_str(),
                allowed.start(),
                allowed.end(),
                self.series.len()
            )));
        }
        for (index, point) in self.series.iter().enumerate() {
            check_text(
                &format!("series[{index}].label"),
                &point.label,
                MAX_LABEL_CHARS,
            )?;
            if !point.value.is_finite() || point.value.abs() > MAX_VALUE {
                return Err(SceneError::usage(format!(
                    "series[{index}].value {} must be a number of magnitude at most {MAX_VALUE}",
                    point.value
                )));
            }
            if self.kind != ChartKind::NumberCallout && point.value < 0.0 {
                return Err(SceneError::usage(format!(
                    "series[{index}].value {} is negative; bar and line charts take values of 0 or more",
                    point.value
                )));
            }
        }
        if self.end_ms <= self.start_ms
            || self.end_ms - self.start_ms < MIN_OVERLAY_MS
            || self.end_ms - self.start_ms > MAX_OVERLAY_MS
        {
            return Err(SceneError::usage(format!(
                "the span {}..{} ms must be {MIN_OVERLAY_MS} to {MAX_OVERLAY_MS} ms long",
                self.start_ms, self.end_ms
            )));
        }
        if !(MIN_WIDTH_PCT..=MAX_WIDTH_PCT).contains(&self.width_pct) {
            return Err(SceneError::usage(format!(
                "width_pct {} must be {MIN_WIDTH_PCT} to {MAX_WIDTH_PCT}",
                self.width_pct
            )));
        }
        Ok(())
    }

    /// The spec as JSON text in the canonical order with every default filled in.
    /// The same spec always gives the same text.
    pub fn canonical(&self) -> Result<String> {
        serde_json::to_string(self)
            .map_err(|error| SceneError::usage(format!("cannot write the spec: {error}")))
    }

    /// 16 hex digits naming this chart overlay: FNV-1a (64 bit) of the canonical text.
    pub fn id(&self) -> Result<String> {
        Ok(format!("{:016x}", fnv1a(self.canonical()?.as_bytes())))
    }

    /// `value` with the unit, as drawn.
    pub fn shown(&self, value: f64) -> String {
        format!(
            "{}{}",
            format_value(value),
            self.unit.as_deref().unwrap_or("")
        )
    }

    /// Everything the scene draws, computed here so the scene only places it.
    /// `duration_s` is how long the scene lasts.
    pub fn payload(&self, duration_s: f64) -> Value {
        let points: Vec<Value> = self
            .series
            .iter()
            .map(|point| {
                json!({
                    "label": point.label,
                    "value": point.value,
                    "shown": self.shown(point.value),
                })
            })
            .collect();
        let mut payload = json!({
            "kind": self.kind.as_str(),
            "title": self.title,
            "points": points,
            "duration_s": duration_s,
        });
        if let Some(object) = payload.as_object_mut() {
            match self.kind {
                ChartKind::NumberCallout => {
                    object.insert(
                        "display".to_string(),
                        json!(self.shown(self.series[0].value)),
                    );
                }
                ChartKind::Bar | ChartKind::Line => {
                    let top = self.series.iter().map(|p| p.value).fold(0.0, f64::max);
                    let (high, step) = axis_range(top);
                    object.insert("y_range".to_string(), json!([0.0, high, step]));
                }
            }
        }
        payload
    }
}

/// FNV-1a, 64 bit.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A number as drawn: at most 2 decimals, no trailing zeros, no exponent, no `-0`.
pub fn format_value(value: f64) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" || text.is_empty() {
        return "0".to_string();
    }
    text.to_string()
}

/// The top and step of a value axis that starts at 0 and has to hold `top` with a
/// little room above it: a step of 1, 2, 2.5, 5 or 10 times a power of ten, about
/// four steps up, and always at least one step above `top`.
pub fn axis_range(top: f64) -> (f64, f64) {
    if !top.is_finite() || top <= 0.0 {
        return (1.0, 0.25);
    }
    let raw = top / 4.0;
    let magnitude = 10f64.powi(raw.log10().floor() as i32);
    let normal = raw / magnitude;
    let factor = if normal <= 1.0 {
        1.0
    } else if normal <= 2.0 {
        2.0
    } else if normal <= 2.5 {
        2.5
    } else if normal <= 5.0 {
        5.0
    } else {
        10.0
    };
    let step = factor * magnitude;
    let mut high = (top / step).ceil() * step;
    if high - top < step * 1e-6 {
        high += step;
    }
    (round_to(high, 9), round_to(step, 9))
}

fn round_to(value: f64, decimals: i32) -> f64 {
    let scale = 10f64.powi(decimals);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_text(extra: &str) -> String {
        format!(
            r#"{{"kind":"bar","title":"Reels","series":[{{"label":"Mon","value":3}},{{"label":"Tue","value":5.5}}],"start_ms":2000,"end_ms":6000{extra}}}"#
        )
    }

    fn parse(text: &str) -> Result<ChartSpec> {
        ChartSpec::parse(text)
    }

    fn message(result: Result<ChartSpec>) -> String {
        result.expect_err("should be refused").to_string()
    }

    #[test]
    fn a_spec_gets_its_defaults() {
        let spec = parse(&spec_text("")).expect("spec");
        assert_eq!(spec.position, OverlayPosition::Center);
        assert_eq!(spec.width_pct, 90);
        assert_eq!(spec.unit, None);
        assert_eq!(spec.series.len(), 2);
        assert_eq!(spec.series[1].value, 5.5);
    }

    #[test]
    fn the_canonical_text_has_a_fixed_order_and_fills_defaults() {
        let spec = parse(&spec_text("")).expect("spec");
        assert_eq!(
            spec.canonical().expect("canonical"),
            r#"{"kind":"bar","title":"Reels","series":[{"label":"Mon","value":3.0},{"label":"Tue","value":5.5}],"start_ms":2000,"end_ms":6000,"position":"center","width_pct":90}"#
        );
        // Another order of the same fields, and the defaults written out, is the
        // same spec.
        let shuffled = r#"{"width_pct":90,"position":"center","end_ms":6000,"start_ms":2000,"series":[{"value":3,"label":"Mon"},{"value":5.5,"label":"Tue"}],"title":"Reels","kind":"bar"}"#;
        let other = parse(shuffled).expect("spec");
        assert_eq!(other.canonical().expect("c"), spec.canonical().expect("c"));
        assert_eq!(other.id().expect("id"), spec.id().expect("id"));
    }

    #[test]
    fn the_id_is_sixteen_hex_digits_that_change_with_any_field() {
        let base = parse(&spec_text("")).expect("spec");
        let id = base.id().expect("id");
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        for change in [
            r#","unit":"%""#,
            r#","position":"top""#,
            r#","width_pct":80"#,
        ] {
            let other = parse(&spec_text(change)).expect("spec");
            assert_ne!(other.id().expect("id"), id, "{change}");
        }
        assert_ne!(
            parse(&spec_text("").replace("3}", "4}"))
                .expect("s")
                .id()
                .expect("id"),
            id
        );
        assert_ne!(
            parse(&spec_text("").replace("2000", "2033"))
                .expect("s")
                .id()
                .expect("id"),
            id
        );
    }

    #[test]
    fn fnv1a_matches_the_reference_values() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn unknown_fields_and_bad_shapes_are_refused() {
        assert!(message(parse(&spec_text(r#","colour":"red""#))).contains("colour"));
        assert!(message(parse("not json")).contains("not a chart spec"));
        assert!(message(parse(&spec_text("").replace("bar", "pie"))).contains("pie"));
        assert!(message(parse(
            &spec_text("").replace("\"label\":\"Mon\"", "\"label\":\"Mon\",\"x\":1")
        ))
        .contains("x"));
        assert!(message(parse(&spec_text(r#","position":"left""#))).contains("left"));
    }

    #[test]
    fn a_spec_over_the_size_cap_is_refused_before_it_is_parsed() {
        let big = format!("{}{}", spec_text(""), " ".repeat(MAX_SPEC_BYTES as usize));
        assert!(message(parse(&big)).contains("at most 65536"));
        let at_cap = format!(
            "{}{}",
            spec_text(""),
            " ".repeat(MAX_SPEC_BYTES as usize - spec_text("").len())
        );
        assert_eq!(at_cap.len() as u64, MAX_SPEC_BYTES);
        parse(&at_cap).expect("exactly the cap is fine");
    }

    fn with_series(kind: &str, count: usize) -> String {
        let points: Vec<String> = (0..count)
            .map(|i| format!(r#"{{"label":"p{i}","value":{i}}}"#))
            .collect();
        format!(
            r#"{{"kind":"{kind}","title":"T","series":[{}],"start_ms":0,"end_ms":1000}}"#,
            points.join(",")
        )
    }

    #[test]
    fn each_kind_takes_its_number_of_points() {
        for (kind, low, high) in [("bar", 1, 8), ("line", 2, 24), ("number-callout", 1, 1)] {
            parse(&with_series(kind, low)).unwrap_or_else(|e| panic!("{kind} {low}: {e}"));
            parse(&with_series(kind, high)).unwrap_or_else(|e| panic!("{kind} {high}: {e}"));
            assert!(
                parse(&with_series(kind, high + 1)).is_err(),
                "{kind} {}",
                high + 1
            );
            if low > 0 {
                assert!(
                    parse(&with_series(kind, low - 1)).is_err(),
                    "{kind} {}",
                    low - 1
                );
            }
        }
    }

    #[test]
    fn text_is_bounded_and_has_no_control_or_direction_characters() {
        let long_title = "x".repeat(60);
        parse(&spec_text("").replace("Reels", &long_title)).expect("60 is fine");
        let too_long = "x".repeat(61);
        assert!(message(parse(&spec_text("").replace("Reels", &too_long))).contains("title"));
        assert!(message(parse(&spec_text("").replace("Reels", "   "))).contains("title"));
        assert!(message(parse(&spec_text("").replace("Reels", ""))).contains("title"));
        assert!(
            message(parse(&spec_text("").replace("Mon", &"y".repeat(25))))
                .contains("series[0].label")
        );
        parse(&spec_text("").replace("Mon", &"y".repeat(24))).expect("24 is fine");
        assert!(message(parse(&spec_text(r#","unit":"123456789""#))).contains("unit"));
        parse(&spec_text(r#","unit":"12345678""#)).expect("8 is fine");
        for bad in [
            "\\u0000", "\\n", "\\u202e", "\\u2066", "\\u200f", "\\u007f", "\\u0085",
        ] {
            let text = spec_text("").replace("Reels", &format!("a{bad}b"));
            assert!(message(parse(&text)).contains("U+"), "{bad}");
        }
        for fine in ["\"", "\\\\", "'", "$ # * { } < >", "日本語 é", "💥"] {
            let text = spec_text("").replace("Reels", &fine.replace('"', "\\\""));
            parse(&text).unwrap_or_else(|e| panic!("{fine}: {e}"));
        }
    }

    #[test]
    fn values_are_finite_bounded_and_not_negative_in_bars_and_lines() {
        for ok in ["0", "1000000000000", "0.001"] {
            parse(&spec_text("").replace("5.5", ok)).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in ["-1", "1000000000001", "-0.5"] {
            assert!(parse(&spec_text("").replace("5.5", bad)).is_err(), "{bad}");
        }
        let callout = r#"{"kind":"number-callout","title":"T","series":[{"label":"x","value":-5}],"start_ms":0,"end_ms":1000}"#;
        parse(callout).expect("a callout may be negative");
        let huge = callout.replace("-5", "-1000000000001");
        assert!(message(parse(&huge)).contains("magnitude"));
        let edge = callout.replace("-5", "-1000000000000");
        parse(&edge).expect("the magnitude cap itself");
    }

    #[test]
    fn the_span_and_the_width_are_bounded() {
        let at = |start: u64, end: u64| {
            spec_text("")
                .replace("2000", &start.to_string())
                .replace("6000", &end.to_string())
        };
        parse(&at(0, 500)).expect("500 ms");
        parse(&at(0, 30_000)).expect("30 s");
        // The length, not where the span sits: 10 s in, 500 ms is the minimum; 20 s in,
        // 5 s is fine and 30 s is the maximum.
        parse(&at(10_000, 10_500)).expect("500 ms late in the video");
        assert!(parse(&at(10_000, 10_499)).is_err());
        parse(&at(20_000, 25_000)).expect("5 s late in the video");
        parse(&at(20_000, 50_000)).expect("30 s late in the video");
        assert!(parse(&at(20_000, 50_001)).is_err());
        assert!(parse(&at(0, 499)).is_err());
        assert!(parse(&at(0, 30_001)).is_err());
        assert!(parse(&at(1000, 1000)).is_err());
        assert!(parse(&at(2000, 1000)).is_err());
        parse(&spec_text(r#","width_pct":20"#)).expect("20");
        parse(&spec_text(r#","width_pct":100"#)).expect("100");
        assert!(parse(&spec_text(r#","width_pct":19"#)).is_err());
        assert!(parse(&spec_text(r#","width_pct":101"#)).is_err());
    }

    #[test]
    fn numbers_are_drawn_with_two_decimals_at_most() {
        assert_eq!(format_value(3.0), "3");
        assert_eq!(format_value(5.5), "5.5");
        assert_eq!(format_value(5.25), "5.25");
        assert_eq!(format_value(5.255), "5.25");
        assert_eq!(format_value(0.004), "0");
        assert_eq!(format_value(-0.004), "0");
        assert_eq!(format_value(-12.5), "-12.5");
        assert_eq!(format_value(100.0), "100");
        assert_eq!(format_value(1e12), "1000000000000");
        let spec = parse(&spec_text(r#","unit":"%""#)).expect("spec");
        assert_eq!(spec.shown(5.5), "5.5%");
    }

    #[test]
    fn the_axis_holds_the_top_with_room_in_round_steps() {
        assert_eq!(axis_range(5.5), (6.0, 2.0));
        assert_eq!(axis_range(7.0), (8.0, 2.0));
        assert_eq!(axis_range(5.0), (6.0, 2.0));
        assert_eq!(axis_range(8.0), (10.0, 2.0));
        assert_eq!(axis_range(100.0), (125.0, 25.0));
        assert_eq!(axis_range(1.0), (1.25, 0.25));
        assert_eq!(axis_range(0.0), (1.0, 0.25));
        assert_eq!(axis_range(f64::NAN), (1.0, 0.25));
        assert_eq!(axis_range(1e12), (1.25e12, 2.5e11));
        assert_eq!(axis_range(0.3), (0.4, 0.1));
        for top in [0.7, 1.9, 3.0, 7.0, 12.0, 99.0, 250.0, 4321.0] {
            let (high, step) = axis_range(top);
            assert!(high > top, "{top}: {high}");
            assert!(
                high - top <= step * 1.0001 + top * 1e-9 || high / step <= 6.01,
                "{top}"
            );
            assert!(step > 0.0);
        }
    }

    #[test]
    fn the_payload_carries_everything_the_scene_draws() {
        let spec = parse(&spec_text(r#","unit":"k""#)).expect("spec");
        let payload = spec.payload(4.0);
        assert_eq!(payload["kind"], "bar");
        assert_eq!(payload["title"], "Reels");
        assert_eq!(payload["duration_s"], 4.0);
        assert_eq!(payload["y_range"], json!([0.0, 6.0, 2.0]));
        assert_eq!(payload["points"][1]["shown"], "5.5k");
        assert_eq!(payload["points"][0]["label"], "Mon");
        assert!(payload.get("display").is_none());
        let callout = r#"{"kind":"number-callout","title":"T","series":[{"label":"views","value":1234.5}],"unit":"x","start_ms":0,"end_ms":1000}"#;
        let payload = parse(callout).expect("spec").payload(1.0);
        assert_eq!(payload["display"], "1234.5x");
        assert!(payload.get("y_range").is_none());
    }
}
