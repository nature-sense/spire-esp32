import SwiftUI

/// The right pane when a **component** is selected: what it is, and the one box that changes it.
///
/// This was a sheet before — `ComponentSheet` for a library's component, `UnitEditSheet` for an
/// application's unit. A sheet is the wrong shape for the one thing a person does while reading
/// the tree: the box is the component's workspace, so it stays open beside the navigator, starts
/// from the design's own words when the design has them, and the result of a Write is read under
/// the box rather than in a dialog that has to be reopened to try again.
///
/// What hangs off the box follows from what the component is:
///
///   • **Write** always runs the component's host test as its gate; naming a chip adds that
///     chip's build as a second gate. The result says what was checked.
///   • **History** is the recorded runs for this component — what was asked and what the gate did.
///   • **Remove** is the directory, refused when another component is built on it.
///
/// A **framework** component has none of the three: it arrives complete, it is what the library
/// *is*, and both write and remove are refused if asked for anyway. Offering the buttons would be
/// inviting a refusal.
struct ComponentDetailView: View {
    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme

    let project: ProjectInfo
    let component: SubprojectInfo

    @State private var chip = ""
    @State private var chips: [Platform] = []
    @State private var described = ""
    /// Whether the box started from the design's own words rather than the person's typing.
    @State private var prefilled = false
    @State private var running = false
    @State private var step = ""
    @State private var failure: String?
    @State private var outcome: String?
    @State private var confirmingRemove = false
    @State private var showHistory = false
    /// What the graph says this component is — `nil` while it is being read, and set from the
    /// answer even when the component predates the graph (`stored == false`, implied from its tree).
    @State private var reading: ComponentSpecReading?

    /// The component's kind as the tree states it in its own `CMakeLists.txt` — `"driver"`,
    /// `"library"`, `"actor"` or `"stage"`.
    private var kind: String { component.componentKind ?? "" }

    /// A **unit** of the composition is an actor or a stage — the kinds the application's own
    /// fill wrote, and the only ones the design gives a sentence about to start the box from.
    private var isUnit: Bool { kind == "actor" || kind == "stage" }

    /// A shipped framework component is not the library's own work, so nothing here writes it.
    private var isFramework: Bool { component.componentFramework != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            details

            if isFramework {
                shippedNote
            } else {
                editor
                if let failure {
                    Text(failure)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .textSelection(.enabled)
                        .lineLimit(12)
                }
                if let outcome {
                    Text(outcome)
                        .font(.caption2)
                        .foregroundStyle(theme.textSecondary)
                        .textSelection(.enabled)
                        .lineLimit(20)
                }
                if running {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text(step).font(.caption).foregroundStyle(theme.textSecondary)
                    }
                }
                actions
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .alert("Remove \(component.name)?", isPresented: $confirmingRemove) {
            Button("Remove", role: .destructive) { remove() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Its directory goes. If another component is built on it, the removal is "
                 + "refused rather than repairing the other component.")
        }
        .sheet(isPresented: $showHistory) {
            ComponentEditHistorySheet(root: project.root, component: component.name)
        }
        // A different component is a different box: clear the words, the chip and the last
        // result rather than carrying one component's answer into another's editor.
        .task(id: component.id) {
            described = ""
            prefilled = false
            chip = ""
            failure = nil
            outcome = nil
            reading = nil
            await loadChips()
            await loadSpec()
            await prefill()
        }
    }

    // MARK: - What it is

