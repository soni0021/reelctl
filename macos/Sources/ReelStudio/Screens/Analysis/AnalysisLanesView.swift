import ReelCore
import SwiftUI

/// The Library's lanes over the reel's duration, drawn with a Canvas: cuts,
/// shot colours, silence (dead air in red), words, loudness, visual events and
/// zoom curves. A click selects the marker under the pointer (or the time) and
/// reports where to seek; a drag scrubs.
struct AnalysisLanesView: View {
    let dna: EditDNA
    let markers: [AnalysisMarker]
    let playheadMs: Int64
    let selectedID: String?
    /// Horizontal zoom: points per second of video.
    let pointsPerSecond: Double
    /// The marker clicked (nil for empty space) and the ms to seek to.
    let select: (AnalysisMarker?, Int64) -> Void
    let scrub: (Int64) -> Void

    private static let labelWidth: CGFloat = 70

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(AnalysisLane.allCases) { lane in
                    Text(lane.title)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .frame(width: Self.labelWidth, height: lane.height, alignment: .leading)
                }
            }
            GeometryReader { geometry in
                ScrollView(.horizontal, showsIndicators: true) {
                    AnalysisLanesCanvas(
                        dna: dna,
                        markers: markers,
                        playheadMs: playheadMs,
                        selectedID: selectedID,
                        select: select,
                        scrub: scrub
                    )
                    .frame(width: canvasWidth(viewport: geometry.size.width), height: AnalysisLane.totalHeight)
                }
            }
        }
        .frame(height: AnalysisLane.totalHeight + 16)
    }

    private func canvasWidth(viewport: CGFloat) -> CGFloat {
        let seconds = Double(max(1, dna.durationMs)) / 1000
        return max(viewport, CGFloat(seconds * pointsPerSecond))
    }
}

private struct AnalysisLanesCanvas: View {
    let dna: EditDNA
    let markers: [AnalysisMarker]
    let playheadMs: Int64
    let selectedID: String?
    let select: (AnalysisMarker?, Int64) -> Void
    let scrub: (Int64) -> Void
    @State private var scrubbing = false

