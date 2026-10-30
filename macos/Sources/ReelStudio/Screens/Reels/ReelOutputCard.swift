import AVFoundation
import ReelCore
import SwiftUI

/// One produced reel: the 9:16 video playing, its length, the diff flags
/// against the outliers (click one to seek), Export and Reveal.
struct ReelOutputCard: View {
    @Environment(AppModel.self) private var model
    let reel: ReelRef
    @State private var player = FramePlayer()
    @State private var url: URL?
    @State private var lengthMs: Int64?
    @State private var report: DiffReport?
    @State private var note: String?

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text(reel.name).font(.headline)
                    if let lengthMs { Text(formatMs(lengthMs)).font(.callout.monospacedDigit()).foregroundStyle(.secondary) }
                    Spacer()
                    Button("Export…") { export() }
                        .disabled(url == nil)
                    Button("Reveal") { if let url { serviceReveal([url]) } }
                        .disabled(url == nil)
                }
                if url != nil {
                    PlayerView(player: player.player)
                        .frame(width: 240, height: 426)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                } else if let note {
                    Text(note).foregroundStyle(.secondary)
                } else {
                    ProgressView().controlSize(.small)
                }
                flags
            }
            .padding(6)
        }
        .task(id: reel.renderHash) { await loadRender() }
        .task(id: reel.diffReportHash) { await loadReport() }
    }

    @ViewBuilder
    private var flags: some View {
        if let report, !report.flags.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text("Diff against the outliers").font(.subheadline.weight(.semibold))
                ForEach(report.flags) { flag in
                    Button {
                        player.seek(ms: flag.atMs)
                    } label: {
                        HStack(spacing: 8) {
                            Image(systemName: "flag.fill").foregroundStyle(.orange)
                            Text(MakeReelsReelCard.flagTitle(flag.kind))
                            Text(formatMs(flag.atMs)).monospacedDigit().foregroundStyle(.secondary)
                        }
                    }
                    .buttonStyle(.plain)
                    .help("Seek the reel to this flag")
                }
            }
        }
    }

    private func loadRender() async {
        guard let file = await model.perform({ try await $0.objectPath(reel.renderHash) }) else {
            note = "The render is not in the store."
            return
        }
        url = file
        player.load(file)
        let asset = AVURLAsset(url: Playable.url(for: file))
        if let duration = try? await asset.load(.duration), duration.seconds.isFinite {
            lengthMs = Int64(duration.seconds * 1000)
        }
    }

    private func loadReport() async {
        guard let hash = reel.diffReportHash,
              let file = await model.perform({ try await $0.objectPath(hash) }) else {
            report = nil
            return
        }
        report = try? DiffReport.load(file)
    }

    private func export() {
        guard let url else { return }
        exportVideo(url, suggestedName: "\(reel.name)-#\(reel.renderCheckpoint)", model: model)
    }
}
