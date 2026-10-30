import AppKit
import ReelCore
import SwiftUI

@main
struct ReelStudioApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup("ReelStudio") {
            RootView()
                .environment(model)
                .frame(minWidth: 1100, minHeight: 700)
                .task {
                    delegate.model = model
                    if model.connection == .disconnected { await model.connect() }
                }
        }
        .commands {
            CommandGroup(after: .newItem) {
                Button("Command Palette…") { model.showPalette = true }
                    .keyboardShortcut("k", modifiers: .command)
                Button("Reload") { Task { await model.refresh() } }
                    .keyboardShortcut("r", modifiers: .command)
            }
        }
    }
}

/// Sends `shutdown` on quit when no job runs (the spec's stopping rule).
final class AppDelegate: NSObject, NSApplicationDelegate {
    @MainActor var model: AppModel?

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let model = MainActor.assumeIsolated({ self.model }) else { return .terminateNow }
        Task { @MainActor in
            await model.quit()
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        NavigationSplitView {
            List(selection: $model.selection) {
                SwiftUI.Section("Work") {
                    ForEach(AppSection.work) { section in
                        Label(section.title, systemImage: section.symbol).tag(section)
                    }
                }
                SwiftUI.Section("Service") {
                    ForEach(AppSection.service) { section in
                        Label(section.title, systemImage: section.symbol).tag(section)
                    }
                }
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 200)
            .safeAreaInset(edge: .bottom) { ConnectionBadge().padding(8) }
        } detail: {
            VStack(spacing: 0) {
                if let banner = model.banner {
                    BannerView(text: banner) { model.banner = nil }
                }
                screen(for: model.selection ?? .library)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .sheet(isPresented: $model.showPalette) { CommandPalette() }
        .sheet(isPresented: $model.showOnboarding) { OnboardingView() }
    }

    @ViewBuilder
    private func screen(for section: AppSection) -> some View {
        if case .failed(let reason) = model.connection, section != .settings {
            ContentUnavailableView {
                Label("Not connected", systemImage: "bolt.horizontal.circle")
            } description: {
                Text(reason)
            } actions: {
                Button("Try again") { Task { await model.connect() } }
                Button("Settings") { model.selection = .settings }
            }
        } else {
            switch section {
            case .library: LibraryScreen()
            case .makeReels: MakeReelsScreen()
            case .reels: ReelsOutputScreen()
            case .projects: ProjectsScreen()
            case .compare: CompareScreen()
            case .look: LookScreen()
            case .presenter: PresenterScreen()
            case .understand: UnderstandScreen()
            case .datasets: DatasetsScreen()
            case .overlays: OverlaysScreen()
            case .jobs: JobsScreen()
            case .history: HistoryScreen()
            case .storage: StorageScreen()
            case .settings: SettingsScreen()
            }
        }
    }
}

struct ConnectionBadge: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(color).frame(width: 8, height: 8)
            Text(text).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            Spacer()
        }
    }

    private var color: Color {
        switch model.connection {
        case .connected: return .green
        case .connecting: return .yellow
        case .disconnected, .failed: return .red
        }
    }

    private var text: String {
        switch model.connection {
        case .connected: return "reelctl \(model.info?.version ?? "") · \(model.workspace.lastPathComponent)"
        case .connecting: return "Connecting…"
        case .disconnected: return "Not connected"
        case .failed: return "Not connected"
        }
    }
}

struct BannerView: View {
    let text: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .top) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            Text(text).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
            Button(action: dismiss) { Image(systemName: "xmark") }.buttonStyle(.borderless)
        }
        .padding(10)
        .background(.orange.opacity(0.12))
    }
}
