import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// The Make reels settings: the long video, 1 to 3 outlier DNAs, the bridge,
/// which clips, the reframe, the model and the caption style.
struct MakeReelsForm: View {
    @Environment(AppModel.self) private var model
    @Bindable var run: MakeReelsRun

    var body: some View {
        Form {
            SwiftUI.Section("Long video") {
                AssetPicker(title: "Video", kind: "video", types: UTType.videoTypes, selection: $run.draft.video)
                    .dropImport(kind: "video") { asset in run.draft.video = asset.id }
                AssetPicker(title: "Its DNA (optional)", kind: "edit_dna", types: [.json], selection: $run.draft.existingDna)
                Text(dnaNote).font(.caption).foregroundStyle(.secondary)
            }

            SwiftUI.Section("Outlier DNAs (1 to 3)") {
                ForEach(Array(run.draft.outliers.enumerated()), id: \.offset) { index, outlier in
                    HStack {
                        Image(systemName: icon(outlier)).foregroundStyle(.secondary)
                        Text(name(outlier)).lineLimit(1)
                        Spacer()
                        Button {
                            if run.draft.outliers.indices.contains(index) { run.draft.outliers.remove(at: index) }
                        } label: {
                            Image(systemName: "minus.circle")
                        }
                        .buttonStyle(.borderless)
                    }
                }
                HStack {
                    Menu("Add outlier") {
                        let dnas = model.assets.filter { $0.kind == "edit_dna" }
                        if !dnas.isEmpty {
                            SwiftUI.Section("Imported DNAs") {
                                ForEach(dnas) { asset in
                                    Button(asset.name) { add(.asset(asset.id)) }
                                }
                            }
                        }
                        let reels = model.subjects(kind: "reel").filter { $0.headCheckpoint != nil }
                        if !reels.isEmpty {
                            SwiftUI.Section("Analysed reels") {
                                ForEach(reels) { subject in
                                    Button(subject.name) { add(.subject(subject.id)) }
                                }
                            }
                        }
                        if dnas.isEmpty && reels.isEmpty {
                            Text("No DNAs yet: import one or decompile a reel in Library.")
                        }
                    }
                    .disabled(run.draft.outliers.count >= 3)
                    Button("Import…") {
                        guard let url = chooseFile([.json], title: "Outlier edit_dna.json") else { return }
                        Task {
                            if let asset = await model.importFile(url, kind: "edit_dna") { add(.asset(asset.id)) }
                        }
                    }
                    .disabled(run.draft.outliers.count >= 3)
                }
                .dropImport(kind: "edit_dna") { asset in add(.asset(asset.id)) }
                .help("Drop edit_dna.json files here")
            }

            SwiftUI.Section("Bridge card") {
                TextField("Target URL", text: $run.draft.targetURL, prompt: Text("https://…"))
                TextField("Card length (ms)", text: $run.draft.cardMsText, prompt: Text("default 2500"))
            }

            SwiftUI.Section("Clips") {
                Toggle("Limit the number of reels", isOn: $run.draft.limitReels)
                if run.draft.limitReels {
                    Stepper(value: $run.draft.reels, in: 0...20) {
                        LabeledContent("Reels") { Text("\(run.draft.reels)").monospacedDigit() }
                    }
                }
                TextField("Clips by rank", text: $run.draft.clipsText, prompt: Text("all that pass, or e.g. 1,3"))
                Toggle("Allow flagged clips", isOn: $run.draft.allowFlagged)
            }

            SwiftUI.Section("Frame") {
                Picker("Follow", selection: $run.draft.follow) {
                    Text("Presenter").tag("presenter")
                    Text("Centre").tag("centre")
                }
                .pickerStyle(.segmented)
                if run.draft.follow == "presenter" && ToolSetting.rvmWeights.value.isEmpty {
                    Text("No RobustVideoMatting weights in Settings: the service may fall back to a centre crop, and the reel's notes will say so.")
                        .font(.caption)
                        .foregroundStyle(.orange)
                }
                TextField("Size", text: $run.draft.size, prompt: Text("1080x1920"))
            }

            SwiftUI.Section("Model (paid call)") {
                TextField("Model", text: $run.draft.model, prompt: Text(MakeReelsDraft.defaultModel))
                TextField("Cost cap (USD)", text: $run.draft.maxCostText, prompt: Text("optional, e.g. 0.50"))
                Text("You approve the exact upper bound before anything is sent.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            MakeReelsCaptionSection(style: $run.draft.style)
        }
        .formStyle(.grouped)
        .disabled(run.isRunning)
    }

    private var dnaNote: String {
        if run.draft.existingDna != nil { return "Decompile is skipped; the DNA must have whisper words." }
        if ToolSetting.whisperModel.value.isEmpty {
            return "No whisper model in Settings: decompile cannot measure words, and make-reels needs them. Set one, or pick a DNA with whisper words."
        }
        return "Decompile runs first, with the whisper model from Settings."
    }

    private func add(_ outlier: MakeReelsOutlier) {
        guard run.draft.outliers.count < 3, !run.draft.outliers.contains(outlier) else { return }
        run.draft.outliers.append(outlier)
    }

    private func name(_ outlier: MakeReelsOutlier) -> String {
        switch outlier {
        case .asset(let id): return model.assets.first(where: { $0.id == id })?.name ?? "DNA \(id)"
        case .subject(let id): return model.subject(id)?.name ?? "Reel \(id)"
        }
    }

    private func icon(_ outlier: MakeReelsOutlier) -> String {
        switch outlier {
        case .asset: return "doc.text"
        case .subject: return "film"
        }
    }
}

/// The caption style: a preset, then the overrides (empty keeps the preset's).
struct MakeReelsCaptionSection: View {
    @Binding var style: MakeReelsCaptionStyle
    @State private var expanded = false

    var body: some View {
        SwiftUI.Section("Captions") {
            Picker("Preset", selection: $style.preset) {
                Text("Default").tag("")
                ForEach(MakeReelsCaptionStyle.presets, id: \.self) { preset in
                    Text(preset.capitalized).tag(preset)
                }
            }
            DisclosureGroup("Style overrides", isExpanded: $expanded) {
                HStack {
                    TextField("Font", text: $style.font, prompt: Text("/path/to/font.ttf"))
                    Button("Choose…") {
                        if let url = chooseFile([.font], title: "Caption font") { style.font = url.path }
                    }
                }
                field("Font size (% of height)", $style.fontSize, "2–12")
                field("Colour", $style.color, "#ffffff")
                field("Highlight colour", $style.highlightColor, "#rrggbb or none")
                field("Outline colour", $style.outlineColor, "#000000")
                field("Outline width (px)", $style.outlineWidth, "0–12")
                field("Shadow", $style.shadow, "0–12")
                field("Box colour", $style.boxColor, "#000000")
                field("Box opacity", $style.boxOpacity, "0–1")
                Picker("Position", selection: $style.position) {
                    Text("Preset's").tag("")
                    ForEach(MakeReelsCaptionStyle.positions, id: \.self) { position in
                        Text(position.capitalized).tag(position)
                    }
                }
                field("Margin (%)", $style.margin, "0–40")
                field("Words per group", $style.wordsPerGroup, "1–5")
                field("Max characters", $style.maxChars, "8–40")
                Picker("Case", selection: $style.textCase) {
                    Text("Preset's").tag("")
                    ForEach(MakeReelsCaptionStyle.cases, id: \.self) { value in
                        Text(value).tag(value)
                    }
                }
                field("Minimum gap (ms)", $style.minGapMs, "100–2000")
            }
            let problems = style.problems
            if !problems.isEmpty {
                ForEach(problems, id: \.self) { problem in
                    Text(problem).font(.caption).foregroundStyle(.red)
                }
            }
        }
    }

    private func field(_ title: String, _ text: Binding<String>, _ prompt: String) -> some View {
        TextField(title, text: text, prompt: Text(prompt))
    }
}
