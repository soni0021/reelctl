import ReelCore
import SwiftUI

/// The ⌘K palette: jump to a section or run a main action. Type to filter,
/// the arrow keys move, Return runs, Escape closes.
struct CommandPalette: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var selected: String?
    @FocusState private var searchFocused: Bool

    var body: some View {
        let items = PaletteItem.matching(query)
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Go to a section or run an action", text: $query)
                    .textFieldStyle(.plain)
                    .font(.title3)
                    .focused($searchFocused)
                    .onSubmit { run(selectedItem(in: items)) }
                    .onKeyPress(.downArrow) {
                        move(1, in: items)
                        return .handled
                    }
                    .onKeyPress(.upArrow) {
                        move(-1, in: items)
                        return .handled
                    }
            }
            .padding(12)
            Divider()
            if items.isEmpty {
                Text("No match for \u{201C}\(query)\u{201D}")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(items, selection: $selected) { item in
                    HStack {
                        Label(item.title, systemImage: item.symbol)
                        Spacer()
                        Text(item.detail).font(.caption).foregroundStyle(.secondary)
                    }
                    .tag(item.id)
                }
                .contextMenu(forSelectionType: String.self) { _ in
                    EmptyView()
                } primaryAction: { ids in
                    if let id = ids.first { run(items.first { $0.id == id }) }
                }
            }
            Divider()
            HStack {
                Text("↑↓ to move · Return to run · Esc to close").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Close") { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            .padding(8)
        }
        .frame(width: 520, height: 420)
        .onAppear {
            searchFocused = true
            selected = items.first?.id
        }
        .onChange(of: query) { _, newQuery in
            selected = PaletteItem.matching(newQuery).first?.id
        }
    }

    private func selectedItem(in items: [PaletteItem]) -> PaletteItem? {
        if let selected, let item = items.first(where: { $0.id == selected }) { return item }
        return items.first
    }

    private func move(_ delta: Int, in items: [PaletteItem]) {
        guard !items.isEmpty else { return }
        let current = items.firstIndex { $0.id == selected } ?? (delta > 0 ? -1 : items.count)
        let next = min(max(current + delta, 0), items.count - 1)
        selected = items[next].id
    }

    private func run(_ item: PaletteItem?) {
        guard let item else { return }
        switch item.action {
        case .section(let section):
            model.selection = section
        case .reload:
            Task { await model.refresh() }
        case .importVideo:
            model.selection = .library
        case .onboarding:
            model.showPalette = false
            dismiss()
            // Open the setup sheet after this one has closed.
            Task { @MainActor in
                try? await Task.sleep(nanoseconds: 300_000_000)
                model.showOnboarding = true
            }
            return
        }
        model.showPalette = false
        dismiss()
    }
}

/// What a palette entry does.
enum PaletteAction: Hashable {
    case section(AppSection)
    case reload
    case importVideo
    case onboarding
}

struct PaletteItem: Identifiable, Hashable {
    let id: String
    let title: String
    let symbol: String
    let detail: String
    let action: PaletteAction

    static var all: [PaletteItem] {
        var items: [PaletteItem] = [
            PaletteItem(id: "action.import", title: "Import video…", symbol: "square.and.arrow.down",
                        detail: "Library", action: .importVideo),
            PaletteItem(id: "action.reload", title: "Reload", symbol: "arrow.clockwise",
                        detail: "⌘R", action: .reload),
            PaletteItem(id: "action.settings", title: "Settings and Doctor", symbol: AppSection.settings.symbol,
                        detail: "Action", action: .section(.settings)),
            PaletteItem(id: "action.jobs", title: "Show the job queue", symbol: AppSection.jobs.symbol,
                        detail: "Action", action: .section(.jobs)),
            PaletteItem(id: "action.onboarding", title: "Run first-launch setup again", symbol: "sparkles",
                        detail: "Action", action: .onboarding),
        ]
        for section in AppSection.allCases {
            items.append(PaletteItem(id: "section." + section.rawValue, title: section.title, symbol: section.symbol,
                                     detail: "Go to", action: .section(section)))
        }
        return items
    }

    /// Entries whose title has the query's letters in order (case-insensitive);
    /// a title that starts with or contains the query ranks first.
    static func matching(_ query: String) -> [PaletteItem] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return all }
        let scored: [(PaletteItem, Int)] = all.compactMap { (item: PaletteItem) -> (PaletteItem, Int)? in
            let title = item.title.lowercased()
            if title.hasPrefix(needle) { return (item, 0) }
            if title.contains(needle) { return (item, 1) }
            if isSubsequence(needle, of: title) { return (item, 2) }
            return nil
        }
        return scored.enumerated()
            .sorted { lhs, rhs in
                lhs.element.1 != rhs.element.1 ? lhs.element.1 < rhs.element.1 : lhs.offset < rhs.offset
            }
            .map { $0.element.0 }
    }

    private static func isSubsequence(_ needle: String, of haystack: String) -> Bool {
        var rest = Substring(haystack)
        for character in needle where character != " " {
            guard let index = rest.firstIndex(of: character) else { return false }
            rest = rest[rest.index(after: index)...]
        }
        return true
    }
}
