import ReelCore
import SwiftUI

/// The project's lanes, blocks proportional to frames. Dragging across the
/// lanes selects a range (it fills a punch-in's start and end).
struct ProjectTrackView: View {
    let lanes: ProjectLanes
    @Binding var selection: ProjectFrameRange?
    @State private var zoom: Double = 1

    private let labelWidth: CGFloat = 78
    private let spacing: CGFloat = 4

    private var contentHeight: CGFloat {
        let heights = lanes.lanes.reduce(CGFloat(0)) { $0 + CGFloat($1.height) }
        return heights + spacing * CGFloat(max(0, lanes.lanes.count - 1))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            header
            HStack(alignment: .top, spacing: 6) {
                VStack(alignment: .trailing, spacing: spacing) {
                    ForEach(lanes.lanes) { lane in
                        Text(lane.title)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .frame(width: labelWidth, height: CGFloat(lane.height), alignment: .trailing)
                    }
                }
                GeometryReader { geometry in
                    let width = max(geometry.size.width, geometry.size.width * CGFloat(zoom))
                    let total = max(lanes.totalFrames, 1)
                    let scale = width / CGFloat(total)
                    ScrollView(.horizontal) {
                        ZStack(alignment: .topLeading) {
                            VStack(alignment: .leading, spacing: spacing) {
                                ForEach(lanes.lanes) { lane in
                                    laneView(lane, scale: scale, width: width)
                                }
                            }
                            selectionMark(scale: scale)
                        }
                        .frame(width: width, height: contentHeight, alignment: .topLeading)
                        .contentShape(Rectangle())
                        .gesture(dragGesture(scale: scale, total: total))
                    }
                }
                .frame(height: contentHeight + 16)
            }
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Text("\(lanes.totalFrames) frames · \(formatMs(lanes.ms(lanes.totalFrames))) · \(String(format: "%g", lanes.rate)) fps")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            if let selection, selection.frames > 0 {
                Text("Selected \(formatMs(lanes.ms(selection.start)))–\(formatMs(lanes.ms(selection.end))) (\(selection.frames) frames)")
                    .font(.caption.monospacedDigit())
                Button("Clear") { self.selection = nil }
                    .buttonStyle(.borderless)
                    .font(.caption)
            } else {
                Text("Drag across the track to select a range.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            Spacer()
            Image(systemName: "minus.magnifyingglass").foregroundStyle(.secondary)
            Slider(value: $zoom, in: 1...24)
                .frame(width: 140)
                .help("Zoom the track")
            Image(systemName: "plus.magnifyingglass").foregroundStyle(.secondary)
        }
    }

    private func laneView(_ lane: ProjectLane, scale: CGFloat, width: CGFloat) -> some View {
        let height = CGFloat(lane.height)
        return ZStack(alignment: .topLeading) {
            Rectangle()
                .fill(Color.secondary.opacity(0.06))
                .frame(width: width, height: height)
            ForEach(lane.spans) { span in
                spanView(span, height: height, scale: scale)
            }
            ForEach(Array(lane.ticks.enumerated()), id: \.offset) { item in
                Rectangle()
                    .fill(Color.green)
                    .frame(width: 1.5, height: height)
                    .offset(x: CGFloat(item.element) * scale)
            }
        }
        .frame(width: width, height: height, alignment: .topLeading)
        .clipped()
    }

    private func spanView(_ span: ProjectLaneSpan, height: CGFloat, scale: CGFloat) -> some View {
        let blockWidth = max(1, CGFloat(span.range.frames) * scale)
        return RoundedRectangle(cornerRadius: 3)
            .fill(Self.colour(span.style))
            .overlay {
                RoundedRectangle(cornerRadius: 3)
                    .strokeBorder(Self.colour(span.style).opacity(0.9), lineWidth: span.style == .gap ? 1 : 0)
            }
            .overlay(alignment: .leading) {
                if blockWidth > 24 {
                    Text(span.title)
                        .font(.caption2)
                        .lineLimit(1)
                        .padding(.horizontal, 3)
                        .frame(width: blockWidth, alignment: .leading)
                        .clipped()
                }
            }
            .frame(width: blockWidth, height: height)
            .offset(x: CGFloat(span.range.start) * scale)
            .help("\(span.detail)\n\(formatMs(lanes.ms(span.range.start)))–\(formatMs(lanes.ms(span.range.end)))")
    }

    @ViewBuilder
    private func selectionMark(scale: CGFloat) -> some View {
        if let selection, selection.frames > 0 {
            Rectangle()
                .fill(Color.accentColor.opacity(0.18))
                .overlay {
                    Rectangle().strokeBorder(Color.accentColor, lineWidth: 1)
                }
                .frame(width: max(1, CGFloat(selection.frames) * scale), height: contentHeight)
                .offset(x: CGFloat(selection.start) * scale)
                .allowsHitTesting(false)
        }
    }

    private func dragGesture(scale: CGFloat, total: Int64) -> some Gesture {
        DragGesture(minimumDistance: 3)
            .onChanged { value in
                let a = frame(at: value.startLocation.x, scale: scale, total: total)
                let b = frame(at: value.location.x, scale: scale, total: total)
                selection = ProjectFrameRange(start: min(a, b), end: max(a, b))
            }
    }

    private func frame(at x: CGFloat, scale: CGFloat, total: Int64) -> Int64 {
        guard scale > 0 else { return 0 }
        let frame = Int64((x / scale).rounded())
        return min(max(0, frame), total)
    }

    static func colour(_ style: ProjectSpanStyle) -> Color {
        switch style {
        case .clip: return Color.blue.opacity(0.45)
        case .gap: return Color.gray.opacity(0.18)
        case .card: return Color.orange.opacity(0.65)
        case .punch: return Color.purple.opacity(0.55)
        case .caption: return Color.teal.opacity(0.55)
        case .chart: return Color.pink.opacity(0.55)
        case .reframe: return Color.green.opacity(0.25)
        }
    }
}
