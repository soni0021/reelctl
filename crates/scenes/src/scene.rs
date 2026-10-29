//! The scene file: a fixed Python template with the chart's data in it once, as a
//! string literal. Chart text never becomes Python syntax.

use serde_json::Value;

/// The scene template. `@@REELCTL_PAYLOAD@@` appears in it exactly once.
pub const TEMPLATE: &str = include_str!("scene_template.py");
/// Where the data goes.
pub const MARKER: &str = "@@REELCTL_PAYLOAD@@";
/// The scene class in the template, named on the command line.
pub const SCENE_NAME: &str = "ReelctlChart";

/// `value` as JSON text that is pure ASCII: every character above U+007E is a
/// `\uXXXX` escape (a pair, for one outside the basic plane).
pub fn ascii_json(value: &Value) -> String {
    let text = value.to_string();
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if (c as u32) < 0x7f {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// A Python string literal whose value is `text`. A JSON string is one: `serde_json`
/// writes only `\" \\ \n \r \t \b \f` and `\u00XX`, all of which Python reads the
/// same way, and never `\/`.
pub fn python_literal(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

/// The scene file for a payload: the template with the data, written as JSON text and
/// then as a string literal, in place of the marker. Only the first marker is
/// replaced, so a payload that contains the marker text is data.
pub fn scene_source(payload: &Value) -> String {
    let literal = python_literal(&ascii_json(payload));
    match TEMPLATE.split_once(MARKER) {
        Some((before, after)) => format!("{before}{literal}{after}"),
        None => TEMPLATE.to_string(),
    }
}

/// The size of the scene's video: `width_pct` of the source's width, rounded to an
/// even number, and four thirds as wide as high.
pub fn scene_size(source_width: u32, width_pct: u32) -> (u32, u32) {
    let even = |value: f64| ((value / 2.0).round() as u32).max(1) * 2;
    let width = even(f64::from(source_width) * f64::from(width_pct) / 100.0);
    let height = even(f64::from(width) * 3.0 / 4.0);
    (width, height)
}

/// The frame rate the scene is rendered at: the project's, rounded to a whole
/// number (the tool takes only whole rates), 1 to 60.
pub fn scene_fps(project_fps: f64) -> u32 {
    (project_fps.round() as i64).clamp(1, 60) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ChartSpec;
    use serde_json::json;

    fn spec_with_title(title: &str) -> ChartSpec {
        let text = json!({
            "kind": "bar",
            "title": title,
            "series": [{"label": title.chars().take(24).collect::<String>(), "value": 3}],
            "unit": title.chars().take(8).collect::<String>(),
            "start_ms": 0,
            "end_ms": 4000,
        })
        .to_string();
        ChartSpec::parse(&text).expect("spec")
    }

    #[test]
    fn the_template_has_its_one_marker() {
        assert_eq!(TEMPLATE.matches(MARKER).count(), 1);
        assert!(TEMPLATE.contains(&format!("class {SCENE_NAME}(")));
        assert!(TEMPLATE.contains("m.config.background_color = \"#ff00ff\""));
        // The draw-in never outlasts a short span (the shortest is 500 ms).
        assert!(TEMPLATE.contains("animation = min(ANIMATION_SECONDS, DATA[\"duration_s\"])"));
        assert!(TEMPLATE.contains("run_time=animation"));
        assert!(!TEMPLATE.contains("run_time=ANIMATION_SECONDS"));
        assert!(TEMPLATE.contains(&format!("\"{}\"", timeline::KEY_COLOR)));
    }

    /// Everything a title could try.
    const HOSTILE: [&str; 8] = [
        "\"\"\" + __import__('os').system('x') + \"\"\"",
        "it's",
        "back\\slash \\n \\\" \\u0041",
        "# comment {} {x} %s",
        "日本語 é ñ",
        "emoji 💥 \u{1F600}",
        "@@REELCTL_PAYLOAD@@",
        "'''\\''' \"\\\"",
    ];

    #[test]
    fn hostile_text_stays_inside_one_literal_that_decodes_back_to_the_data() {
        let (before, after) = TEMPLATE.split_once(MARKER).expect("marker");
        for title in HOSTILE {
            let spec = spec_with_title(title);
            let payload = spec.payload(4.0);
            let source = scene_source(&payload);
            // The file is the template with exactly the literal put in the hole.
            assert!(source.starts_with(before), "{title}");
            assert!(source.ends_with(after), "{title}");
            let literal = &source[before.len()..source.len() - after.len()];
            assert!(
                literal.starts_with('"') && literal.ends_with('"'),
                "{title}"
            );
            assert!(literal.is_ascii(), "{title}");
            assert!(
                !literal.contains('\n') && !literal.contains('\r'),
                "{title}"
            );
            // No raw quote inside it: every quote in the middle is escaped.
            let middle = &literal[1..literal.len() - 1];
            let mut escaped = false;
            for c in middle.chars() {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else {
                    assert_ne!(c, '"', "{title}: a raw quote ends the literal early");
                }
            }
            assert!(!escaped, "{title}");
            // The literal's value is JSON text and that text is the payload.
            let inner: String = serde_json::from_str(literal).expect("a string literal");
            let decoded: Value = serde_json::from_str(&inner).expect("json inside");
            assert_eq!(decoded, payload, "{title}");
            assert_eq!(decoded["title"], title);
            // The rest of the file is the template and nothing of the title.
            assert!(!before.contains("__import__") && !after.contains("__import__"));
            // One call to json.loads, one literal.
            assert_eq!(source.matches("json.loads(").count(), 1, "{title}");
        }
    }

    #[test]
    fn a_control_character_cannot_reach_a_scene() {
        let text = json!({
            "kind": "bar", "title": "line\u{85}one",
            "series": [{"label": "a", "value": 1}], "start_ms": 0, "end_ms": 4000,
        })
        .to_string();
        assert!(ChartSpec::parse(&text).is_err());
    }

    #[test]
    fn the_payload_text_is_ascii_with_surrogate_pairs() {
        let value = json!({"a": "é💥", "b": "plain"});
        let text = ascii_json(&value);
        assert!(text.is_ascii());
        assert!(text.contains("\\u00e9"));
        assert!(text.contains("\\ud83d\\udca5"));
        let back: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(back, value);
        // U+007F (DEL) is escaped too; U+007E is not.
        assert_eq!(ascii_json(&json!("~\u{7f}")), "\"~\\u007f\"");
    }

    #[test]
    fn the_literal_is_the_json_encoding_of_the_text() {
        assert_eq!(python_literal("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(python_literal(""), "\"\"");
    }

    #[test]
    fn the_scene_size_follows_the_source_width() {
        assert_eq!(scene_size(1080, 90), (972, 730));
        assert_eq!(scene_size(1920, 100), (1920, 1440));
        assert_eq!(scene_size(640, 20), (128, 96));
        assert_eq!(scene_size(1080, 33), (356, 267 + 1));
        assert_eq!(scene_size(2, 20), (2, 2));
        let (w, h) = scene_size(1081, 77);
        assert!(w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn the_scene_rate_is_a_whole_number_from_1_to_60() {
        assert_eq!(scene_fps(30.0), 30);
        assert_eq!(scene_fps(29.97002997), 30);
        assert_eq!(scene_fps(23.976), 24);
        assert_eq!(scene_fps(120.0), 60);
        assert_eq!(scene_fps(0.2), 1);
        assert_eq!(scene_fps(60.0), 60);
        assert_eq!(scene_fps(1.0), 1);
    }
}
