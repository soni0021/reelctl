import AVFoundation
import ReelCore
import SwiftUI

/// Reels: every source video with the reels made from it, read from the
/// workspace, so it is still here after the app is reopened (the Make reels
/// panel only shows the run in progress).
struct ReelsOutputScreen: View {
    @Environment(AppModel.self) private var model
    @State private var loader = ReelsOutputLoader()
    @State private var selected: Int64?
    @State private var player = FramePlayer()

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            sourceList
                .frame(minWidth: 220, idealWidth: 260, maxWidth: 320)
            Divider()
            detail
        }
        .navigationTitle("Reels")
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await loader.load(model: model) }
                } label: {
                    Label("Reload", systemImage: "arrow.clockwise")
                }
            }
        }
        .task { await loader.load(model: model) }
        .onChange(of: model.checkpointTick) { _, _ in Task { await loader.load(model: model) } }
        .onChange(of: loader.groups) { _, groups in
            if selected == nil || !groups.contains(where: { $0.id == selected }) { selected = groups.first?.id }
        }
    }

    private var sourceList: some View {
        List(selection: $selected) {
            ForEach(loader.groups) { group in
                VStack(alignment: .leading, spacing: 2) {
                    Text(title(of: group)).lineLimit(2)
                    Text("\(group.reels.count) reel\(group.reels.count == 1 ? "" : "s")")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .tag(group.id)
            }
        }
        .overlay {
            if loader.groups.isEmpty && !loader.loading {
                ContentUnavailableView("No reels yet", systemImage: "film.stack",
                                       description: Text("A reel appears here once a render finishes."))
            }
        }
    }

    @ViewBuilder
    private var detail: some View {
        if let group = loader.groups.first(where: { $0.id == selected }) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    inputVideo(group)
                    Text("Produced reels").font(.title3.bold())
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 300, maximum: 420), spacing: 16, alignment: .top)], alignment: .leading, spacing: 16) {
                        ForEach(group.reels) { reel in
                            ReelOutputCard(reel: reel)
                        }
                    }
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        } else {
            Color.clear
        }
    }

    @ViewBuilder
    private func inputVideo(_ group: SourceGroup) -> some View {
        let asset = group.source.flatMap { id in model.assets.first { $0.id == id } }
        VStack(alignment: .leading, spacing: 6) {
            Text("Input video").font(.title3.bold())
            if let asset {
                Text(asset.name).font(.callout).foregroundStyle(.secondary)
                if FileManager.default.fileExists(atPath: asset.sourcePath) {
                    PlayerView(player: player.player)
                        .frame(minHeight: 260, idealHeight: 340, maxHeight: 400)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .onAppear { player.load(asset.url) }
                        .onChange(of: asset.id) { _, _ in player.load(asset.url) }
                } else {
                    Label("The source file is no longer at \(asset.sourcePath).", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
            } else {
                Label("The source video of these reels is not known.", systemImage: "questionmark.circle")
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func title(of group: SourceGroup) -> String {
        guard let id = group.source else { return "Unknown source" }
        return model.assets.first { $0.id == id }?.name ?? "Video \(id)"
    }
}

/// Loads the grouping from the service: the reel subjects, then each one's
/// checkpoints and jobs.
@MainActor
@Observable
final class ReelsOutputLoader {
    private(set) var groups: [SourceGroup] = []
    private(set) var loading = false

    func load(model: AppModel) async {
        loading = true
        defer { loading = false }
        await model.refreshSubjects()
        let reels = model.subjects(kind: "reel")
        let found: (checkpoints: [SubjectID: [Checkpoint]], jobs: [SubjectID: [Job]])? = await model.perform { studio in
            var checkpoints: [SubjectID: [Checkpoint]] = [:]
            var jobs: [SubjectID: [Job]] = [:]
            for subject in reels {
                checkpoints[subject.id] = try await studio.checkpoints(subject: subject.id)
                jobs[subject.id] = try await studio.jobs(subject: subject.id)
            }
            return (checkpoints, jobs)
        }
        guard let found else { return }
        groups = ReelOutputs.group(subjects: reels, checkpoints: found.checkpoints, jobs: found.jobs)
    }
}
