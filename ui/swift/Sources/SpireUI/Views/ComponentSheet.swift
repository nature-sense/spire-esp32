import SwiftUI

/// The one sheet a **new** component needs: what it is, and what the user knows about it.
///
/// Add and write are here because a new component is one decision in two steps: the skeleton
/// follows from the kind — a driver gets a bus seam and a fake bus, so a host test can stand in
/// for the device that is not there; a library gets plain code and an ordinary unit test, because
/// there is nothing to stand in for — and the code is written from the box. **Stub only** stops
/// after the first step, for when the device's facts (or the algorithm) are not in hand yet.
///
/// Editing an existing component is deliberately **not** a sheet: it is the right pane's
/// `ComponentDetailView`. The box is the component's workspace, so it lives beside the navigator
/// and stays open while the person reads the tree.
///
/// The **chip** picker is here for both kinds, because the second gate is the same for both.
/// Host-only is the default because it is the honest default: it is the check that ran, and the
/// result says what did not.
struct ComponentSheet: View {
    /// What the user is adding, mirroring `ComponentKind` in the core.
    ///
    /// This is the question the component model turns on, and the reason it is asked rather than
    /// inferred: the two kinds share no skeleton.
    enum Kind: String, CaseIterable, Identifiable {
        case driver
        case library

        var id: String { rawValue }

        /// The kind in the user's terms rather than the tool's.
        var label: String {
            switch self {
            case .driver: return "Device driver"
            case .library: return "Library"
            }
        }

        /// The one sentence that tells the two apart.
        var hint: String {
            switch self {
            case .driver:
                return "One device on one bus: a protocol, written from its datasheet."
            case .library:
                return "Pure code — an algorithm, a filter, a codec — with no device and no bus."
            }
        }

        /// What the name field is asking for.
        var nameLabel: String { self == .driver ? "Device" : "Component" }

        /// What the empty name field suggests.
        var nameHint: String { self == .driver ? "sps30" : "moving_average" }
    }

    enum Result {
        /// Kept: what was asked, **verbatim**, and the gate's summary, kept apart so the pane
        /// showing the result can put the words back beside it. A stub add asks for nothing, so
        /// its instruction is empty.
        case ok(instruction: String, summary: String)
        case failed(String)
    }

    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme
    @Environment(\.dismiss) private var dismiss

    let root: String
    let done: (Result) -> Void

