import AppKit
import AVFoundation
import CoreImage
import ReelCore
import SwiftUI

/// The draft at a chosen time, original on the left and through the cube on
/// the right, with a draggable divider.
struct LookPreviewPane: View {
    let draftURL: URL?
    let cube: CubeLUT?
    /// Changes when the cube does (its object hash).
    let cubeTag: String?

    @State private var duration: Double = 0
    @State private var seconds: Double = 0
    @State private var original: NSImage?
    @State private var graded: NSImage?
    @State private var divider: CGFloat = 0.5
    @State private var failure: String?

    var body: some View {
        VStack(spacing: 10) {
            if draftURL == nil {
                ContentUnavailableView(
                    "Pick a draft",
                    systemImage: "camera.filters",
                    description: Text("The preview shows a frame of the draft, before and after the look.")
                )
            } else {
                picture
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                controls
            }
        }
        .padding(12)
        .task(id: draftURL) { await loadDuration() }
        .task(id: LookPreviewKey(url: draftURL, seconds: seconds, cube: cubeTag)) { await refresh() }
    }

    @ViewBuilder
    private var picture: some View {
        if let original {
            if let graded {
                LookSplitWipe(original: original, graded: graded, divider: $divider)
            } else {
                VStack {
                    Image(nsImage: original).resizable().aspectRatio(contentMode: .fit)
                    Text(ungradedNote)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        } else if let failure {
            ContentUnavailableView("No frame", systemImage: "exclamationmark.triangle", description: Text(failure))
        } else {
            ProgressView()
        }
    }

    private var ungradedNote: String {
        cube == nil ? "Run the look to preview the cube." : "Core Image could not apply the cube."
    }

    private var controls: some View {
        HStack(spacing: 10) {
            Text(formatMs(Int64(seconds * 1000)))
                .font(.callout.monospacedDigit())
                .frame(width: 80, alignment: .leading)
            Slider(value: $seconds, in: 0...max(duration, 0.01))
                .disabled(duration <= 0)
            Text(formatMs(Int64(duration * 1000)))
                .font(.callout.monospacedDigit())
                .foregroundStyle(.secondary)
            Button("Centre") { divider = 0.5 }
                .disabled(graded == nil)
        }
    }

    private func loadDuration() async {
        guard let url = draftURL else {
            duration = 0
            return
        }
        let length = await LookImaging.duration(url: url)
        guard draftURL == url else { return }
        duration = length
        if seconds > length { seconds = 0 }
    }

    private func refresh() async {
        guard let url = draftURL else { return }
        // A short pause, so dragging the slider grabs only the frame it stops on.
        try? await Task.sleep(nanoseconds: 60_000_000)
        if Task.isCancelled { return }
        let at = duration > 0 ? min(seconds, max(0, duration - 0.05)) : seconds
        let lut = cube
        do {
            let frames = try await LookImaging.frames(url: url, seconds: at, cube: lut)
            if Task.isCancelled { return }
            original = LookImaging.image(frames.original)
            graded = frames.graded.map { LookImaging.image($0) }
            failure = nil
        } catch {
            if Task.isCancelled { return }
            original = nil
            graded = nil
            failure = error.localizedDescription
        }
    }
}

/// What the preview depends on.
struct LookPreviewKey: Hashable {
    let url: URL?
    let seconds: Double
    let cube: String?
}

/// The split wipe: both pictures fitted to the same rectangle, the original
/// masked to the part left of the divider.
struct LookSplitWipe: View {
    let original: NSImage
    let graded: NSImage
    @Binding var divider: CGFloat

    var body: some View {
        GeometryReader { geometry in
            let fitted = LookSplitWipe.fit(original.size, in: geometry.size)
            let x = fitted.width * divider
            ZStack(alignment: .topLeading) {
                Image(nsImage: graded)
                    .resizable()
                    .frame(width: fitted.width, height: fitted.height)
                Image(nsImage: original)
                    .resizable()
                    .frame(width: fitted.width, height: fitted.height)
                    .mask(alignment: .leading) {
                        Rectangle().frame(width: max(0, x), height: fitted.height)
                    }
                Rectangle()
                    .fill(Color.white)
                    .frame(width: 2, height: fitted.height)
                    .shadow(color: .black.opacity(0.6), radius: 2)
                    .offset(x: x - 1)
                Circle()
                    .fill(Color.white)
                    .frame(width: 22, height: 22)
                    .overlay {
                        Image(systemName: "arrow.left.and.right")
                            .font(.caption2.bold())
                            .foregroundStyle(Color.black)
                    }
                    .shadow(color: .black.opacity(0.5), radius: 3)
                    .offset(x: x - 11, y: fitted.height / 2 - 11)
                HStack {
                    LookTag(text: "Original")
                    Spacer()
                    LookTag(text: "Graded")
                }
                .padding(8)
                .frame(width: fitted.width)
            }
            .frame(width: fitted.width, height: fitted.height)
            .clipped()
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        guard fitted.width > 0 else { return }
                        divider = min(1, max(0, value.location.x / fitted.width))
                    }
            )
            .frame(width: geometry.size.width, height: geometry.size.height)
        }
    }

