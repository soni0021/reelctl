import AppKit
import Foundation
import ReelCore
import SwiftUI

// Helpers for the Service screens (Jobs, History, Storage, Settings,
// Onboarding, the palette). Names start with `Service` so they do not clash
// with the other screens' files in the same module.

enum ServiceFormat {
    /// A catalogue time (milliseconds since 1970) as a short date and time.
    static func time(_ ms: Int64?) -> String {
        guard let ms else { return "—" }
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        return date.formatted(date: .abbreviated, time: .standard)
    }

    /// Only the clock time, for a row.
    static func clock(_ ms: Int64?) -> String {
        guard let ms else { return "—" }
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        return date.formatted(date: .omitted, time: .standard)
    }

    /// How long a job ran, when it started and finished.
    static func duration(from start: Int64?, to end: Int64?) -> String? {
        guard let start, let end, end >= start else { return nil }
        return formatMs(end - start)
    }

    /// Indented JSON with sorted keys.
    static func pretty(_ value: JSONValue) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        guard let data = try? encoder.encode(value) else { return value.text }
        return String(decoding: data, as: UTF8.self)
    }

    /// One line for a job event (`stage`, `progress`, `warning`, `error`,
    /// `result`, `log`, or anything else as compact JSON).
    static func eventLine(_ event: JSONValue) -> String {
        let kind = event["event"].string ?? ""
        switch kind {
        case "stage":
            return "stage: " + (event["name"].string ?? event.text)
        case "progress":
            let stage = event["stage"].string ?? ""
            let done = event["done"].int.map(String.init) ?? "?"
            if let total = event["total"].int { return "progress \(stage): \(done)/\(total)" }
            return "progress \(stage): \(done)"
        case "warning":
            return "warning: " + (event["message"].string ?? event.text)
        case "error":
            let code = event["code"].int.map { " (\($0))" } ?? ""
            return "error\(code): " + (event["message"].string ?? event.text)
        case "result":
            return "result: " + event["fields"].text
        case "log":
            return event["line"].string ?? event["message"].string ?? event.text
        default:
            return event.text
        }
    }

    static func eventIsProblem(_ event: JSONValue) -> Bool {
        let kind = event["event"].string ?? ""
        return kind == "warning" || kind == "error"
    }

    /// `make_reels` → `Make reels`.
    static func kindTitle(_ kind: String) -> String {
        let spaced = kind.replacingOccurrences(of: "_", with: " ")
        guard let first = spaced.first else { return spaced }
        return String(first).uppercased() + spaced.dropFirst()
    }
}

/// Copy plain text (never a key) to the general pasteboard.
@MainActor
func serviceCopy(_ text: String) {
    let pasteboard = NSPasteboard.general
    pasteboard.clearContents()
    pasteboard.setString(text, forType: .string)
}

/// Save a stored video as a named file through a save panel. The store is only
/// read; the extension (`mp4` or `mov`) comes from the file's own bytes.
@MainActor
func exportVideo(_ file: URL, suggestedName: String, model: AppModel) {
    let name = suggestedName.replacingOccurrences(of: "/", with: "-")
    let panel = NSSavePanel()
    panel.nameFieldStringValue = "\(name).\(Playable.fileExtension(for: file))"
    guard panel.runModal() == .OK, let destination = panel.url else { return }
    do {
        try Data(contentsOf: file).write(to: destination)
        model.banner = "Saved \(destination.lastPathComponent)."
    } catch {
        model.banner = "Could not save the video: \(error.localizedDescription)"
    }
}

/// Show files in Finder.
@MainActor
func serviceReveal(_ urls: [URL]) {
    guard !urls.isEmpty else { return }
    // A stored video has no extension: reveal its typed link, so Finder shows <hash>.mp4.
    let shown = urls.map { url in url.isFileURL && Playable.isVideo(url) ? Playable.url(for: url) : url }
    NSWorkspace.shared.activateFileViewerSelecting(shown)
}

/// A small capsule label ("cached", "head", "after #3").
struct ServiceBadge: View {
    let text: String
    var color: Color = .blue

    var body: some View {
        Text(text)
            .font(.caption2)
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .background(color.opacity(0.15), in: Capsule())
    }
}

/// What a stored object holds, sniffed from its first bytes (store objects
/// have no file extension).
enum ServiceObjectKind: Equatable {
    /// An ISO base media file (`ftyp`): `qt` is true for a QuickTime brand.
    case video(qt: Bool)
    case text
    case folder
    case other

    static func of(_ file: StoredFile) -> ServiceObjectKind {
        if file.isFolder { return .folder }
        guard let handle = try? FileHandle(forReadingFrom: file.url) else { return .other }
        defer { try? handle.close() }
        let head: Data = (try? handle.read(upToCount: 512)) ?? Data()
        let bytes = [UInt8](head)
        if bytes.count >= 12, bytes[4] == 0x66, bytes[5] == 0x74, bytes[6] == 0x79, bytes[7] == 0x70 {
            let brand = String(decoding: bytes[8..<12], as: UTF8.self)
            return .video(qt: brand == "qt  ")
        }
        if let first = bytes.first(where: { $0 != 0x20 && $0 != 0x0A && $0 != 0x0D && $0 != 0x09 }) {
            if first == 0x7B || first == 0x5B { return .text }
        }
        if !bytes.isEmpty, !bytes.contains(0), String(data: head, encoding: .utf8) != nil { return .text }
        return .other
    }

    /// A link with a media extension to the read-only object, in the temporary
    /// folder, so AVFoundation knows its type. Nothing is written in the store.
    static func playableURL(for file: StoredFile, qt: Bool) -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ReelStudio-compare", isDirectory: true)
        let link = folder.appendingPathComponent(file.hash + (qt ? ".mov" : ".mp4"))
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            if FileManager.default.fileExists(atPath: link.path) { return link }
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: file.url)
            return link
        } catch {
            return file.url
        }
    }

    /// The first megabyte of a text object.
    static func readText(_ file: StoredFile) -> String {
        guard let handle = try? FileHandle(forReadingFrom: file.url) else { return "(cannot read \(file.path))" }
        defer { try? handle.close() }
        let limit = 1_048_576
        let data: Data = (try? handle.read(upToCount: limit + 1)) ?? Data()
        var text = String(decoding: data.prefix(limit), as: UTF8.self)
        if data.count > limit { text += "\n… (truncated at 1 MB)" }
        return text
    }
}
