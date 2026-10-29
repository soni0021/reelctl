import ReelCore
import SwiftUI

/// The rows of the Library's lanes view, top to bottom.
enum AnalysisLane: Int, CaseIterable, Identifiable, Hashable {
    case ruler, cuts, shots, silence, words, loudness, events, zooms

    var id: Int { rawValue }

    var title: String {
        switch self {
        case .ruler: return "Time"
        case .cuts: return "Cuts"
        case .shots: return "Shots"
        case .silence: return "Silence"
        case .words: return "Words"
        case .loudness: return "Loudness"
        case .events: return "Events"
        case .zooms: return "Zooms"
        }
    }

    var height: CGFloat {
        switch self {
        case .ruler: return 16
        case .words: return 26
        case .shots: return 22
        default: return 20
        }
    }

    /// Where the lane starts, from the top of the lanes view.
    var top: CGFloat {
        AnalysisLane.allCases.filter { $0.rawValue < rawValue }.reduce(CGFloat(0)) { $0 + $1.height }
    }

    static var totalHeight: CGFloat { allCases.reduce(CGFloat(0)) { $0 + $1.height } }

    static func lane(atY y: CGFloat) -> AnalysisLane? {
        allCases.first { y >= $0.top && y < $0.top + $0.height }
    }
}

struct AnalysisField: Hashable, Identifiable {
    let label: String
    let value: String
    var id: String { label }
}

/// One thing drawn on a lane: a point (a cut, an event) or a range (a shot, a
/// silence gap, a word, a zoom curve, the payoff window).
struct AnalysisMarker: Identifiable, Hashable {
    enum Kind: String, Hashable {
        case cut, shot, silence, deadAir, word, punchIn, zoomOut, overlayChange, otherEvent, zoom, payoff, loudness
    }

    let id: String
    let kind: Kind
    let lane: AnalysisLane
    let startMs: Int64
    /// Equal to `startMs` for a point.
    let endMs: Int64
    /// Where a click seeks (inside the marker's frame).
    let seekMs: Int64
    let title: String
    let fields: [AnalysisField]
    /// A shot's mean colour, 0...1.
    var rgb: [Double] = []

    var isPoint: Bool { endMs <= startMs }

    var color: Color {
        switch kind {
        case .cut: return .orange
        case .shot:
            guard rgb.count == 3 else { return .gray }
            return Color(red: rgb[0], green: rgb[1], blue: rgb[2])
        case .silence: return .gray
        case .deadAir: return .red
        case .word: return .yellow
        case .punchIn: return .green
        case .zoomOut: return .purple
        case .overlayChange: return .teal
        case .otherEvent: return .pink
        case .zoom: return .indigo
        case .payoff: return .yellow
        case .loudness: return .blue
        }
    }
}