    @State private var kind: Kind = .driver
    @State private var name = ""
    @State private var bus = "i2c"
    /// A driver's part number — the key its facts are looked up by. Left empty, the component's own
    /// name is used, which is right when the two are the same word.
    @State private var part = ""
    /// The bus address, when the part has one (`0x69`).
    @State private var address = ""
    /// The capability paths the component provides, comma-separated — validated against the shared
    /// vocabulary before anything is written.
    @State private var roles = ""
    @State private var chip = ""
    @State private var chips: [Platform] = []
    @State private var described = ""
    @State private var running = false
    @State private var step = ""
    @State private var failure: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Add a component")
                    .font(.title3.weight(.semibold))
                Text("The skeleton follows from the kind; the code is written from what you say "
                     + "here, and then its host test has to pass.")
                    .font(.caption)
                    .foregroundStyle(theme.textSecondary)
            }

            VStack(alignment: .leading, spacing: 6) {
                SectionHeading("What it is")
                Picker("", selection: $kind) {
                    ForEach(Kind.allCases) { Text($0.label).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .disabled(running)
                Text(kind.hint)
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
            }
            VStack(alignment: .leading, spacing: 6) {
                SectionHeading(kind.nameLabel)
                TextField(kind.nameHint, text: $name)
                    .textFieldStyle(.roundedBorder)
                    .disabled(running)
            }
            // The bus, the part and the address are a **driver's** facts, so only a driver is asked
            // for them. A library is code: a bus field on it would be a bus it does not have.
            if kind == .driver {
                VStack(alignment: .leading, spacing: 6) {
                    SectionHeading("Bus")
                    Picker("", selection: $bus) {
                        Text("I²C").tag("i2c")
                        Text("SPI").tag("spi")
                        Text("UART").tag("uart")
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .disabled(running)
                }
                HStack(alignment: .top, spacing: 12) {
                    VStack(alignment: .leading, spacing: 6) {
                        SectionHeading("Part")
                        TextField(trimmedName.isEmpty ? "sps30" : trimmedName, text: $part)
                            .textFieldStyle(.roundedBorder)
                            .disabled(running)
                    }
                    VStack(alignment: .leading, spacing: 6) {
                        SectionHeading("Address")
                        TextField("0x69", text: $address)
                            .textFieldStyle(.roundedBorder)
                            .disabled(running)
                    }
                }
                Text("The part number is the key its facts are looked up by — often the same word as "
                     + "the component, and not always. Both are optional: a driver whose datasheet "
                     + "you do not have yet is still a driver.")
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
            }

            VStack(alignment: .leading, spacing: 6) {
                SectionHeading("Provides")
                TextField("sensing.particulate, sensing.temperature", text: $roles)
                    .textFieldStyle(.roundedBorder)
                    .disabled(running)
                Text("The capability paths this component provides, comma-separated, from the shared "
                     + "vocabulary. Left empty, it claims none — which is honest for a component "
                     + "whose role is not settled.")
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
            }

            VStack(alignment: .leading, spacing: 6) {
                SectionHeading("Verify against")
                Picker("", selection: $chip) {
                    Text("Host only").tag("")
                    ForEach(chips) { Text($0.name).tag($0.id) }
                }
                .labelsHidden()
                .disabled(running)
                .help("The host test always runs. Naming a chip adds `idf.py build` for it as a "
                      + "second gate — slower, and it catches what a host test cannot.")
                Text(chip.isEmpty
                     ? "The host test is the whole check: seconds, no board, no IDF — and it does "
                       + "not compile the component against the real driver."
                     : "Adds `idf.py build` for \(chip) as a second gate. It catches what a host "
                       + "test cannot — a wrong REQUIRES, a link error against the IDF driver.")
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
            }

            VStack(alignment: .leading, spacing: 6) {
                SectionHeading(boxHeading)
                TextEditor(text: $described)
                    .font(.system(.body, design: .monospaced))
                    .frame(height: 140)
                    .overlay(
                        RoundedRectangle(cornerRadius: 6).stroke(theme.border, lineWidth: 1)
                    )
                    .disabled(running)
                Text(descriptionHint)
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
            }

            if let failure {
                Text(failure)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .lineLimit(8)
            }
            if running {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(step).font(.caption).foregroundStyle(theme.textSecondary)
                }
            }

            HStack(spacing: 8) {
                Spacer(minLength: 0)
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                    .disabled(running)
                Button("Stub only") { addStubOnly() }
                    .disabled(!canRun)
                Button("Add & Write") { write() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!canRun)
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(theme.background)
        .task { await loadChips() }
    }

    private var trimmedName: String {
        name.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var canRun: Bool {
        !running && !trimmedName.isEmpty && !trimmedName.contains("/")
    }

    /// What the box is asking for, which is not the same question for every component.
    private var boxHeading: String {
        kind == .driver ? "What the device is" : "What this should do"
    }

    /// A driver's answer comes off a datasheet and a library's comes from the caller's head, so
    /// the same box would otherwise invite the wrong one: "what does register 0x1F return" has no
    /// answer for a filter.
    private var descriptionHint: String {
        switch (kind, described.isEmpty) {
        case (.driver, true):
            return "Command words, register addresses, reply lengths, checksums, byte order — the "
                 + "things that come off a datasheet. Left empty, nothing is invented: the stub "
                 + "keeps its TODOs."
        case (.driver, false):
            return "Written from this alone: the protocol is not looked up, and nothing you did "
                 + "not say is added."
        case (_, true):
            return "What it computes, and with what inputs and outputs, and the cases where it "
                 + "must refuse — the part no code can supply. Left empty, nothing is invented: "
                 + "the stub keeps its TODOs."
        case (_, false):
            return "Written from this alone: no algorithm, unit or constant you did not give is "
                 + "assumed."
        }
    }


    // MARK: - Acting

    /// The chips the registry knows — the same source the build leg reads its targets from.
    ///
    /// An empty list (a machine whose registry has never been loaded) leaves the picker on
    /// "Host only", which is still a truthful answer: the host test runs whatever is named here.
    private func loadChips() async {
        guard chips.isEmpty else { return }
        chips = await bridge.fetchPlatforms()
            .filter { $0.os == "esp-idf" && $0.kind == "chip" }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    /// The bus to pass: a driver's, and nothing for a library. A bus handed to a library is a
    /// contract that lies, so it is not handed one.
    private var busArgument: String { kind == .driver ? bus : "" }

    /// A driver's part and address, and nothing for a library — the same rule as the bus.
    private var partArgument: String {
        kind == .driver ? part.trimmingCharacters(in: .whitespacesAndNewlines) : ""
    }

    private var addressArgument: String {
        kind == .driver ? address.trimmingCharacters(in: .whitespacesAndNewlines) : ""
    }

    /// The capability paths, one per comma. Whitespace and empty entries are dropped here rather
    /// than sent on: an empty path is not a role, and the core would refuse it as a misspelling.
    private var rolesArgument: [String] {
        roles.split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }

    /// What the spec will say this component should become: the box, when the person wrote in it.
    private var description: String {
        described.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Add the stub and stop: the skeleton, with no model call. For when the device's facts (or
    /// the algorithm) are not in hand yet and the component should still exist.
    private func addStubOnly() {
        running = true
        failure = nil
        step = "Adding the stub…"
        Task {
            let (result, error) = await bridge.idfAddComponent(
                root: root, name: trimmedName, kind: kind.rawValue, bus: busArgument,
                part: partArgument, address: addressArgument, roles: rolesArgument,
                description: description
            )
            await MainActor.run {
                running = false
                if let error {
                    failure = error
                    return
                }
                let files = (result?["files"] as? [String]) ?? []
                let kindWord = kind == .driver ? "protocol" : "code"
                dismiss()
                done(.ok(instruction: "",
                         summary: "Added components/\(trimmedName) — \(files.count) files, its "
                                  + "\(kindWord) still a TODO. Write it when you have what it "
                                  + "needs."))
            }
        }
    }

    /// Add the stub, then write the code. The write is the one step that needs a model, and the
    /// one that has to pass the component's host test to be kept.
    private func write() {
        running = true
        failure = nil
        step = "Adding the stub…"
        Task {
            let (_, error) = await bridge.idfAddComponent(
                root: root, name: trimmedName, kind: kind.rawValue, bus: busArgument,
                part: partArgument, address: addressArgument, roles: rolesArgument,
                description: description
            )
            if let error {
                await MainActor.run {
                    running = false
                    failure = error
                }
                return
            }
            let what = kind == .driver ? "Writing the protocol" : "Writing the code"
            await MainActor.run {
                step = chip.isEmpty
                    ? "\(what), then its host test…"
                    : "\(what), then its host test and the \(chip) build…"
            }
            let (result, editError) = await bridge.idfComponentEdit(
                root: root,
                name: trimmedName,
                instruction: described,
                platform: chip.isEmpty ? nil : chip
            )
            await MainActor.run {
                running = false
                step = ""
                if let editError {
                    failure = editError
                    return
                }
                let kept = (result?["success"] as? Bool) ?? false
                let summary = (result?["output"] as? String) ?? ""
                let chipBuild = (result?["chip_build"] as? String) ?? ""
                if kept {
                    dismiss()
                    done(.ok(instruction: described,
                             summary: "Wrote \(trimmedName).\n\(summary)\n\(chipBuild)"))
                } else {
                    // The gate refused the change and restored it. That is a *result*, not a
                    // failure of the tool — so it is reported as what happened, with what was
                    // checked.
                    failure = "The change was not kept.\n\(summary)\n\(chipBuild)"
                }
            }
        }
    }
}
