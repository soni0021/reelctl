import AppKit
import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// The `compositor.isolate-presenter` arguments the screen sets (the
/// catalogue's `mode`, `background`, `weights`, `downsample_ratio`).
struct PresenterOptions {
    enum Mode: String, CaseIterable, Identifiable {
        case composite, matte, rgba

        var id: String { rawValue }

        var title: String {
            switch self {
            case .composite: return "Composite"
            case .matte: return "Matte"
            case .rgba: return "RGBA"
            }
        }

        var explanation: String {
            switch self {
            case .composite: return "The presenter over a background colour, image or video (mp4)."
            case .matte: return "The alpha alone as a grey video, no audio (mp4)."
            case .rgba: return "The presenter with straight alpha, ProRes 4444 in a .mov."
            }
        }
    }

    enum Background: String, CaseIterable, Identifiable {
        case colour, file

        var id: String { rawValue }

        var title: String {
            switch self {
            case .colour: return "Colour"
            case .file: return "Image or video"
            }
        }
    }

    var mode: Mode = .composite
    var background: Background = .colour
    /// `#0b1d2a`, the CLI docs' example.
    var colour = Color(.sRGB, red: 11.0 / 255.0, green: 29.0 / 255.0, blue: 42.0 / 255.0, opacity: 1)
    var backgroundFile: URL?
    var customRatio = false
    var downsampleRatio = 0.4

    /// Why the options cannot be submitted, or nil.
    var problem: String? {
        switch mode {
        case .rgba:
            return "RGBA needs a .mov output, and the service names this job's output presenter.mp4, so reelctl would refuse it. Use Composite or Matte (see Screens/Presenter/NOTES.md)."
        case .matte:
            return nil
        case .composite:
            switch background {
            case .colour:
                return Self.hex(colour) == nil ? "The colour has no sRGB value." : nil
            case .file:
                guard let backgroundFile else { return "Choose a background image or video." }
                return backgroundFile.path.hasPrefix("/") ? nil : "The background must be an absolute path."
            }
        }
    }

    /// The typed arguments, with the weights path.
    func args(weights: String) -> [String: JSONValue] {
        var args: [String: JSONValue] = [
            "mode": .string(mode.rawValue),
            "weights": .string(weights),
        ]
        // `--background` is required for composite and refused for the other modes.
        if mode == .composite {
            switch background {
            case .colour:
                if let hex = Self.hex(colour) { args["background"] = .string(hex) }
            case .file:
                if let backgroundFile { args["background"] = .string(backgroundFile.path) }
            }
        }
        if customRatio {
            args["downsample_ratio"] = .double(min(1, max(0.01, downsampleRatio)))
        }
        return args
    }

    /// `#rrggbb` in sRGB.
    static func hex(_ colour: Color) -> String? {
        guard let rgb = NSColor(colour).usingColorSpace(.sRGB) else { return nil }
        let components: [CGFloat] = [rgb.redComponent, rgb.greenComponent, rgb.blueComponent]
        var text = "#"
        for component in components {
            let clamped = min(CGFloat(1), max(CGFloat(0), component))
            let byte = Int((clamped * 255).rounded())
            let digits = String(byte, radix: 16)
            text += digits.count < 2 ? "0" + digits : digits
        }
        return text
    }
}

/// The options as form rows.
struct PresenterOptionsForm: View {
    @Binding var options: PresenterOptions

    var body: some View {
        Picker("Output", selection: $options.mode) {
            ForEach(PresenterOptions.Mode.allCases) { mode in
                Text(mode.title).tag(mode)
            }
        }
        .pickerStyle(.segmented)
        Text(options.mode.explanation)
            .font(.caption)
            .foregroundStyle(.secondary)
        if options.mode == .composite {
            Picker("Background", selection: $options.background) {
                ForEach(PresenterOptions.Background.allCases) { background in
                    Text(background.title).tag(background)
                }
            }
            switch options.background {
            case .colour:
                ColorPicker("Colour", selection: $options.colour, supportsOpacity: false)
                if let hex = PresenterOptions.hex(options.colour) {
                    FieldRow(label: "Sent as", value: hex)
                }
            case .file:
                HStack {
                    Text(options.backgroundFile?.lastPathComponent ?? "No file")
                        .foregroundStyle(options.backgroundFile == nil ? Color.secondary : Color.primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Button("Choose…") {
                        let types: [UTType] = [.image] + UTType.videoTypes
                        if let url = chooseFile(types, title: "Background") {
                            options.backgroundFile = url
                        }
                    }
                }
                Text("Scaled to cover the frame and cropped at the centre; a video loops.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        Toggle("Set the downsample ratio", isOn: $options.customRatio)
        if options.customRatio {
            HStack {
                Slider(value: $options.downsampleRatio, in: 0.05...1)
                Text(String(format: "%.2f", options.downsampleRatio))
                    .font(.callout.monospacedDigit())
                    .frame(width: 44, alignment: .trailing)
            }
            Text("Lower is faster and sees less detail.")
                .font(.caption)
                .foregroundStyle(.secondary)
        } else {
            Text("Default: 512 divided by the longest side, at most 1 (0.4 for 720p).")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        if let problem = options.problem {
            Label(problem, systemImage: "exclamationmark.triangle")
                .font(.caption)
                .foregroundStyle(.orange)
        }
    }
}
