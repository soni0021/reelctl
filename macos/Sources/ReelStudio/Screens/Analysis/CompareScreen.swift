import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// A `diff` job and the draft it compares.
struct CompareRun: Equatable {
    let draft: AssetID
    let job: JobID
}

/// Spec screen 2, Compare: a draft video against one to three outlier DNAs
/// (`diff`), the flags, the cuts-per-window chart, the metrics, and the
/// players side by side.
struct CompareScreen: View {
    @Environment(AppModel.self) private var model
    @State private var draft: AssetID?
    @State private var outliers: [CompareOutlier?] = [nil, nil, nil]
    @State private var outlierVideo: AssetID?
    @State private var run: CompareRun?
    @State private var report: DiffReport?
    @State private var reportSource: String?
    @State private var reportError: String?
    @State private var selectedFlag: String?
    @State private var draftPlayer = FramePlayer()
    @State private var outlierPlayer = FramePlayer()

    init() {}

    private func asset(_ id: AssetID?) -> Asset? {
        guard let id else { return nil }
        return model.assets.first { $0.id == id }
    }

    private var chosenOutliers: [CompareOutlier] { outliers.compactMap { $0 } }

    private var isRunning: Bool {
        guard let run, let job = model.job(run.job) else { return false }
        return !job.status.finished
    }

