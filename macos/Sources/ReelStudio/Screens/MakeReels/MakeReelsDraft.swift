import Foundation
import ReelCore

/// An outlier DNA: an imported `edit_dna` asset, or a reel subject's head `out`.
enum MakeReelsOutlier: Hashable {
    case asset(AssetID)
    case subject(SubjectID)
}

/// The `overlay captions` style keys (crates/studio/src/commands.rs,
/// `CAPTION_STYLE`), as text fields; an empty field is left out so the preset's
/// value holds. `style` (a style file) is not offered here.
struct MakeReelsCaptionStyle: Hashable {
    static let presets = ["bold", "clean", "boxed", "karaoke", "minimal"]
    static let positions = ["top", "middle", "bottom"]
    static let cases = ["as-is", "upper", "lower"]

    /// Empty: the service's default preset.
    var preset = ""
    var font = ""
    var fontSize = ""
    var color = ""
    var highlightColor = ""
    var outlineColor = ""
    var outlineWidth = ""
    var shadow = ""
    var boxColor = ""
    var boxOpacity = ""
    var position = ""
    var margin = ""
    var wordsPerGroup = ""
    var maxChars = ""
    var textCase = ""
    var minGapMs = ""

    private enum Kind { case text, int, float }

    private struct Field {
        let key: String
        let label: String
        let value: String
        let kind: Kind
        let range: ClosedRange<Double>?
    }

    private var fields: [Field] {
        [
            Field(key: "font", label: "Font", value: font, kind: .text, range: nil),
            Field(key: "font_size", label: "Font size", value: fontSize, kind: .float, range: 2...12),
            Field(key: "color", label: "Colour", value: color, kind: .text, range: nil),
            Field(key: "highlight_color", label: "Highlight colour", value: highlightColor, kind: .text, range: nil),
            Field(key: "outline_color", label: "Outline colour", value: outlineColor, kind: .text, range: nil),
            Field(key: "outline_width", label: "Outline width", value: outlineWidth, kind: .int, range: 0...12),
            Field(key: "shadow", label: "Shadow", value: shadow, kind: .int, range: 0...12),
            Field(key: "box_color", label: "Box colour", value: boxColor, kind: .text, range: nil),
            Field(key: "box_opacity", label: "Box opacity", value: boxOpacity, kind: .float, range: 0...1),
            Field(key: "position", label: "Position", value: position, kind: .text, range: nil),
            Field(key: "margin", label: "Margin", value: margin, kind: .float, range: 0...40),
            Field(key: "words_per_group", label: "Words per group", value: wordsPerGroup, kind: .int, range: 1...5),
            Field(key: "max_chars", label: "Max characters", value: maxChars, kind: .int, range: 8...40),
            Field(key: "case", label: "Case", value: textCase, kind: .text, range: nil),
            Field(key: "min_gap_ms", label: "Minimum gap", value: minGapMs, kind: .int, range: 100...2000),
        ]
    }

    /// `MakeReelsRequest.style`.
    var json: [String: JSONValue] {
        var out: [String: JSONValue] = [:]
        if !preset.isEmpty { out["preset"] = .string(preset) }
        for field in fields {
            let text = field.value.trimmingCharacters(in: .whitespaces)
            if text.isEmpty { continue }
            switch field.kind {
            case .text:
                out[field.key] = .string(text)
            case .int:
                if let value = Int64(text) { out[field.key] = .int(value) }
            case .float:
                if let value = Double(text) { out[field.key] = .double(value) }
            }
        }
        return out
    }

    var problems: [String] {
        var found: [String] = []
        for field in fields {
            let text = field.value.trimmingCharacters(in: .whitespaces)
            if text.isEmpty { continue }
            let number: Double?
            switch field.kind {
            case .text:
                number = nil
            case .int:
                guard let value = Int64(text) else {
                    found.append("\(field.label) must be a whole number.")
                    continue
                }
                number = Double(value)
            case .float:
                guard let value = Double(text), value.isFinite else {
                    found.append("\(field.label) must be a number.")
                    continue
                }
                number = value
            }
            if let number, let range = field.range, !range.contains(number) {
                found.append("\(field.label) must be \(Self.short(range.lowerBound)) to \(Self.short(range.upperBound)).")
            }
        }
        let fontText = font.trimmingCharacters(in: .whitespaces)
        if !fontText.isEmpty && !fontText.hasPrefix("/") { found.append("The font must be an absolute path.") }
        return found
    }

    private static func short(_ value: Double) -> String {
        value.rounded() == value ? String(Int(value)) : String(value)
    }
}

/// The Make reels form, kept between visits to the screen.
struct MakeReelsDraft: Hashable {
    static let defaultModel = "qwen/qwen3.5-9b"

    var video: AssetID?
    /// An `edit_dna` of the long video with whisper words: skips decompile.
    var existingDna: AssetID?
    var outliers: [MakeReelsOutlier] = []
    var targetURL = ""
    var limitReels = false
    var reels = 3
    /// `1,3`: the clips to make, by rank.
    var clipsText = ""
    var allowFlagged = false
    /// `presenter` or `centre`.
    var follow = "presenter"
    var cardMsText = ""
    var size = "1080x1920"
    var model = ""
    var maxCostText = ""
    var style = MakeReelsCaptionStyle()

    /// The `clips` list, or nil when it is not a list of distinct positive numbers.
    var clips: [Int]? {
        let text = clipsText.trimmingCharacters(in: .whitespaces)
        if text.isEmpty { return [] }
        var list: [Int] = []
        for part in text.split(separator: ",") {
            guard let value = Int(part.trimmingCharacters(in: .whitespaces)), value > 0, !list.contains(value) else { return nil }
            list.append(value)
        }
        return list
    }

    var problems: [String] {
        var found: [String] = []
        if video == nil { found.append("Choose or drop the long video.") }
        if outliers.isEmpty { found.append("Add 1 to 3 outlier DNAs.") }
        if outliers.count > 3 { found.append("At most 3 outlier DNAs.") }
        let url = targetURL.trimmingCharacters(in: .whitespaces)
        if url.isEmpty { found.append("Enter the target URL for the bridge card.") }
        if clips == nil { found.append("Clips must be distinct numbers from 1, comma-separated (e.g. 1,3).") }
        let card = cardMsText.trimmingCharacters(in: .whitespaces)
        if !card.isEmpty, Int(card).map({ $0 > 0 }) != true { found.append("The card length must be a positive number of ms.") }
        let sizeText = size.trimmingCharacters(in: .whitespaces)
        if !sizeText.isEmpty && !Self.isSize(sizeText) { found.append("The size must be WxH, e.g. 1080x1920.") }
        let cost = maxCostText.trimmingCharacters(in: .whitespaces)
        if !cost.isEmpty, Double(cost).map({ $0 > 0 }) != true { found.append("The cost cap must be a positive amount in USD.") }
        found.append(contentsOf: style.problems)
        return found
    }

    var modelName: String {
        let text = model.trimmingCharacters(in: .whitespaces)
        return text.isEmpty ? Self.defaultModel : text
    }

    private static func isSize(_ text: String) -> Bool {
        let parts = text.split(separator: "x")
        guard parts.count == 2, let width = Int(parts[0]), let height = Int(parts[1]) else { return false }
        return width > 0 && height > 0
    }
}
