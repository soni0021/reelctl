import AVFoundation
import ReelCore
import SwiftUI

/// Understand (paid): plan an `understand` call, approve its bound exactly in
/// `PaidCallSheet` every time, run it, and play the clips it picked
/// (docs/specs/mac-app.md screen 6, ADR-031).
struct UnderstandScreen: View {
    @Environment(AppModel.self) private var model
    @State private var form = UnderstandForm()
    @State private var session = UnderstandSession()
    @State private var player = FramePlayer()
    /// The plan the approval sheet shows; the sheet is open while it is set.
    @State private var approving: UnderstandPlan?
    @State private var playingClip: Int?
    @State private var playTask: Task<Void, Never>?

    init() {}

    var body: some View {
        HStack(spacing: 0) {
            UnderstandFormView(
                form: $form,
                session: session,
                onPlan: plan,
                onSpend: { approving = $0 }
            )
            .frame(width: 420)
            Divider()
            UnderstandResultView(
                session: session,
                player: player,
                hasVideo: videoURL != nil,
                playing: playingClip,
                play: play
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle("Understand")
        .sheet(isPresented: sheetShown) {
            if let plan = approving {
                PaidCallSheet(
                    plan: plan,
                    model: session.plannedModel,
                    approve: {
                        // The yes is for exactly this plan and its bound.
                        approving = nil
                        Task { await session.run(approved: plan, model: model) }
                    },
                    cancel: { approving = nil }
                )
            }
        }
        .onChange(of: form) { old, new in
            if old != new { session.invalidatePlan() }
            if old.dna != new.dna {
                Task { await session.checkDNA(new.dna, model: model) }
            }
            if old.subject != new.subject {
                stopPlaying()
                Task { await session.loadHead(subject: new.subject, model: model) }
            }
        }
        .task {
            await session.checkDNA(form.dna, model: model)
            await session.loadHead(subject: form.subject, model: model)
        }
        .onDisappear {
            stopPlaying()
        }
    }

    private var sheetShown: Binding<Bool> {
        Binding(
            get: { approving != nil },
            set: { shown in if !shown { approving = nil } }
        )
    }

    private var videoURL: URL? {
        guard let id = form.video else { return nil }
        return model.assets.first(where: { $0.id == id })?.url
    }

    private func plan() {
        let current = form
        Task { await session.makePlan(form: current, model: model) }
    }

    private func stopPlaying() {
        playTask?.cancel()
        playTask = nil
        player.player.pause()
        playingClip = nil
    }

    /// Play `[startMs, endMs]` of the source: seek to the start, then pause at
    /// the end through a periodic check of the player's time.
    private func play(_ clip: Understanding.Clip) {
        guard let url = videoURL else {
            model.banner = "Pick the source video to play a clip."
            return
        }
        playTask?.cancel()
        playingClip = clip.rank
        let player = self.player
        let start = clip.startMs
        let end = clip.endMs
        playTask = Task { @MainActor in
            player.player.pause()
            player.load(url)
            var waited = 0
            while player.player.currentItem?.status != .readyToPlay, waited < 100, !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 50_000_000)
                waited += 1
            }
            if Task.isCancelled { return }
            player.seek(ms: start)
            player.player.play()
            // Armed once the player is inside the range, so the time before the
            // seek lands cannot stop it.
            var armed = false
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 100_000_000)
                let now = player.currentMs
                if !armed {
                    if now >= start - 250, now < end { armed = true }
                    continue
                }
                if now >= end {
                    player.player.pause()
                    return
                }
            }
        }
    }
}
