import Foundation
import ReelCore
import SwiftUI

/// One reel of a make-reels run: its eight steps, the running one's progress,
/// the rendered reel playing, and its diff flags.
struct MakeReelsReelCard: View {
    @Environment(AppModel.self) private var model
    let reel: ReelJobs
    @State private var player: FramePlayer? = nil
    @State private var report: DiffReport? = nil
    @State private var reportNote: String? = nil

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 12) {
                header
                stepStrip
                if let step = currentStep {
                    JobProgressView(job: step.job)
                }
                if let step = stoppedStep {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Stopped at \(step.step)").font(.callout.weight(.medium)).foregroundStyle(.red)
                        JobProgressView(job: step.job)
                    }
                }
                HStack(alignment: .top, spacing: 16) {
                    if let player {
                        PlayerView(player: player.player)
                            .frame(width: 240, height: 426)
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                    }
                    diffSection
                }
            }
            .padding(6)
        }
        .task {
            for step in reel.jobs where model.job(step.job) == nil {
                await model.reloadJob(step.job)
            }
        }
        .task(id: renderSucceeded) { await loadRender() }
        .task(id: diffSucceeded) { await loadDiff() }
    }

    private var header: some View {
        HStack {
            Text("Reel \(reel.clip)").font(.headline)
            Text(summary).font(.callout).foregroundStyle(.secondary)
            Spacer()
            if !openJobs.isEmpty {
                Button("Cancel reel", role: .destructive) { cancelAll() }
            }
        }
    }

    private var stepStrip: some View {
        HStack(spacing: 6) {
            ForEach(reel.jobs, id: \.job) { step in
                VStack(spacing: 4) {
                    StatusDot(status: status(step.job))
                    Text(step.step)
                        .font(.caption2)
                        .lineLimit(1)
                        .foregroundStyle(step.job == currentStep?.job ? Color.primary : Color.secondary)
                }
                .frame(minWidth: 64)
                .help("\(step.step): \(status(step.job).rawValue) (job \(step.job))")
            }
        }
    }

    @ViewBuilder
    private var diffSection: some View {
        if let report {
            VStack(alignment: .leading, spacing: 6) {
                Text("Diff against the outliers").font(.subheadline.weight(.semibold))
                if report.flags.isEmpty && (report.comparison?.flags ?? []).isEmpty {
                    Text("No flags.").foregroundStyle(.secondary)
                }
                ForEach(report.flags) { flag in
                    Button {
                        player?.seek(ms: flag.atMs)
                    } label: {
                        HStack(spacing: 8) {
                            Image(systemName: "flag.fill").foregroundStyle(.orange)
                            Text(Self.flagTitle(flag.kind))
                            Text(formatMs(flag.atMs)).monospacedDigit().foregroundStyle(.secondary)
                            if let duration = flag.durationMs {
                                Text("for \(formatMs(duration))").monospacedDigit().foregroundStyle(.secondary)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                    .help("Seek the reel to this flag")
                }
                ForEach(Array((report.comparison?.flags ?? []).enumerated()), id: \.offset) { _, flag in
                    HStack(spacing: 8) {
                        Image(systemName: "waveform.path").foregroundStyle(.purple)
                        Text(Self.flagTitle(flag.kind))
                        if let start = flag.windowStartMs, let end = flag.windowEndMs {
                            Text("\(formatMs(start)) to \(formatMs(end))").monospacedDigit().foregroundStyle(.secondary)
                        }
                    }
                }
                Text("The silent bridge card at the end is always flagged as dead air and a static window.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        } else if let reportNote {
            Text(reportNote).font(.callout).foregroundStyle(.secondary)
        }
    }

    // MARK: Derived

    private func status(_ id: JobID) -> JobStatus { model.job(id)?.status ?? .queued }

    private func job(for name: String) -> JobID? { reel.jobs.first { $0.step == name }?.job }

    private var renderSucceeded: Bool { job(for: "render").map { status($0) == .succeeded } ?? false }

    private var diffSucceeded: Bool { job(for: "diff").map { status($0) == .succeeded } ?? false }

    private var currentStep: StepJob? {
        reel.jobs.first { status($0.job) == .running } ?? reel.jobs.first { !status($0.job).finished && status($0.job) != .blocked }
    }

    private var stoppedStep: StepJob? {
        reel.jobs.first { step in
            let value = status(step.job)
            return value == .failed || value == .cancelled || value == .interrupted
        }
    }

    private var openJobs: [JobID] { reel.jobs.map(\.job).filter { !status($0).finished } }

    private var summary: String {
        let done = reel.jobs.filter { status($0.job) == .succeeded }.count
        if stoppedStep != nil { return "failed · \(done) of \(reel.jobs.count) steps" }
        if done == reel.jobs.count { return "done" }
        return "\(done) of \(reel.jobs.count) steps"
    }

    static func flagTitle(_ kind: String) -> String {
        switch kind {
        case "dead_air": return "Dead air"
        case "static_window": return "Static window"
        case "missing_payoff_cue": return "Missing payoff cue"
        case "rhythm_gap": return "Rhythm gap"
        case "pause_density": return "Pause density"
        default: return kind.replacingOccurrences(of: "_", with: " ")
        }
    }

    // MARK: Actions

    private func cancelAll() {
        let ids = openJobs
        Task {
            await model.perform { studio in
                for id in ids.reversed() {
                    _ = try? await studio.cancel(id)
                }
            }
        }
    }

    private func loadRender() async {
        guard renderSucceeded, let id = job(for: "render") else { return }
        guard let file = await model.output(of: id, role: "out") else { return }
        let current = player ?? FramePlayer()
        current.load(Self.playableURL(file, ext: "mp4"))
        player = current
    }

    /// Store objects have no extension, and AVFoundation picks the format from
    /// it, so play through a symbolic link `$TMPDIR/ReelStudio-reels/<hash>.mp4`
    /// to the read-only object (nothing is written into the store).
    static func playableURL(_ file: StoredFile, ext: String) -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ReelStudio-reels", isDirectory: true)
        let link = folder.appendingPathComponent("\(file.hash).\(ext)")
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            if (try? link.checkResourceIsReachable()) == true { return link }
            try? FileManager.default.removeItem(at: link)
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: file.url)
            return link
        } catch {
            return file.url
        }
    }

    private func loadDiff() async {
        guard diffSucceeded, let id = job(for: "diff") else { return }
        guard let file = await model.output(of: id, role: "report") else {
            reportNote = "The diff report is not in the store."
            return
        }
        do {
            report = try DiffReport.load(file.url)
            reportNote = nil
        } catch {
            reportNote = "The diff report could not be read: \(error.localizedDescription)"
        }
    }
}
