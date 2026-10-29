import Charts
import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// Where an outlier's edit DNA comes from: an imported `edit_dna` file, or the
/// `out` of a `reel` subject's analysis (its head when that holds one).
enum CompareOutlier: Hashable {
    case asset(AssetID)
    case reel(SubjectID)
}

/// One outlier slot: a picker over both sources, and Import….
struct CompareOutlierPicker: View {
    @Environment(AppModel.self) private var model
    let slot: Int
    @Binding var selection: CompareOutlier?

    private var dnaAssets: [Asset] { model.assets.filter { $0.kind == "edit_dna" } }

    private var reels: [Subject] {
        model.subjects(kind: "reel").filter { $0.headCheckpoint != nil && !AnalysisSupport.isCompareSubject($0) }
    }

    var body: some View {
        HStack {
            Picker("Outlier \(slot + 1)", selection: $selection) {
                Text("None").tag(CompareOutlier?.none)
                if !dnaAssets.isEmpty {
                    SwiftUI.Section("edit_dna files") {
                        ForEach(dnaAssets) { asset in
                            Text(asset.name).tag(CompareOutlier?.some(.asset(asset.id)))
                        }
                    }
                }
                if !reels.isEmpty {
                    SwiftUI.Section("Analysed reels (their edit DNA)") {
                        ForEach(reels) { subject in
                            Text(subject.name).tag(CompareOutlier?.some(.reel(subject.id)))
                        }
                    }
                }
            }
            Button("Import…") {
                guard let url = chooseFile([.json], title: "Choose an edit_dna.json") else { return }
                Task {
                    if let asset = await model.importFile(url, kind: "edit_dna") { selection = .asset(asset.id) }
                }
            }
        }
    }
}

/// A row of the flag list.
struct CompareFlagItem: Identifiable, Hashable {
    let id: String
    let symbol: String
    let title: String
    let detail: String
    /// Where a click seeks the draft; nil for a flag over the whole reel.
    let atMs: Int64?

    static func items(_ report: DiffReport) -> [CompareFlagItem] {
        var items: [CompareFlagItem] = []
        for (index, flag) in report.flags.enumerated() {
            let range = "\(formatMs(flag.startMs ?? flag.windowStartMs ?? 0)) – \(formatMs(flag.endMs ?? flag.windowEndMs ?? 0))"
            switch flag.kind {
            case "dead_air":
                items.append(CompareFlagItem(
                    id: "f\(index)", symbol: "speaker.slash", title: "Dead air",
                    detail: "\(range) (\(flag.durationMs ?? 0) ms)", atMs: flag.atMs))
            case "static_window":
                items.append(CompareFlagItem(
                    id: "f\(index)", symbol: "pause.rectangle", title: "No cut or visual event",
                    detail: "\(range) (\(formatMs(flag.durationMs ?? 0)))", atMs: flag.atMs))
            case "missing_payoff_cue":
                items.append(CompareFlagItem(
                    id: "f\(index)", symbol: "flag.slash", title: "Missing payoff cue",
                    detail: "\(range): the outliers cut here, the draft does not", atMs: flag.atMs))
            default:
                items.append(CompareFlagItem(
                    id: "f\(index)", symbol: "flag", title: flag.kind, detail: range, atMs: flag.atMs))
            }
        }
        for (index, flag) in (report.comparison?.flags ?? []).enumerated() {
            switch flag.kind {
            case "rhythm_gap":
                let start = flag.windowStartMs ?? 0
                items.append(CompareFlagItem(
                    id: "c\(index)", symbol: "metronome", title: "Rhythm gap",
                    detail: "\(formatMs(start)) – \(formatMs(flag.windowEndMs ?? start)): most outliers cut here, the draft does not",
                    atMs: start))
            case "pause_density":
                let draft = String(format: "%.1f", flag.draftPerMinute ?? 0)
                let outlier = String(format: "%.1f", flag.outlierPerMinute ?? 0)
                items.append(CompareFlagItem(
                    id: "c\(index)", symbol: "pause.circle", title: "Pause density",
                    detail: "\(draft) pauses per minute against \(outlier) in the outliers", atMs: nil))
            default:
                items.append(CompareFlagItem(
                    id: "c\(index)", symbol: "flag", title: flag.kind, detail: "", atMs: flag.windowStartMs))
            }
        }
        return items
    }
}

