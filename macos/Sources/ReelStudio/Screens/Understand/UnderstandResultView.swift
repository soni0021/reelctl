import AVKit
import ReelCore
import SwiftUI

/// The right column: the model that answered, the player, the clips and the
/// rejected candidates. A click on a clip plays its range in the source video.
struct UnderstandResultView: View {
    let session: UnderstandSession
    let player: FramePlayer
    let hasVideo: Bool
    let playing: Int?
    let play: (Understanding.Clip) -> Void

    var body: some View {
        if let result = session.result {
            VStack(alignment: .leading, spacing: 0) {
                UnderstandModelHeader(result: result, source: session.resultSource)
                    .padding(12)
                Divider()
                PlayerView(player: player.player)
                    .frame(minHeight: 200, idealHeight: 300, maxHeight: 360)
                if !hasVideo {
                    Text("Pick the source video on the left to play a clip.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 4)
                }
                Divider()
                List {
                    SwiftUI.Section("Clips (\(result.clips.count))") {
                        if result.clips.isEmpty {
                            Text("The model proposed no clip that met the rules.").foregroundStyle(.secondary)
                        }
                        ForEach(result.clips) { clip in
                            Button {
                                play(clip)
                            } label: {
                                UnderstandClipRow(clip: clip, playing: playing == clip.rank)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    if !result.rejected.isEmpty {
                        SwiftUI.Section("Rejected (\(result.rejected.count))") {
                            ForEach(Array(result.rejected.enumerated()), id: \.offset) { pair in
                                UnderstandRejectedRow(rejected: pair.element)
                            }
                        }
                    }
                }
            }
        } else {
            ContentUnavailableView(
                "No understanding yet",
                systemImage: "text.magnifyingglass",
                description: Text("Pick a subject, plan the call, approve its bound, and the clips appear here.")
            )
        }
    }
}

/// Which model answered, through which provider, for how much.
struct UnderstandModelHeader: View {
    let result: Understanding
    let source: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline) {
                Text(result.summary.topic.isEmpty ? "Understanding" : result.summary.topic)
                    .font(.title3.bold())
                    .lineLimit(2)
                Spacer()
                if !source.isEmpty {
                    Text(source).font(.caption).foregroundStyle(.secondary)
                }
            }
            HStack(spacing: 16) {
                headerField("Model", result.model.id)
                headerField("Provider", result.model.provider)
                headerField("Cost", result.model.costUsd.map(formatUsd) ?? "not reported")
                headerField("Bound", formatUsd(result.model.costUpperBoundUsd))
                if !result.summary.language.isEmpty {
                    headerField("Language", result.summary.language)
                }
            }
        }
    }

    private func headerField(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label).font(.caption2).foregroundStyle(.secondary)
            Text(value).font(.callout.monospaced()).textSelection(.enabled)
        }
    }
}

/// One clip: rank, title, its three lines, lengths, flags, why and the open loop.
struct UnderstandClipRow: View {
    let clip: Understanding.Clip
    let playing: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("#\(clip.rank)").font(.headline.monospacedDigit())
                Text(clip.title).font(.headline)
                Spacer()
                if playing {
                    Image(systemName: "play.circle.fill").foregroundStyle(.tint)
                }
                Text("\(formatMs(clip.startMs)) – \(formatMs(clip.endMs))")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            HStack(spacing: 14) {
                Text("Length \(formatMs(clip.lengthMs))")
                Text("Tight \(formatMs(clip.tightLengthMs))")
                if let payoff = clip.payoffAtMs {
                    Text("Payoff at \(formatMs(payoff))")
                }
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
            line("Hook", clip.hook.text)
            line("Payoff", clip.payoff.text)
            line("Last", clip.last.text)
            if !clip.flags.isEmpty {
                Label(clip.flags.joined(separator: " · "), systemImage: "flag.fill")
                    .font(.caption.bold())
                    .foregroundStyle(.orange)
            }
            if !clip.why.isEmpty {
                line("Why", clip.why)
            }
            if !clip.openLoop.isEmpty {
                line("Open loop", clip.openLoop)
            }
            if !clip.visualSuggestion.isEmpty {
                line("Visual", clip.visualSuggestion)
            }
            if !clip.adjustments.isEmpty {
                line("Adjusted", clip.adjustments.joined(separator: "; "))
            }
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .background(playing ? Color.accentColor.opacity(0.10) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
    }

    private func line(_ label: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(label)
                .font(.caption.bold())
                .foregroundStyle(.secondary)
                .frame(width: 64, alignment: .leading)
            Text(text)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct UnderstandRejectedRow: View {
    let rejected: Understanding.Rejected

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text("#\(rejected.rank)").font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                Text(rejected.title).font(.callout)
            }
            Text(rejected.reason)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, 3)
    }
}
