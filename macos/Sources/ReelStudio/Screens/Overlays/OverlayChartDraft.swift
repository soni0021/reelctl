import Foundation
import ReelCore

// The chart spec `overlay add-chart` takes (crates/scenes/src/spec.rs,
// `ChartSpec`, `deny_unknown_fields`): kind, title, series of {label, value},
// unit (optional), start_ms, end_ms, position, width_pct. The checks here mirror
// docs/cli.md "Chart overlays: overlay"; the service checks again.

enum OverlayChartKind: String, CaseIterable, Identifiable, Hashable {
    case bar
    case line
    case numberCallout = "number-callout"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .bar: return "Bar"
        case .line: return "Line"
        case .numberCallout: return "Number callout"
        }
    }

    var minPoints: Int {
        switch self {
        case .bar: return 1
        case .line: return 2
        case .numberCallout: return 1
        }
    }

    var maxPoints: Int {
        switch self {
        case .bar: return 8
        case .line: return 24
        case .numberCallout: return 1
        }
    }

    var pointsText: String {
        minPoints == maxPoints ? "exactly \(minPoints)" : "\(minPoints) to \(maxPoints) points"
    }
}

enum OverlayChartPosition: String, CaseIterable, Identifiable, Hashable {
    case top, center, bottom

    var id: String { rawValue }
    var title: String { rawValue.capitalized }
}

struct OverlayChartPoint: Identifiable, Hashable {
    var id = UUID()
    var label: String
    var value: Double
}

struct OverlayChartDraft: Hashable {
    static let minSpanMs = 500
    static let maxSpanMs = 30_000
    static let maxValue = 1e12

    var kind: OverlayChartKind = .bar
    var title = ""
    var points: [OverlayChartPoint] = [
        OverlayChartPoint(label: "Before", value: 10),
        OverlayChartPoint(label: "After", value: 40),
    ]
    var unit = ""
    var startMs = 1000
    var endMs = 4000
    var position: OverlayChartPosition = .center
    var widthPct = 90

    /// Keep the series inside what `kind` allows.
    mutating func fit(to kind: OverlayChartKind) {
        if points.count > kind.maxPoints { points = Array(points.prefix(kind.maxPoints)) }
        while points.count < kind.minPoints {
            points.append(OverlayChartPoint(label: "Point \(points.count + 1)", value: 0))
        }
    }

    mutating func addPoint() {
        guard points.count < kind.maxPoints else { return }
        points.append(OverlayChartPoint(label: "Point \(points.count + 1)", value: 0))
    }

    mutating func removePoint(_ id: UUID) {
        guard points.count > kind.minPoints else { return }
        points.removeAll { $0.id == id }
    }

    /// The spec as the service reads it. `unit` is left out when empty.
    var json: JSONValue {
        var object: [String: JSONValue] = [
            "kind": .string(kind.rawValue),
            "title": .string(title),
            "series": .array(points.map { point -> JSONValue in
                ["label": .string(point.label), "value": .double(point.value)]
            }),
            "start_ms": .int(Int64(max(0, startMs))),
            "end_ms": .int(Int64(max(0, endMs))),
            "position": .string(position.rawValue),
            "width_pct": .int(Int64(widthPct)),
        ]
        let trimmedUnit = unit.trimmingCharacters(in: .whitespaces)
        if !trimmedUnit.isEmpty { object["unit"] = .string(unit) }
        return .object(object)
    }

    /// Indented JSON, for the preview and the file.
    var prettyText: String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        guard let data = try? encoder.encode(json) else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }

    /// What the service would refuse (exit 2), worded for the form.
    var problems: [String] {
        var found: [String] = []
        let titleCount = title.count
        if title.trimmingCharacters(in: .whitespaces).isEmpty {
            found.append("The title is empty.")
        } else if titleCount > 60 {
            found.append("The title is longer than 60 characters.")
        }
        if Self.hasForbiddenCharacters(title) { found.append("The title holds a control or text-direction character.") }
        if points.count < kind.minPoints || points.count > kind.maxPoints {
            found.append("A \(kind.title.lowercased()) chart takes \(kind.pointsText).")
        }
        for (index, point) in points.enumerated() {
            let number = index + 1
            if point.label.trimmingCharacters(in: .whitespaces).isEmpty {
                found.append("Point \(number) has no label.")
            } else if point.label.count > 24 {
                found.append("Point \(number)'s label is longer than 24 characters.")
            }
            if Self.hasForbiddenCharacters(point.label) { found.append("Point \(number)'s label holds a control or text-direction character.") }
            if !point.value.isFinite {
                found.append("Point \(number)'s value is not a number.")
            } else if kind == .numberCallout {
                if abs(point.value) > Self.maxValue { found.append("The callout's value is over 1e12 in size.") }
            } else if point.value < 0 || point.value > Self.maxValue {
                found.append("Point \(number)'s value must be 0 to 1e12.")
            }
        }
        let trimmedUnit = unit.trimmingCharacters(in: .whitespaces)
        if !unit.isEmpty && trimmedUnit.isEmpty { found.append("The unit is only spaces.") }
        if unit.count > 8 { found.append("The unit is longer than 8 characters.") }
        if Self.hasForbiddenCharacters(unit) { found.append("The unit holds a control or text-direction character.") }
        if startMs < 0 { found.append("The start is before 0 ms.") }
        let span = endMs - startMs
        if span < Self.minSpanMs || span > Self.maxSpanMs {
            found.append("The span must be 500 to 30000 ms long (it is \(span) ms).")
        }
        if widthPct < 20 || widthPct > 100 { found.append("The width must be 20 to 100 percent.") }
        return found
    }

    private static let directionScalars: Set<UInt32> = [
        0x061C, 0x200E, 0x200F, 0x202A, 0x202B, 0x202C, 0x202D, 0x202E, 0x2066, 0x2067, 0x2068, 0x2069,
    ]

    static func hasForbiddenCharacters(_ text: String) -> Bool {
        text.unicodeScalars.contains { scalar in
            CharacterSet.controlCharacters.contains(scalar) || directionScalars.contains(scalar.value)
        }
    }
}

/// Writes a spec to the temporary folder (never the store) for `assets.import`.
enum OverlayChartFile {
    static func write(_ draft: OverlayChartDraft) throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ReelStudio-charts", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appendingPathComponent("chart-\(UUID().uuidString).json")
        try Data(draft.prettyText.utf8).write(to: url, options: .atomic)
        return url
    }
}
