import AppKit
import ReelCore
import SwiftUI

/// One `timeline_view` PNG of the `views` output.
struct AnalysisThumb: Identifiable {
    let id: String
    let name: String
    let url: URL
    let image: NSImage?
    /// `<NN>` of `timeline_view_cut_<NN>_f<frame>.png`.
    let cutIndex: Int?
    /// `<frame>` of the same name.
    let frame: Int64?
}

/// A tree object's manifest (`crates/studio/src/store.rs`, `TreeManifest`).
private struct AnalysisTreeManifest: Decodable {
    struct Entry: Decodable {
        let name: String
        let hash: String
        let size: UInt64?
        let kind: String?
    }

    let kind: String?
    let entries: [Entry]
}

enum AnalysisFilmstrip {
    /// The PNGs of a stored `views` folder. Its stored path is the tree's JSON
    /// manifest; each entry's file comes from `objects.path`. A plain folder
    /// (should the service ever hand one out) is listed directly.
    @MainActor
    static func load(model: AppModel, views: StoredFile) async -> [AnalysisThumb] {
        var found: [(name: String, url: URL)] = []
        var isDirectory: ObjCBool = false
        if FileManager.default.fileExists(atPath: views.path, isDirectory: &isDirectory), isDirectory.boolValue {
            let names = (try? FileManager.default.contentsOfDirectory(atPath: views.path)) ?? []
            for name in names where name.lowercased().hasSuffix(".png") {
                found.append((name: name, url: views.url.appendingPathComponent(name)))
            }
        } else {
            guard let data = try? Data(contentsOf: views.url),
                  let manifest = try? JSONDecoder().decode(AnalysisTreeManifest.self, from: data)
            else { return [] }
            for entry in manifest.entries where entry.name.lowercased().hasSuffix(".png") && entry.kind != "tree" {
                let hash = entry.hash
                let path: URL? = await model.perform { try await $0.objectPath(hash) }
                if let path { found.append((name: entry.name, url: path)) }
            }
        }
        let thumbs = found.map { item -> AnalysisThumb in
            let parsed = parse(item.name)
            return AnalysisThumb(
                id: item.name, name: item.name, url: item.url,
                image: NSImage(contentsOf: item.url),
                cutIndex: parsed.cut, frame: parsed.frame)
        }
        return thumbs.sorted { a, b in
            switch (a.cutIndex, b.cutIndex) {
            case let (x?, y?) where x != y: return x < y
            default: return a.name < b.name
            }
        }
    }

    /// `timeline_view_cut_01_f270.png` → cut 1, frame 270.
    static func parse(_ name: String) -> (cut: Int?, frame: Int64?) {
        var stem = name
        if stem.lowercased().hasSuffix(".png") { stem = String(stem.dropLast(4)) }
        let parts = stem.split(separator: "_").map(String.init)
        var cut: Int?
        var frame: Int64?
        for (index, part) in parts.enumerated() {
            if part == "cut", index + 1 < parts.count { cut = Int(parts[index + 1]) }
            if part.hasPrefix("f"), let value = Int64(part.dropFirst()) { frame = value }
        }
        return (cut, frame)
    }

    /// The cut a thumbnail shows: by its frame, else its number, else its order.
    static func cut(for thumb: AnalysisThumb, at index: Int, in dna: EditDNA) -> EditDNA.Cut? {
        if let frame = thumb.frame, let cut = dna.cuts.first(where: { $0.frameIndex == frame }) { return cut }
        if let number = thumb.cutIndex, dna.cuts.indices.contains(number) { return dna.cuts[number] }
        if dna.cuts.indices.contains(index) { return dna.cuts[index] }
        return nil
    }

    /// Where a click on the thumbnail seeks.
    static func seekMs(for thumb: AnalysisThumb, at index: Int, in dna: EditDNA) -> Int64? {
        if let cut = cut(for: thumb, at: index, in: dna) {
            return AnalysisSupport.frameMs(cut.frameIndex, fps: dna.fps, fallback: cut.timeMs)
        }
        if let frame = thumb.frame {
            return AnalysisSupport.frameMs(frame, fps: dna.fps, fallback: 0)
        }
        return nil
    }
}

/// The row of `timeline_view` thumbnails; a click picks one.
struct AnalysisFilmstripRow: View {
    let thumbs: [AnalysisThumb]
    let selectedFrame: Int64?
    let pick: (Int, AnalysisThumb) -> Void

    var body: some View {
        ScrollView(.horizontal, showsIndicators: true) {
            LazyHStack(spacing: 8) {
                ForEach(Array(thumbs.enumerated()), id: \.element.id) { index, thumb in
                    Button {
                        pick(index, thumb)
                    } label: {
                        VStack(spacing: 2) {
                            if let image = thumb.image {
                                Image(nsImage: image)
                                    .resizable()
                                    .aspectRatio(contentMode: .fit)
                                    .frame(height: 78)
                            } else {
                                Rectangle().fill(Color.gray.opacity(0.2)).frame(width: 140, height: 78)
                            }
                            Text(caption(thumb, index))
                                .font(.caption2.monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                        .padding(3)
                        .overlay {
                            if thumb.frame != nil, thumb.frame == selectedFrame {
                                RoundedRectangle(cornerRadius: 4).stroke(Color.accentColor, lineWidth: 2)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                    .help(thumb.name)
                }
            }
            .padding(.horizontal, 4)
        }
        .frame(height: 104)
    }

    private func caption(_ thumb: AnalysisThumb, _ index: Int) -> String {
        let number = thumb.cutIndex.map { "cut \($0 + 1)" } ?? "cut \(index + 1)"
        guard let frame = thumb.frame else { return number }
        return "\(number) · f\(frame)"
    }
}
