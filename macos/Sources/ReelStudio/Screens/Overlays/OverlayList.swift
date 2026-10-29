import Foundation
import ReelCore
import SwiftUI

/// One overlay as read back from a project's `.otio`.
struct OverlayEntry: Identifiable, Hashable {
    let id: String
    let title: String
    let kind: String
    /// First frame, and one past the last, on the edited timeline.
    let startFrame: Int64
    let endFrame: Int64
    let rate: Double
    let position: String
    let widthPct: Int64?
    let renderer: String

    var startMs: Int64 { Self.ms(startFrame, rate) }
    var endMs: Int64 { Self.ms(endFrame, rate) }

    private static func ms(_ frame: Int64, _ rate: Double) -> Int64 {
        guard rate > 0 else { return 0 }
        return Int64((Double(frame) * 1000 / rate).rounded())
    }
}

/// Reads overlays loosely from the `.otio` JSON: the track marked
/// `metadata.reelctl.overlays: true`, whose clips carry `metadata.reelctl.overlay`
/// (crates/timeline/src/overlay.rs), and captions from the timeline's
/// `metadata.reelctl.captions`. Anything it does not recognise is skipped.
enum OverlayReader {
    static func entries(_ otio: JSONValue) -> [OverlayEntry] {
        var found: [OverlayEntry] = []
        let tracks = otio["tracks"]["children"].array ?? []
        for track in tracks where track["metadata"]["reelctl"]["overlays"].bool == true {
            var at: Int64 = 0
            for child in track["children"].array ?? [] {
                let duration = child["source_range"]["duration"]
                let frames = Int64((duration["value"].double ?? 0).rounded())
                let rate = duration["rate"].double ?? 0
                let meta = child["metadata"]["reelctl"]["overlay"]
                if let id = meta["id"].string {
                    let spec = meta["spec"]
                    found.append(OverlayEntry(
                        id: id,
                        title: spec["title"].string ?? id,
                        kind: spec["kind"].string ?? "chart",
                        startFrame: at,
                        endFrame: at + frames,
                        rate: rate,
                        position: meta["position"].string ?? spec["position"].string ?? "center",
                        widthPct: meta["width_pct"].int ?? spec["width_pct"].int,
                        renderer: meta["renderer"].string ?? ""
                    ))
                }
                at += frames
            }
        }
        return found
    }

    static func hasCaptions(_ otio: JSONValue) -> Bool {
        !otio["metadata"]["reelctl"]["captions"].isNull
    }
}

/// The project's overlays, each with Remove, and the captions with Remove.
struct OverlayListView: View {
    let overlays: [OverlayEntry]
    let hasCaptions: Bool
    let disabled: Bool
    let remove: (OverlayEntry) -> Void
    let removeCaptions: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("On this project").font(.headline)
            if overlays.isEmpty && !hasCaptions {
                Text("No overlays yet.").foregroundStyle(.secondary)
            }
            ForEach(overlays) { overlay in
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Image(systemName: symbol(overlay.kind)).foregroundStyle(.tint)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(overlay.title).font(.body.weight(.medium))
                        Text("\(overlay.kind) · \(formatMs(overlay.startMs)) to \(formatMs(overlay.endMs)) · frames \(overlay.startFrame)..\(overlay.endFrame) · \(overlay.position)\(widthText(overlay))")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Text("id \(overlay.id)\(overlay.renderer.isEmpty ? "" : " · " + overlay.renderer)")
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                    Spacer()
                    Button("Remove", role: .destructive) { remove(overlay) }
                        .disabled(disabled)
                }
                .padding(.vertical, 4)
                Divider()
            }
            if hasCaptions {
                HStack {
                    Image(systemName: "captions.bubble").foregroundStyle(.tint)
                    Text("Word captions")
                    Spacer()
                    Button("Remove captions", role: .destructive, action: removeCaptions)
                        .disabled(disabled)
                }
            }
            Text("A removed overlay's video stays in the store; Undo is History → Revert.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func symbol(_ kind: String) -> String {
        switch kind {
        case "line": return "chart.xyaxis.line"
        case "number-callout": return "number.square"
        default: return "chart.bar"
        }
    }

    private func widthText(_ overlay: OverlayEntry) -> String {
        guard let width = overlay.widthPct else { return "" }
        return " · \(width)% wide"
    }
}