/// The flag list; a click on a timed flag seeks the draft.
struct CompareFlagList: View {
    let items: [CompareFlagItem]
    let selected: String?
    let pick: (CompareFlagItem) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Flags").font(.headline)
            if items.isEmpty {
                Label("Nothing flagged.", systemImage: "checkmark.seal").foregroundStyle(.green)
            }
            ForEach(items) { item in
                Button {
                    pick(item)
                } label: {
                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: item.symbol).frame(width: 18).foregroundStyle(.orange)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(item.title).font(.callout.bold())
                            Text(item.detail).font(.caption).foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        Spacer(minLength: 0)
                        if item.atMs != nil {
                            Image(systemName: "play.circle").foregroundStyle(.secondary)
                        }
                    }
                    .padding(6)
                    .background(
                        RoundedRectangle(cornerRadius: 6)
                            .fill(item.id == selected ? Color.accentColor.opacity(0.15) : Color.gray.opacity(0.06))
                    )
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(item.atMs == nil)
            }
        }
    }
}

/// The reels of a comparison, labelled for the chart and the table.
enum CompareSeries {
    static func names(_ comparison: DiffReport.Comparison) -> [String] {
        var names: [String] = ["Draft · " + comparison.draft.name]
        for (index, outlier) in comparison.outliers.enumerated() {
            names.append("Outlier \(index + 1) · " + outlier.name)
        }
        return names
    }
}

/// Cuts per 5 s window: the draft against each outlier.
struct CompareCutsChart: View {
    let comparison: DiffReport.Comparison

    private struct Point: Identifiable {
        let id: String
        let window: String
        let series: String
        let cuts: Int
    }

    private var points: [Point] {
        let names = CompareSeries.names(comparison)
        var points: [Point] = []
        for window in comparison.windows {
            let label = String(format: "%g–%g s", Double(window.startMs) / 1000, Double(window.endMs) / 1000)
            points.append(Point(id: "\(window.startMs)-0", window: label, series: names[0], cuts: window.draftCuts))
            for (index, count) in window.outlierCuts.enumerated() {
                guard let count, index + 1 < names.count else { continue }
                points.append(Point(id: "\(window.startMs)-\(index + 1)", window: label, series: names[index + 1], cuts: count))
            }
        }
        return points
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Cuts per 5 s window").font(.headline)
            Chart(points) { point in
                BarMark(
                    x: .value("Window", point.window),
                    y: .value("Cuts", point.cuts)
                )
                .foregroundStyle(by: .value("Reel", point.series))
                .position(by: .value("Reel", point.series))
            }
            .chartYAxisLabel("cuts")
            .chartLegend(position: .bottom, alignment: .leading)
            .frame(height: 220)
        }
    }
}

/// The metrics of the draft and each outlier, one column per reel.
struct CompareMetricsGrid: View {
    let comparison: DiffReport.Comparison

    private struct Row: Identifiable {
        let label: String
        let value: (DiffReport.Metrics) -> String
        var id: String { label }
    }

    private var rows: [Row] {
        [
            Row(label: "Length") { formatMs($0.durationMs) },
            Row(label: "Cuts") { "\($0.cutCount)" },
            Row(label: "Mean shot") { formatMs($0.meanShotMs) },
            Row(label: "Longest shot") { formatMs($0.longestShotMs) },
            Row(label: "Pauses over 180 ms") { String(format: "%ld (%.1f/min)", $0.deadAirCount, $0.deadAirPerMinute) },
            Row(label: "Silent time") { String(format: "%.0f %%", $0.silentShare * 100) },
            Row(label: "Words per minute") { String(format: "%.1f", $0.wordsPerMinute) },
            Row(label: "Loudness") { String(format: "%.1f LUFS", $0.integratedLufs) },
        ]
    }

    var body: some View {
        let reels: [DiffReport.Metrics] = [comparison.draft] + comparison.outliers
        let names = CompareSeries.names(comparison)
        VStack(alignment: .leading, spacing: 6) {
            Text("Metrics").font(.headline)
            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 6) {
                GridRow {
                    Text("")
                    ForEach(reels.indices, id: \.self) { index in
                        Text(index < names.count ? names[index] : "")
                            .font(.callout.bold())
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .frame(maxWidth: 200, alignment: .leading)
                    }
                }
                Divider()
                ForEach(rows) { row in
                    GridRow {
                        Text(row.label).foregroundStyle(.secondary)
                        ForEach(reels.indices, id: \.self) { index in
                            Text(row.value(reels[index])).font(.callout.monospacedDigit()).textSelection(.enabled)
                        }
                    }
                }
            }
        }
    }
}