    var body: some View {
        GeometryReader { geometry in
            let width = geometry.size.width
            Canvas { context, size in
                draw(in: &context, size: size)
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        if abs(value.translation.width) > 3 || abs(value.translation.height) > 3 {
                            scrubbing = true
                        }
                        if scrubbing { scrub(ms(atX: value.location.x, width: width)) }
                    }
                    .onEnded { value in
                        if !scrubbing { click(at: value.location, width: width) }
                        scrubbing = false
                    }
            )
        }
    }

    private var duration: Double { Double(max(1, dna.durationMs)) }

    private func ms(atX x: CGFloat, width: CGFloat) -> Int64 {
        guard width > 0 else { return 0 }
        let fraction = min(1, max(0, Double(x / width)))
        return Int64(fraction * duration)
    }

    private func click(at point: CGPoint, width: CGFloat) {
        let clickedMs = ms(atX: point.x, width: width)
        guard width > 0, let lane = AnalysisLane.lane(atY: point.y), lane != .ruler else {
            select(nil, clickedMs)
            return
        }
        let scale = width / CGFloat(duration)
        var bestPoint: AnalysisMarker?
        var bestPointDistance = CGFloat.infinity
        var bestRange: AnalysisMarker?
        var bestRangeDistance = CGFloat.infinity
        for marker in markers where marker.lane == lane {
            let x0 = CGFloat(marker.startMs) * scale
            let x1 = CGFloat(marker.endMs) * scale
            let distance: CGFloat = point.x < x0 ? x0 - point.x : (point.x > x1 ? point.x - x1 : 0)
            if marker.isPoint {
                if distance <= 6, distance < bestPointDistance {
                    bestPoint = marker
                    bestPointDistance = distance
                }
            } else if distance <= 3, distance < bestRangeDistance {
                bestRange = marker
                bestRangeDistance = distance
            }
        }
        guard let hit = bestPoint ?? bestRange else {
            select(nil, clickedMs)
            return
        }
        select(hit, hit.kind == .loudness ? clickedMs : hit.seekMs)
    }

    // MARK: Drawing

    private func draw(in context: inout GraphicsContext, size: CGSize) {
        let width = size.width
        let scale = width / CGFloat(duration)
        func x(_ ms: Int64) -> CGFloat { CGFloat(ms) * scale }

        for lane in AnalysisLane.allCases where lane.rawValue % 2 == 1 {
            let rect = CGRect(x: 0, y: lane.top, width: width, height: lane.height)
            context.fill(Path(rect), with: .color(Color.gray.opacity(0.08)))
        }
        drawRuler(in: &context, width: width, scale: scale)

        // Ranges first, points on top.
        let ordered = markers.filter { !$0.isPoint } + markers.filter { $0.isPoint }
        var lastWordEnd: CGFloat = -CGFloat.infinity
        for marker in ordered {
            let lane = marker.lane
            let x0 = x(marker.startMs)
            let x1 = max(x0 + 1, x(marker.endMs))
            let top = lane.top
            let height = lane.height
            let rect = CGRect(x: x0, y: top + 2, width: x1 - x0, height: height - 4)
            switch marker.kind {
            case .cut:
                var path = Path()
                path.move(to: CGPoint(x: x0, y: top))
                path.addLine(to: CGPoint(x: x0, y: top + height))
                context.stroke(path, with: .color(marker.color), lineWidth: 2)
            case .payoff:
                context.fill(Path(rect), with: .color(marker.color.opacity(0.25)))
            case .shot:
                context.fill(Path(rect), with: .color(marker.color))
                var edge = Path()
                edge.move(to: CGPoint(x: x0, y: top + 2))
                edge.addLine(to: CGPoint(x: x0, y: top + height - 2))
                context.stroke(edge, with: .color(Color.black.opacity(0.4)), lineWidth: 1)
            case .silence:
                context.fill(Path(rect), with: .color(Color.gray.opacity(0.45)))
            case .deadAir:
                context.fill(Path(rect), with: .color(Color.red.opacity(0.75)))
            case .word:
                var tick = Path()
                tick.move(to: CGPoint(x: x0, y: top + 2))
                tick.addLine(to: CGPoint(x: x0, y: top + height - 2))
                context.stroke(tick, with: .color(Color.orange.opacity(0.8)), lineWidth: 1)
                if x0 + 2 > lastWordEnd {
                    let text = context.resolve(Text(marker.title).font(.system(size: 10)))
                    let measured = text.measure(in: CGSize(width: 300, height: height))
                    context.draw(text, at: CGPoint(x: x0 + 2, y: top + height / 2), anchor: .leading)
                    lastWordEnd = x0 + 2 + measured.width + 3
                }
            case .loudness:
                drawLoudness(in: &context, rect: CGRect(x: 0, y: top, width: width, height: height))
            case .punchIn, .zoomOut, .overlayChange, .otherEvent:
                drawEvent(marker, in: &context, x: x0, top: top, height: height)
            case .zoom:
                let bar = CGRect(x: x0, y: top + 4, width: x1 - x0, height: height - 8)
                context.fill(Path(roundedRect: bar, cornerRadius: 3), with: .color(marker.color.opacity(0.6)))
            }
            if marker.id == selectedID {
                let outline = marker.isPoint
                    ? CGRect(x: x0 - 5, y: top + 1, width: 10, height: height - 2)
                    : CGRect(x: x0, y: top + 1, width: x1 - x0, height: height - 2)
                context.stroke(Path(roundedRect: outline, cornerRadius: 2), with: .color(Color.accentColor), lineWidth: 2)
            }
        }

        // The playhead.
        let px = x(playheadMs)
        var playhead = Path()
        playhead.move(to: CGPoint(x: px, y: 0))
        playhead.addLine(to: CGPoint(x: px, y: size.height))
        context.stroke(playhead, with: .color(Color.accentColor), lineWidth: 1.5)
    }

    private func drawRuler(in context: inout GraphicsContext, width: CGFloat, scale: CGFloat) {
        let pointsPerSecond = scale * 1000
        let step: Int64 = pointsPerSecond >= 40 ? 1000 : (pointsPerSecond >= 10 ? 5000 : 10000)
        var ms: Int64 = 0
        let lane = AnalysisLane.ruler
        while Double(ms) <= duration {
            let x = CGFloat(ms) * scale
            var tick = Path()
            tick.move(to: CGPoint(x: x, y: lane.top + lane.height - 5))
            tick.addLine(to: CGPoint(x: x, y: lane.top + lane.height))
            context.stroke(tick, with: .color(Color.secondary), lineWidth: 1)
            let label = context.resolve(Text("\(ms / 1000)s").font(.system(size: 9)).foregroundStyle(Color.secondary))
            context.draw(label, at: CGPoint(x: x + 2, y: lane.top + 1), anchor: .topLeading)
            ms += step
        }
    }

    private func drawLoudness(in context: inout GraphicsContext, rect: CGRect) {
        // A bar whose height follows the integrated value over -60...0 LUFS.
        let lufs = dna.integratedLufs
        let fraction = CGFloat(min(1, max(0, (lufs + 60) / 60)))
        let barHeight = max(2, (rect.height - 4) * fraction)
        let bar = CGRect(x: rect.minX, y: rect.maxY - 2 - barHeight, width: rect.width, height: barHeight)
        context.fill(Path(bar), with: .color(Color.blue.opacity(0.25)))
        let label = context.resolve(
            Text(String(format: "%.1f LUFS integrated", lufs)).font(.system(size: 10, weight: .medium))
        )
        context.draw(label, at: CGPoint(x: rect.minX + 4, y: rect.midY), anchor: .leading)
    }

    private func drawEvent(_ marker: AnalysisMarker, in context: inout GraphicsContext, x: CGFloat, top: CGFloat, height: CGFloat) {
        var path = Path()
        let mid = top + height / 2
        switch marker.kind {
        case .punchIn:
            // Pointing up: the picture grows.
            path.move(to: CGPoint(x: x, y: top + 3))
            path.addLine(to: CGPoint(x: x + 5, y: top + height - 3))
            path.addLine(to: CGPoint(x: x - 5, y: top + height - 3))
        case .zoomOut:
            path.move(to: CGPoint(x: x, y: top + height - 3))
            path.addLine(to: CGPoint(x: x + 5, y: top + 3))
            path.addLine(to: CGPoint(x: x - 5, y: top + 3))
        default:
            path.move(to: CGPoint(x: x, y: top + 3))
            path.addLine(to: CGPoint(x: x + 5, y: mid))
            path.addLine(to: CGPoint(x: x, y: top + height - 3))
            path.addLine(to: CGPoint(x: x - 5, y: mid))
        }
        path.closeSubpath()
        context.fill(path, with: .color(marker.color))
    }
}