    var body: some View {
        VStack(spacing: 0) {
            controls.padding(12)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    players
                    results
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .task(id: draft) { await openDraft() }
        .task(id: outlierVideo) { outlierPlayer.load(asset(outlierVideo)?.url) }
        .task(id: run) { await follow(run) }
    }

    // MARK: Controls

    private var controls: some View {
        VStack(alignment: .leading, spacing: 8) {
            AssetPicker(title: "Draft", kind: "video", types: UTType.videoTypes, selection: $draft)
                .dropImport(kind: "video") { imported in draft = imported.id }
            VStack(alignment: .leading, spacing: 6) {
                ForEach(0..<3, id: \.self) { slot in
                    CompareOutlierPicker(slot: slot, selection: $outliers[slot])
                }
            }
            .dropImport(kind: "edit_dna") { imported in place(imported) }
            HStack(spacing: 12) {
                Button("Compare") { compare() }
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(draft == nil || chosenOutliers.isEmpty || isRunning || !model.isConnected)
                Text("Drop a video on Draft, or edit_dna.json files on the outliers.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
            }
            if let run, run.draft == draft {
                JobProgressView(job: run.job)
            }
            if let reportError {
                Text(reportError).font(.callout).foregroundStyle(.red).textSelection(.enabled)
            }
        }
    }

    /// A dropped DNA goes into the first empty slot (or replaces the last).
    private func place(_ imported: Asset) {
        let value = CompareOutlier.asset(imported.id)
        if let empty = outliers.firstIndex(where: { $0 == nil }) {
            outliers[empty] = value
        } else {
            outliers[outliers.count - 1] = value
        }
    }

    // MARK: Players

    private var players: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Draft").font(.headline)
                if asset(draft) != nil {
                    PlayerView(player: draftPlayer.player).frame(height: 300)
                    Text(formatMs(draftPlayer.currentMs)).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                } else {
                    placeholder("Pick or drop a draft video.")
                }
            }
            .frame(maxWidth: .infinity)
            VStack(alignment: .leading, spacing: 4) {
                AssetPicker(title: "Outlier video", kind: "video", types: UTType.videoTypes, selection: $outlierVideo)
                if asset(outlierVideo) != nil {
                    PlayerView(player: outlierPlayer.player).frame(height: 300)
                    Text(formatMs(outlierPlayer.currentMs)).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                } else {
                    placeholder("Optional: an outlier's video, to watch beside the draft.")
                }
            }
            .frame(maxWidth: .infinity)
        }
    }

    private func placeholder(_ text: String) -> some View {
        RoundedRectangle(cornerRadius: 8)
            .fill(Color.gray.opacity(0.08))
            .frame(height: 300)
            .overlay {
                Text(text).font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center).padding()
            }
    }

    // MARK: Results

    @ViewBuilder
    private var results: some View {
        if let report {
            if let reportSource {
                Text(reportSource).font(.caption).foregroundStyle(.secondary)
            }
            HStack(alignment: .top, spacing: 16) {
                CompareFlagList(items: CompareFlagItem.items(report), selected: selectedFlag) { item in
                    selectedFlag = item.id
                    if let ms = item.atMs { draftPlayer.seek(ms: ms) }
                }
                .frame(width: 320)
                if let comparison = report.comparison {
                    VStack(alignment: .leading, spacing: 16) {
                        CompareCutsChart(comparison: comparison)
                        CompareMetricsGrid(comparison: comparison)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    Text("This report has no comparison section (an older reelctl).")
                        .foregroundStyle(.secondary)
                }
            }
        } else if !isRunning {
            ContentUnavailableView(
                "No comparison yet",
                systemImage: "square.split.2x1",
                description: Text("Pick a draft and one to three outliers, then Compare. diff decompiles the draft and compares it with the outliers' edit DNA.")
            )
            .frame(maxHeight: 220)
        }
    }

    // MARK: Actions

    private func resolve(_ outlier: CompareOutlier) async -> InputRef? {
        switch outlier {
        case .asset(let id):
            return .asset(id)
        case .reel(let subject):
            let checkpoint = await AnalysisSupport.latestCheckpoint(model: model, subject: subject, roles: ["out"])
            guard let hash = checkpoint?.outputs["out"] else {
                let name = model.subject(subject)?.name ?? "\(subject)"
                reportError = "\(name) has no edit DNA yet: analyse it in the Library first."
                return nil
            }
            return .hash(hash)
        }
    }

    private func compare() {
        guard let draftID = draft, let draftAsset = asset(draftID) else { return }
        let picked = chosenOutliers
        Task {
            reportError = nil
            var refs: [InputRef] = []
            for outlier in picked.prefix(3) {
                guard let ref = await resolve(outlier) else { return }
                refs.append(ref)
            }
            guard !refs.isEmpty else { return }
            var inputs: [String: InputRef] = ["draft": .asset(draftID)]
            for (index, ref) in refs.enumerated() {
                inputs[index == 0 ? "outlier" : "outlier.\(index + 1)"] = ref
            }
            let name = AnalysisSupport.compareSubjectName(draftAsset)
            guard let subject = await AnalysisSupport.ensureReelSubject(model: model, name: name) else { return }
            let args = AnalysisSupport.whisperArgs()
            let submitted: SubmitResult? = await model.perform {
                try await $0.submit(subject: subject.id, command: "diff", args: args, inputs: inputs)
            }
            guard let submitted else { return }
            await model.reloadJob(submitted.job)
            run = CompareRun(draft: draftID, job: submitted.job)
        }
    }

    /// Load the draft and its newest report, if it was compared before.
    private func openDraft() async {
        report = nil
        reportSource = nil
        reportError = nil
        selectedFlag = nil
        guard let current = asset(draft) else {
            draftPlayer.load(nil)
            return
        }
        draftPlayer.load(current.url)
        guard let subject = AnalysisSupport.reelSubject(model: model, name: AnalysisSupport.compareSubjectName(current)) else { return }
        if let active = AnalysisSupport.activeJob(model: model, subject: subject.id, command: "diff") {
            run = CompareRun(draft: current.id, job: active.id)
            return
        }
        guard let checkpoint = await AnalysisSupport.latestCheckpoint(model: model, subject: subject.id, roles: ["report"]),
              let file = checkpoint.files?["report"],
              draft == current.id
        else { return }
        load(file, source: "The latest comparison of this draft (checkpoint \(checkpoint.id)).")
    }

    private func follow(_ run: CompareRun?) async {
        guard let run else { return }
        guard let job = await model.waitFor(run.job), job.status == .succeeded else { return }
        let file = await model.output(of: run.job, role: "report")
        guard draft == run.draft else { return }
        guard let file else {
            reportError = "The diff job finished without a report."
            return
        }
        load(file, source: "Job \(run.job).")
    }

    private func load(_ file: StoredFile, source: String) {
        do {
            report = try DiffReport.load(file.url)
            reportSource = source
            selectedFlag = nil
        } catch {
            reportError = "Could not read the diff report: \(error.localizedDescription)"
        }
    }
}
