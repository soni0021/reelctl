import AppKit
import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// A decompile's outputs, read from the store.
struct LibraryAnalysis {
    let dna: EditDNA
    let markers: [AnalysisMarker]
    let thumbs: [AnalysisThumb]
    let checkpoint: CheckpointID
}

/// A decompile job and the video it is for.
struct LibraryRun: Equatable {
    let asset: AssetID
    let job: JobID
}

/// Spec screen 1, Reel analysis: imported videos, `decompile`, and the
/// player over the lanes, the filmstrip and the inspector.
struct LibraryScreen: View {
    @Environment(AppModel.self) private var model
    @State private var selectedAsset: AssetID?
    @State private var run: LibraryRun?
    @State private var analysis: LibraryAnalysis?
    @State private var loadError: String?
    @State private var loading = false
    @State private var selectedMarker: AnalysisMarker?
    @State private var showInspector = true
    @State private var pointsPerSecond: Double = 80
    @State private var player = FramePlayer()

    init() {}

    private var videos: [Asset] {
        model.assets.filter { $0.kind == "video" }.sorted { $0.createdAt > $1.createdAt }
    }

    private var asset: Asset? {
        guard let selectedAsset else { return nil }
        return model.assets.first { $0.id == selectedAsset }
    }

    var body: some View {
        HStack(spacing: 0) {
            sidebar.frame(width: 250)
            Divider()
            detail.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .dropImport(kind: "video") { imported in selectedAsset = imported.id }
        .inspector(isPresented: $showInspector) {
            AnalysisInspector(asset: asset, dna: analysis?.dna, marker: selectedMarker, checkpoint: analysis?.checkpoint)
                .inspectorColumnWidth(min: 240, ideal: 300, max: 440)
        }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    showInspector.toggle()
                } label: {
                    Label("Inspector", systemImage: "sidebar.right")
                }
                .help("Show or hide the inspector")
            }
        }
        .task(id: selectedAsset) { await openSelected() }
        .task(id: run) { await follow(run) }
    }

    // MARK: Sidebar

    private var sidebar: some View {
        VStack(spacing: 0) {
            List(selection: $selectedAsset) {
                SwiftUI.Section("Videos") {
                    ForEach(videos) { video in
                        LibraryAssetRow(asset: video).tag(video.id)
                    }
                }
            }
            .overlay {
                if videos.isEmpty {
                    Text("Drop a video here, or import one.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .padding()
                }
            }
            Divider()
            HStack {
                Button("Import video…") { importVideo() }
                Spacer()
            }
            .padding(8)
        }
    }

    private func importVideo() {
        guard let url = chooseFile(UTType.videoTypes, title: "Import a video") else { return }
        Task {
            if let imported = await model.importFile(url, kind: "video") { selectedAsset = imported.id }
        }
    }

    // MARK: Detail

    @ViewBuilder
    private var detail: some View {
        if let asset {
            VStack(alignment: .leading, spacing: 8) {
                header(asset)
                PlayerView(player: player.player)
                    .frame(minHeight: 220, maxHeight: .infinity)
                if let analysis {
                    lanesToolbar(analysis.dna)
                    AnalysisLanesView(
                        dna: analysis.dna,
                        markers: analysis.markers,
                        playheadMs: player.currentMs,
                        selectedID: selectedMarker?.id,
                        pointsPerSecond: pointsPerSecond,
                        select: { marker, ms in
                            selectedMarker = marker
                            player.seek(ms: ms)
                        },
                        scrub: { ms in player.seek(ms: ms) }
                    )
                    if analysis.thumbs.isEmpty {
                        Text("No timeline_view pictures in this analysis.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else {
                        AnalysisFilmstripRow(thumbs: analysis.thumbs, selectedFrame: selectedCutFrame) { index, thumb in
                            pickThumb(index, thumb, in: analysis)
                        }
                    }
                } else if loading {
                    ProgressView("Reading the analysis…").frame(maxWidth: .infinity)
                } else if run?.asset != asset.id {
                    ContentUnavailableView(
                        "Not analysed yet",
                        systemImage: "waveform.badge.magnifyingglass",
                        description: Text("Analyse runs decompile on this video: cuts, shot colours, silence, words, loudness, visual events and zoom curves.")
                    )
                    .frame(maxHeight: 220)
                }
            }
            .padding(12)
        } else {
            ContentUnavailableView {
                Label("No video selected", systemImage: "film")
            } description: {
                Text("Drop a video anywhere on this screen, or import one.")
            } actions: {
                Button("Import video…") { importVideo() }
            }
        }
    }

    private func header(_ asset: Asset) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(asset.name).font(.title3.bold())
                    Text(asset.sourcePath).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
                Spacer()
                if !AnalysisSupport.whisperArgs().isEmpty {
                    Label("whisper", systemImage: "text.bubble").font(.caption).foregroundStyle(.secondary)
                        .help("Words come from the whisper model set in Settings.")
                }
                Button(analysis == nil ? "Analyse" : "Analyse again") { analyse(asset) }
                    .disabled(!model.isConnected || isRunning(asset))
            }
            if let loadError {
                Text(loadError).font(.callout).foregroundStyle(.red).textSelection(.enabled)
            }
            if let run, run.asset == asset.id {
                JobProgressView(job: run.job)
            }
        }
    }

    private func lanesToolbar(_ dna: EditDNA) -> some View {
        HStack(spacing: 12) {
            Text(formatMs(player.currentMs)).font(.callout.monospacedDigit())
            Text("/ \(formatMs(dna.durationMs))").font(.callout.monospacedDigit()).foregroundStyle(.secondary)
            Divider().frame(height: 14)
            Text("Loudness").font(.caption).foregroundStyle(.secondary)
            ProgressView(value: min(1, max(0, (dna.integratedLufs + 60) / 60)))
                .frame(width: 100)
                .help("Integrated loudness on a -60…0 LUFS scale")
            Text(String(format: "%.1f LUFS", dna.integratedLufs)).font(.caption.monospacedDigit())
            Text("\(dna.cuts.count) cuts · \(dna.silenceGaps.filter { $0.isDeadAir }.count) dead air · \(dna.visualEvents.count) events")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
            Image(systemName: "minus.magnifyingglass").foregroundStyle(.secondary)
            Slider(value: $pointsPerSecond, in: 20...400).frame(width: 140)
            Image(systemName: "plus.magnifyingglass").foregroundStyle(.secondary)
        }
    }

    private var selectedCutFrame: Int64? {
        guard let marker = selectedMarker, marker.kind == .cut else { return nil }
        return Int64(marker.id.dropFirst(4))
    }

    private func pickThumb(_ index: Int, _ thumb: AnalysisThumb, in analysis: LibraryAnalysis) {
        if let cut = AnalysisFilmstrip.cut(for: thumb, at: index, in: analysis.dna) {
            selectedMarker = analysis.markers.first { $0.id == "cut-\(cut.frameIndex)" }
        }
        if let ms = AnalysisFilmstrip.seekMs(for: thumb, at: index, in: analysis.dna) {
            player.seek(ms: ms)
        }
    }

    // MARK: Actions

    private func isRunning(_ asset: Asset) -> Bool {
        if let run, run.asset == asset.id, let job = model.job(run.job), !job.status.finished { return true }
        return false
    }

    private func analyse(_ asset: Asset) {
        Task {
            guard let subject = await AnalysisSupport.ensureReelSubject(model: model, name: asset.name) else { return }
            let args = AnalysisSupport.whisperArgs()
            let inputs: [String: InputRef] = ["input": .asset(asset.id)]
            let submitted: SubmitResult? = await model.perform {
                try await $0.submit(subject: subject.id, command: "decompile", args: args, inputs: inputs)
            }
            guard let submitted else { return }
            await model.reloadJob(submitted.job)
            loadError = nil
            run = LibraryRun(asset: asset.id, job: submitted.job)
        }
    }

    /// Show the selected video and its newest decompile, or follow the one running.
    private func openSelected() async {
        selectedMarker = nil
        analysis = nil
        loadError = nil
        guard let current = asset else {
            player.load(nil)
            return
        }
        player.load(current.url)
        guard let subject = AnalysisSupport.reelSubject(model: model, name: current.name) else { return }
        if let active = AnalysisSupport.activeJob(model: model, subject: subject.id, command: "decompile") {
            run = LibraryRun(asset: current.id, job: active.id)
            return
        }
        loading = true
        let checkpoint = await AnalysisSupport.latestCheckpoint(model: model, subject: subject.id, roles: ["out"])
        loading = false
        guard let checkpoint, selectedAsset == current.id else { return }
        await show(checkpoint, for: current.id)
    }

    /// Wait for a decompile, then show its outputs.
    private func follow(_ run: LibraryRun?) async {
        guard let run else { return }
        guard let job = await model.waitFor(run.job), job.status == .succeeded else { return }
        guard let checkpointID = job.checkpoint?.id else {
            loadError = "The job finished without a checkpoint."
            return
        }
        let checkpoint: Checkpoint? = await model.perform { try await $0.checkpoint(checkpointID) }
        guard let checkpoint, selectedAsset == run.asset else { return }
        await show(checkpoint, for: run.asset)
    }

    private func show(_ checkpoint: Checkpoint, for assetID: AssetID) async {
        guard let out = checkpoint.files?["out"] else {
            loadError = "Checkpoint \(checkpoint.id) has no edit_dna.json."
            return
        }
        let dna: EditDNA
        do {
            dna = try EditDNA.load(out.url)
        } catch {
            loadError = "Could not read edit_dna.json: \(error.localizedDescription)"
            return
        }
        var thumbs: [AnalysisThumb] = []
        if let views = checkpoint.files?["views"] {
            thumbs = await AnalysisFilmstrip.load(model: model, views: views)
        }
        guard selectedAsset == assetID else { return }
        analysis = LibraryAnalysis(dna: dna, markers: AnalysisMarkers.build(dna), thumbs: thumbs, checkpoint: checkpoint.id)
    }
}

/// A video in the Library list: its name and whether it has been analysed.
private struct LibraryAssetRow: View {
    @Environment(AppModel.self) private var model
    let asset: Asset

    var body: some View {
        let subject = AnalysisSupport.reelSubject(model: model, name: asset.name)
        let active = subject.flatMap { AnalysisSupport.activeJob(model: model, subject: $0.id, command: "decompile") }
        HStack(spacing: 8) {
            Image(systemName: "film").foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(asset.name).lineLimit(1)
                Text(status(subject: subject, active: active)).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if let active { StatusDot(status: active.status) }
        }
    }

    private func status(subject: Subject?, active: Job?) -> String {
        if active != nil { return "analysing…" }
        if subject?.headCheckpoint != nil { return "analysed" }
        return formatBytes(asset.size)
    }
}