enum AnalysisMarkers {
    static func build(_ dna: EditDNA) -> [AnalysisMarker] {
        var list: [AnalysisMarker] = []
        let fps = dna.fps
        let n = AnalysisSupport.number

        let payoff = dna.payoffWindow
        var payoffFields: [AnalysisField] = [
            AnalysisField(label: "Window", value: "\(formatMs(payoff.windowStartMs)) – \(formatMs(payoff.windowEndMs))"),
            AnalysisField(label: "Marker", value: formatMs(payoff.markerMs)),
            AnalysisField(label: "Has cue", value: payoff.hasCue ? "yes" : "no"),
        ]
        if let cue = payoff.cueMs { payoffFields.append(AnalysisField(label: "Cue", value: formatMs(cue))) }
        list.append(AnalysisMarker(
            id: "payoff", kind: .payoff, lane: .cuts,
            startMs: payoff.windowStartMs, endMs: max(payoff.windowEndMs, payoff.windowStartMs + 1),
            seekMs: payoff.cueMs ?? payoff.windowStartMs,
            title: "Payoff window", fields: payoffFields))

        for (index, cut) in dna.cuts.enumerated() {
            list.append(AnalysisMarker(
                id: "cut-\(cut.frameIndex)", kind: .cut, lane: .cuts,
                startMs: cut.timeMs, endMs: cut.timeMs,
                seekMs: AnalysisSupport.frameMs(cut.frameIndex, fps: fps, fallback: cut.timeMs),
                title: "Cut \(index + 1)",
                fields: [
                    AnalysisField(label: "Time", value: formatMs(cut.timeMs)),
                    AnalysisField(label: "Frame", value: "\(cut.frameIndex)"),
                    AnalysisField(label: "Interval", value: "\(cut.intervalMs) ms"),
                    AnalysisField(label: "Scene score", value: n(cut.sceneScore, 3)),
                ]))
        }

        for shot in dna.shotHistograms {
            var marker = AnalysisMarker(
                id: "shot-\(shot.shotIndex)", kind: .shot, lane: .shots,
                startMs: shot.startMs, endMs: max(shot.endMs, shot.startMs + 1),
                seekMs: AnalysisSupport.frameMs(AnalysisSupport.nearestFrame(shot.startMs, fps: fps), fps: fps, fallback: shot.startMs),
                title: "Shot \(shot.shotIndex + 1)",
                fields: [
                    AnalysisField(label: "Start", value: formatMs(shot.startMs)),
                    AnalysisField(label: "End", value: formatMs(shot.endMs)),
                    AnalysisField(label: "Length", value: "\(shot.endMs - shot.startMs) ms"),
                    AnalysisField(label: "Mean R", value: n(shot.meanR, 1)),
                    AnalysisField(label: "Mean G", value: n(shot.meanG, 1)),
                    AnalysisField(label: "Mean B", value: n(shot.meanB, 1)),
                ])
            marker.rgb = [shot.meanR, shot.meanG, shot.meanB].map { min(1, max(0, $0 / 255)) }
            list.append(marker)
        }

        for gap in dna.silenceGaps {
            list.append(AnalysisMarker(
                id: "gap-\(gap.startMs)", kind: gap.isDeadAir ? .deadAir : .silence, lane: .silence,
                startMs: gap.startMs, endMs: max(gap.endMs, gap.startMs + 1),
                seekMs: gap.startMs,
                title: gap.isDeadAir ? "Dead air" : "Silence",
                fields: [
                    AnalysisField(label: "Start", value: formatMs(gap.startMs)),
                    AnalysisField(label: "End", value: formatMs(gap.endMs)),
                    AnalysisField(label: "Duration", value: "\(gap.durationMs) ms"),
                    AnalysisField(label: "Dead air (> 180 ms)", value: gap.isDeadAir ? "yes" : "no"),
                    AnalysisField(label: "Threshold", value: n(dna.silenceThresholdDb, 1) + " dB"),
                ]))
        }

        for (index, word) in dna.words.enumerated() {
            list.append(AnalysisMarker(
                id: "word-\(index)", kind: .word, lane: .words,
                startMs: word.startMs, endMs: max(word.endMs, word.startMs + 1),
                seekMs: word.startMs,
                title: word.text,
                fields: [
                    AnalysisField(label: "Text", value: word.text),
                    AnalysisField(label: "Start", value: formatMs(word.startMs)),
                    AnalysisField(label: "End", value: formatMs(word.endMs)),
                    AnalysisField(label: "Source", value: dna.asrBackend),
                ]))
        }

        list.append(AnalysisMarker(
            id: "loudness", kind: .loudness, lane: .loudness,
            startMs: 0, endMs: max(1, dna.durationMs), seekMs: 0,
            title: "Integrated loudness",
            fields: [AnalysisField(label: "Integrated", value: n(dna.integratedLufs, 1) + " LUFS")]))

        for event in dna.visualEvents {
            let kind: AnalysisMarker.Kind
            switch event.kind {
            case "punch_in": kind = .punchIn
            case "zoom_out": kind = .zoomOut
            case "overlay_change": kind = .overlayChange
            default: kind = .otherEvent
            }
            var fields: [AnalysisField] = [
                AnalysisField(label: "Kind", value: event.kind),
                AnalysisField(label: "Time", value: formatMs(event.timeMs)),
                AnalysisField(label: "Frame", value: "\(event.frameIndex)"),
            ]
            if let scale = event.scale { fields.append(AnalysisField(label: "Scale", value: n(scale, 3))) }
            if let x = event.centerX, let y = event.centerY {
                fields.append(AnalysisField(label: "Centre", value: "\(n(x, 3)), \(n(y, 3))"))
            }
            if let area = event.area { fields.append(AnalysisField(label: "Area", value: n(area, 3))) }
            list.append(AnalysisMarker(
                id: "event-\(event.id)", kind: kind, lane: .events,
                startMs: event.timeMs, endMs: event.timeMs,
                seekMs: AnalysisSupport.frameMs(event.frameIndex, fps: fps, fallback: event.timeMs),
                title: event.kind.replacingOccurrences(of: "_", with: " ").capitalized,
                fields: fields))
        }

        for (index, curve) in (dna.zoomCurves?.curves ?? []).enumerated() {
            var fields: [AnalysisField] = [
                AnalysisField(label: "Start", value: formatMs(curve.startMs)),
                AnalysisField(label: "End", value: formatMs(curve.endMs)),
                AnalysisField(label: "Scale", value: "\(n(curve.fromScale, 3)) → \(n(curve.toScale, 3))"),
            ]
            if let bezier = curve.bezier, !bezier.isEmpty {
                fields.append(AnalysisField(label: "Bezier", value: bezier.map { n($0, 3) }.joined(separator: ", ")))
            }
            list.append(AnalysisMarker(
                id: "zoom-\(index)", kind: .zoom, lane: .zooms,
                startMs: curve.startMs, endMs: max(curve.endMs, curve.startMs + 1),
                seekMs: AnalysisSupport.frameMs(AnalysisSupport.nearestFrame(curve.startMs, fps: fps), fps: fps, fallback: curve.startMs),
                title: curve.toScale >= curve.fromScale ? "Zoom in" : "Zoom out",
                fields: fields))
        }
        return list
    }
}