    private var header: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: component.componentIcon)
                .font(.title3)
                .foregroundStyle(theme.accent)
            VStack(alignment: .leading, spacing: 2) {
                Text(component.name)
                    .font(.title3.weight(.semibold))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(component.componentSummary)
                    .font(.caption)
                    .foregroundStyle(theme.textSecondary)
                    .textSelection(.enabled)
            }
            Spacer(minLength: 0)
            if let framework = component.componentFramework {
                Label("shipped", systemImage: "lock")
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
                    .help("Part of the library's \(framework) framework: it comes with every "
                          + "library, and neither its code nor its place is yours to change.")
            }
        }
    }

    /// The facts a person checks before writing: where it lives and what it states it is.
    private var details: some View {
        VStack(alignment: .leading, spacing: 4) {
            detailRow("Path", component.path)
            detailRow("Kind", kindLabel)
            if let framework = component.componentFramework {
                detailRow("Framework", framework)
            }
            if let files = component.files, !files.isEmpty {
                detailRow("Files", "\(files.count)")
            }
            // What the **spec** states, beside what the tree states: the device its facts come from,
            // the capabilities it provides, and the documents it was written from. A component that
            // predates the graph has none of them, and its row says so rather than showing blanks.
            if let spec = reading?.spec, spec.hasContract {
                if let device = spec.device {
                    detailRow("Device", device.summary)
                }
                if !spec.provides.isEmpty {
                    detailRow("Provides", spec.provides.joined(separator: ", "))
                }
                if !spec.facts.isEmpty {
                    detailRow("From", spec.facts.map(\.source).joined(separator: ", "))
                }
            }
            if let reading {
                detailRow("Spec", specState(reading))
            }
        }
        .cardStyle(theme: theme)
    }

    /// The spec's own state in one line: implied from the tree, or stored with the decision beside it.
    private func specState(_ reading: ComponentSpecReading) -> String {
        guard reading.stored else {
            return "implied from the tree — the first edit makes it a spec"
        }
        var text = reading.freshness.label
        if let record = reading.record { text += " (\(record.summary))" }
        if let file = reading.file { text += " · \(file)" }
        return text
    }

    private func detailRow(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(label)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(theme.textTertiary)
                .frame(width: 64, alignment: .leading)
            Text(value)
                .font(.caption)
                .foregroundStyle(theme.textSecondary)
                .textSelection(.enabled)
            Spacer(minLength: 0)
        }
    }

    private var kindLabel: String {
        switch kind {
        case "driver": return "driver — one device on one bus"
        case "library": return "library — pure code, no bus"
        case "actor": return "actor — a message, its state, and a task of its own"
        case "stage": return "stage — a value pulled, a value pushed"
        default: return "states no kind"
        }
    }

    private var shippedNote: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Shipped with the library", systemImage: "lock")
                .font(.callout.weight(.semibold))
            Text("This component arrives complete: it is part of the library's framework, so "
                 + "there is nothing here to write and nothing here to remove. Its row stays in "
                 + "the navigator because the tree holds it, not because it is yours to change.")
                .font(.caption)
                .foregroundStyle(theme.textSecondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardStyle(theme: theme)
    }

    // MARK: - The box

    private var editor: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionHeading("Verify against")
            Picker("", selection: $chip) {
                Text("Host only").tag("")
                ForEach(chips) { Text($0.name).tag($0.id) }
            }
            .labelsHidden()
            .disabled(running)
            .help("The host test always runs. Naming a chip adds its build as a second gate — "
                  + "the one that checks the code against the real IDF driver.")
            Text(chip.isEmpty
                 ? "The component's host test is the gate that always runs: no board, no chip, "
                   + "no IDF. Naming a chip here builds for it as well."
                 : "Adds the \(chip) build as a second gate. It catches what a host test cannot "
                   + "— a wrong REQUIRES, a link error against the IDF driver.")
                .font(.caption2)
                .foregroundStyle(theme.textTertiary)

            Divider().overlay(theme.divider)

            SectionHeading(boxHeading)
            TextEditor(text: $described)
                .font(.system(.body, design: .monospaced))
                .frame(height: 170)
                .overlay(
                    RoundedRectangle(cornerRadius: 6).stroke(theme.border, lineWidth: 1)
                )
                .disabled(running)
            Text(descriptionHint)
                .font(.caption2)
                .foregroundStyle(theme.textTertiary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .cardStyle(theme: theme)
    }

    /// What the box is for, in this component's terms.
    private var boxHeading: String {
        switch kind {
        case "stage": return "What this stage should do"
        case "actor": return "What this actor should do"
        case "driver": return "What the device is"
        default: return "What this component should do"
        }
    }

    /// What the box is asking for, which is not the same question for every component — and
    /// what an empty one means. A driver's answer comes off a datasheet; a library's comes from
    /// the caller's head; a unit's is the part the design cannot supply on its own.
    private var descriptionHint: String {
        if described.isEmpty {
            switch kind {
            case "driver":
                return "Command words, register addresses, reply lengths, checksums, byte order "
                     + "— the things that come off a datasheet. Left empty, nothing is invented: "
                     + "the stub keeps its TODOs."
            case "actor", "stage":
                return "What this unit does with what it receives or pulls — the part the design "
                     + "cannot supply. Left empty, nothing is invented: the component keeps its "
                     + "TODOs and the design's shape is unchanged."
            default:
                return "What it computes, with what inputs and outputs, and the cases where it "
                     + "must refuse — the part no code can supply. Left empty, nothing is "
                     + "invented: the stub keeps its TODOs."
            }
        }
        if prefilled {
            return "Prefilled from the spec's own words for this component — what it was last "
                 + "asked to be. Edit them freely: what is written is built from this box."
        }
        if isUnit {
            return "Written from this alone, and from the design the application carries: no "
                 + "algorithm, no constant and no peer you were not given is assumed."
        }
        return "Written from this alone: no algorithm, unit or constant you did not give is assumed."
    }

    // MARK: - Acting

    private var actions: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                write()
            } label: {
                Label("Write", systemImage: "wand.and.stars")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .disabled(running)
            .help("Write this component's code from the box and the tree; kept only if its "
                  + "host test still compiles and passes")

            HStack(spacing: 8) {
                Button {
                    showHistory = true
                } label: {
                    Label("History", systemImage: "clock.arrow.circlepath")
                }
                .buttonStyle(.borderless)
                .disabled(running)
                .help("Recorded edits for this component")

                Spacer(minLength: 0)

                Button(role: .destructive) {
                    confirmingRemove = true
                } label: {
                    Label("Remove", systemImage: "trash")
                }
                .buttonStyle(.borderless)
                .disabled(running)
                .help("Remove this component")
            }
            .font(.caption)
        }
    }

    /// The chips the registry knows — the same source the component sheet and the build leg
    /// read. An empty list leaves the picker on "Host only", which is still a truthful answer:
    /// the host test runs whatever is named here.
    private func loadChips() async {
        guard chips.isEmpty else { return }
        chips = await bridge.fetchPlatforms()
            .filter { $0.os == "esp-idf" && $0.kind == "chip" }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    /// Read what the component **is**, before the box is offered: the spec's prose, its device, the
    /// capabilities it provides and the record behind it. A **read**, not a write — a component that
    /// predates the graph comes back implied from its tree, and its first edit is what decides it.
    private func loadSpec() async {
        let (found, error) = await bridge.fetchComponentSpec(
            root: project.root, name: component.name
        )
        if let error {
            failure = error
            return
        }
        reading = found
    }

    /// **Start the box from the spec's own words for this component.**
    ///
    /// The spec is the source of truth: what a component was last asked to become is where the next
    /// edit begins, and a person should not have to retype it. A unit whose spec has no prose yet
    /// (backfilled from its tree, never filled) falls back to the design the application carries — the
    /// one place a sentence about a unit still lives — rather than opening empty.
    ///
    /// It fills only an **empty** box, checked again after the read: a person who has begun typing
    /// before the answer arrives keeps what they wrote.
    private func prefill() async {
        guard described.isEmpty else { return }
        if let text = reading?.spec?.description,
           !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            guard described.isEmpty else { return }
            described = text
            prefilled = true
            return
        }
        guard isUnit else { return }
        let found = await bridge.fetchApplicationDesign(root: project.root)
        guard described.isEmpty,
              let matched = found.design?.composition.first(where: {
                  $0.matches(componentName: component.name)
              }),
              let text = matched.prefillText
        else { return }
        described = text
        prefilled = true
    }

    /// **Write or change the component's code.** The box is the instruction; the tree is the
    /// rest. The change is kept only if the host test still compiles and passes, and restored
    /// byte-for-byte otherwise — which is a *result*, not a failure of the tool.
    private func write() {
        running = true
        failure = nil
        outcome = nil
        step = chip.isEmpty
            ? "Writing \(kindWord), then its host test…"
            : "Writing \(kindWord), then its host test and the \(chip) build…"
        Task {
            let (result, error) = await bridge.idfComponentEdit(
                root: project.root,
                name: component.name,
                instruction: described,
                platform: chip.isEmpty ? nil : chip
            )
            await MainActor.run {
                running = false
                step = ""
                if let error {
                    failure = error
                    return
                }
                let kept = (result?["success"] as? Bool) ?? false
                let summary = (result?["output"] as? String) ?? ""
                let chipBuild = (result?["chip_build"] as? String) ?? ""
                if kept {
                    let asked = described.trimmingCharacters(in: .whitespacesAndNewlines)
                    let wrote = "Wrote \(component.name).\n\(summary)\n\(chipBuild)"
                    outcome = asked.isEmpty ? wrote : "You asked:\n\(asked)\n\n\(wrote)"
                    Task { await bridge.fetchProjectAnalysis() }
                    // The spec is now the run's: what was asked is its prose, and the documents it
                    // was written from are its facts. Re-read rather than patching it by hand, so the
                    // pane and the graph cannot disagree about what was decided.
                    Task { @MainActor in await loadSpec() }
                } else {
                    failure = "The change was not kept.\n\(summary)\n\(chipBuild)"
                }
            }
        }
    }

    /// **Remove the component.** Its directory goes; the selection goes with it, so the right
    /// pane returns to the project's own actions rather than showing a component that is no
    /// longer in the tree.
    private func remove() {
        running = true
        failure = nil
        outcome = nil
        step = "Removing components/\(component.name)…"
        Task {
            let (_, error) = await bridge.idfRemoveComponent(
                root: project.root, name: component.name
            )
            await MainActor.run {
                running = false
                step = ""
                if let error {
                    failure = error
                    return
                }
                bridge.selectedBuildTarget = nil
                bridge.selectSubproject(nil)
                Task { await bridge.fetchProjectAnalysis() }
            }
        }
    }

    /// The kind in the progress line's terms.
    private var kindWord: String {
        switch kind {
        case "stage": return "the stage"
        case "actor": return "the actor"
        case "driver": return "the protocol"
        default: return "the code"
        }
    }
}
