import ReelCore
import SwiftUI

/// The store's size (spec screen 12) and garbage collection: a dry run first,
/// then a confirmation with the bytes freed, then `store.gc` for real.
struct StorageScreen: View {
    @Environment(AppModel.self) private var model
    @State private var stats: StoreStats?
    @State private var loading = false
    @State private var sortOrder: [KeyPathComparator<StorageSubjectRow>] = [KeyPathComparator(\StorageSubjectRow.bytes, order: .reverse)]
    @State private var dryRun: GarbageReport?
    @State private var confirming = false
    @State private var collecting = false
    @State private var lastResult: String?
    @State private var selectedRow: SubjectID?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            summary
            HStack(alignment: .top, spacing: 16) {
                kinds.frame(width: 260)
                subjectsTable
            }
            .frame(maxHeight: .infinity)
        }
        .padding(16)
        .navigationTitle("Storage")
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await reload() }
                } label: {
                    Label("Reload", systemImage: "arrow.clockwise")
                }
            }
        }
        .task(id: model.checkpointTick) { await reload() }
        .confirmationDialog(
            "Free \(formatBytes(dryRun?.bytes ?? 0))?",
            isPresented: $confirming,
            titleVisibility: .visible
        ) {
            Button("Delete \(dryRun?.objects ?? 0) objects", role: .destructive) {
                Task { await collect() }
            }
            Button("Cancel", role: .cancel) { dryRun = nil }
        } message: {
            Text("These \(dryRun?.objects ?? 0) stored objects are not used by any checkpoint, job input or asset. Deleting them cannot be undone.")
        }
    }

    private var summary: some View {
        HStack(alignment: .firstTextBaseline, spacing: 24) {
            VStack(alignment: .leading) {
                Text(stats.map { formatBytes($0.bytes) } ?? "—").font(.largeTitle.bold())
                Text("\(stats?.objects ?? 0) objects in \(model.workspace.path)")
                    .font(.callout).foregroundStyle(.secondary)
            }
            if loading { ProgressView().controlSize(.small) }
            Spacer()
            VStack(alignment: .trailing, spacing: 4) {
                Button {
                    Task { await startCollection() }
                } label: {
                    Label("Collect garbage…", systemImage: "trash")
                }
                .disabled(collecting || !model.isConnected)
                if let lastResult {
                    Text(lastResult).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    private var kinds: some View {
        let entries: [(key: String, value: UInt64)] = (stats?.byKind ?? [:]).sorted { $0.value > $1.value }
        return GroupBox("By kind") {
            if entries.isEmpty {
                Text("Nothing stored").foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(entries, id: \.key) { entry in
                        HStack {
                            Text(entry.key)
                            Spacer()
                            Text(formatBytes(entry.value)).monospacedDigit().foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
    }

    private var subjectsTable: some View {
        let rows = (stats?.bySubject ?? []).map(StorageSubjectRow.init).sorted(using: sortOrder)
        return GroupBox("By subject") {
            Table(rows, selection: $selectedRow, sortOrder: $sortOrder) {
                TableColumn("Subject", value: \StorageSubjectRow.name) { (row: StorageSubjectRow) in
                    Text(row.name)
                }
                TableColumn("Kind", value: \StorageSubjectRow.kind) { (row: StorageSubjectRow) in
                    Text(ServiceFormat.kindTitle(row.kind)).foregroundStyle(.secondary)
                }
                TableColumn("Size", value: \StorageSubjectRow.bytes) { (row: StorageSubjectRow) in
                    Text(formatBytes(row.bytes)).monospacedDigit()
                }
            }
            .contextMenu(forSelectionType: SubjectID.self) { ids in
                if let id = ids.first {
                    Button("Show History") {
                        model.focusedSubject = id
                        model.selection = .history
                    }
                }
            }
        }
    }

    private func reload() async {
        loading = true
        let fetched: StoreStats? = await model.perform { try await $0.stats() }
        if let fetched { stats = fetched }
        loading = false
    }

    private func startCollection() async {
        collecting = true
        defer { collecting = false }
        let report: GarbageReport? = await model.perform { try await $0.collectGarbage(dryRun: true) }
        guard let report else { return }
        if report.objects == 0 {
            dryRun = nil
            lastResult = "Nothing to collect: every object is in use."
        } else {
            dryRun = report
            confirming = true
        }
    }

    private func collect() async {
        collecting = true
        defer { collecting = false }
        let report: GarbageReport? = await model.perform { try await $0.collectGarbage(dryRun: false) }
        dryRun = nil
        if let report {
            lastResult = "Freed \(formatBytes(report.bytes)) (\(report.objects) objects)."
        }
        await reload()
    }
}

/// A `store.stats` subject row, identifiable for the table.
struct StorageSubjectRow: Identifiable, Hashable {
    let id: SubjectID
    let name: String
    let kind: String
    let bytes: UInt64

    init(_ entry: SubjectBytes) {
        id = entry.subject
        name = entry.name
        kind = entry.kind
        bytes = entry.bytes
    }
}