    /// `size` scaled to fit inside `box`, keeping its aspect.
    static func fit(_ size: CGSize, in box: CGSize) -> CGSize {
        guard size.width > 0, size.height > 0, box.width > 0, box.height > 0 else { return .zero }
        let scale = min(box.width / size.width, box.height / size.height)
        return CGSize(width: size.width * scale, height: size.height * scale)
    }
}

struct LookTag: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.caption.bold())
            .foregroundStyle(Color.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.black.opacity(0.55), in: Capsule())
    }
}

/// Frame grabbing (AVAssetImageGenerator, exact time) and the cube through
/// Core Image. Runs off the main actor.
enum LookImaging {
    struct Frames: @unchecked Sendable {
        let original: CGImage
        let graded: CGImage?
    }

    enum Failure: LocalizedError {
        case noFrame

        var errorDescription: String? { "No frame at that time." }
    }

    /// One context for every render.
    static let context = CIContext(options: [CIContextOption.cacheIntermediates: false])

    static func duration(url: URL) async -> Double {
        let asset = AVURLAsset(url: url)
        guard let time = try? await asset.load(.duration) else { return 0 }
        let value = time.seconds
        return value.isFinite && value > 0 ? value : 0
    }

    static func frames(url: URL, seconds: Double, cube: CubeLUT?) async throws -> Frames {
        let asset = AVURLAsset(url: url)
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        generator.requestedTimeToleranceBefore = .zero
        generator.requestedTimeToleranceAfter = .zero
        generator.maximumSize = CGSize(width: 1920, height: 1920)
        let time = CMTime(seconds: max(0, seconds), preferredTimescale: 600)
        let grabbed = try await generator.image(at: time)
        let frame: CGImage = grabbed.image
        guard frame.width > 0, frame.height > 0 else { throw Failure.noFrame }
        let graded: CGImage? = cube.flatMap { apply($0, to: frame) }
        return Frames(original: frame, graded: graded)
    }

    /// The frame through `CIColorCubeWithColorSpace` (the cube's values are sRGB).
    static func apply(_ cube: CubeLUT, to frame: CGImage) -> CGImage? {
        guard let filter = CIFilter(name: "CIColorCubeWithColorSpace"),
              let sRGB = CGColorSpace(name: CGColorSpace.sRGB)
        else { return nil }
        let input = CIImage(cgImage: frame)
        filter.setValue(input, forKey: kCIInputImageKey)
        filter.setValue(NSNumber(value: cube.size), forKey: "inputCubeDimension")
        filter.setValue(cube.data, forKey: "inputCubeData")
        filter.setValue(sRGB, forKey: "inputColorSpace")
        guard let output = filter.outputImage else { return nil }
        return context.createCGImage(output, from: input.extent, format: .RGBA8, colorSpace: sRGB)
    }

    static func image(_ frame: CGImage) -> NSImage {
        NSImage(cgImage: frame, size: NSSize(width: frame.width, height: frame.height))
    }
}
