import SwiftUI

/// The one sheet a **unit** of an ESP-IDF application needs: what it should do, in the user's words.
///
/// A unit — an actor, a ramen stage — is not a library component: it is not added by hand and not
/// removed, because the composition says it exists. What *can* change is its code, and that write is
/// built from the **reviewed design** the application carries (`SPIRE.application.json`) rather than from
/// a datasheet: the box asks what the unit does with the message it receives, or with the value it pulls.
/// That is the part no design can supply *on its own*, which is why it is the only thing asked for here —
/// and why the box starts from the design's own words when it states them.
///
/// The **chip** picker is here for the same reason it is on a component: the host test always runs, and
/// naming a chip adds its build as a *second* gate. Host-only is the honest default — it is the check
/// that ran, and the result says what did not.
struct UnitEditSheet: View {
    enum Result {
        /// Kept: what was asked, **verbatim**, and the gate's summary, kept apart so the pane showing the
        /// result can put the words back beside it — the box is gone the moment this sheet closes.
        case ok(instruction: String, summary: String)
        case failed(String)
    }

    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme
    @Environment(\.dismiss) private var dismiss

    let unit: SubprojectInfo
    /// The application's root — the tree that holds `components/<unit>/` **and** the design the write is
    /// built from.
    let root: String
    let done: (Result) -> Void

    @State private var chip = ""
    @State private var chips: [Platform] = []
    @State private var described: String = ""
    /// Whether the box started from the design's own words rather than the person's typing.
    @State private var prefilled = false
    @State private var running = false
    @State private var step = ""
    @State private var failure: String?

    /// The unit's kind as the application states it in the component's own `CMakeLists.txt` —
    /// `"actor"` or `"stage"`.
    private var kind: String { unit.componentKind ?? "" }

    /// The kind in the report's terms, for the progress line.
    private var kindWord: String { kind == "stage" ? "stage" : "actor" }

    private var subtitle: String {
        switch kind {
        case "stage":
            return "A ramen stage: its code is written from what you say here and from the design the "
                 + "application carries — the value it pulls and the value it pushes. Then its host "
                 + "test has to pass."
        default:
            return "An actor: its code is written from what you say here and from the design the "
                 + "application carries — its message, the state it holds and the peers it sends to. "
                 + "Then its host test has to pass."
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(unit.name).font(.title3.weight(.semibold))
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(theme.textSecondary)
            }

            VStack(alignment: .leading, spacing: 6) {
                SectionHeading("Verify against")
                Picker("", selection: $chip) {
                    Text("Host only").tag("")
                    ForEach(chips) { Text($0.name).tag($0.id) }
                }
                .labelsHidden()
                .disabled(running)
                .help("The host test always runs. Naming a chip adds its build as a second gate — the "
                      + "one that checks the code against the real IDF driver.")
                Text("The unit's host test is the gate that always runs: no board, no chip, no IDF. "
                     + "Naming a chip here builds for it as well.")
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
                Button("Write") { write() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(running)
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(theme.background)
        .task { await loadChips() }
        // The design is read in its own task: it is not the picker's source, and a slow registry
        // must not hold back the words the box starts from.
        .task { await prefill() }
    }

    /// What the box is for, in this unit's terms.
    private var boxHeading: String {
        kind == "stage" ? "What this stage should do" : "What this actor should do"
    }

    /// The design states what a unit **is** — its message, its state, its peers, the values on its
    /// edges — and, when the person wrote one, a sentence about what it does. So an empty box means
    /// nothing is invented: the design's shape is kept and the component's `TODO`s stay. A box that
    /// started from the design's sentence says so, because that sentence is not the person's.
    private var descriptionHint: String {
        if described.isEmpty {
            return "What this unit does with what it receives or pulls — the part the design cannot "
                 + "supply. Left empty, nothing is invented: the component keeps its TODOs and the "
                 + "design's shape is unchanged."
        }
        if prefilled {
            return "Prefilled from the design's own words for this unit — its description and the "
                 + "state it holds. Edit them freely: what is written is built from this box."
        }
        return "Written from this alone, and from the design the application carries: no algorithm, no "
             + "constant and no peer you were not given is assumed."
    }

    // MARK: - Acting

    /// The chips the registry knows — the same source the component sheet and the build leg read.
    ///
    /// An empty list (a machine whose registry has never been loaded) leaves the picker on "Host only",
    /// which is still a truthful answer: the host test runs whatever is named here.
    private func loadChips() async {
        guard chips.isEmpty else { return }
        chips = await bridge.fetchPlatforms()
            .filter { $0.os == "esp-idf" && $0.kind == "chip" }
            .sorted { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    }

    /// **Start the box from the design's own words for this unit.**
    ///
    /// The composition already states a sentence about an actor (its `description`) and the state it
    /// holds; both are what the design knows about what the unit *is*, and a person about to say what
    /// it should do should not have to retype them. The two are joined with an em dash, and a side the
    /// design does not state is left out rather than written as a dangling dash.
    ///
    /// It fills only an **empty** box, checked again after the read: a person who has begun typing
    /// before the design arrives keeps what they wrote. A project with no composition — or a unit the
    /// composition does not describe — leaves the box empty, which still means "nothing is invented".
    private func prefill() async {
        guard described.isEmpty else { return }
        let found = await bridge.fetchApplicationDesign(root: root)
        guard described.isEmpty,
              let matched = found.design?.composition.first(where: {
                  $0.matches(componentName: unit.name)
              }),
              let text = matched.prefillText
        else { return }
        described = text
        prefilled = true
    }

    private func write() {
        running = true
        failure = nil
        step = chip.isEmpty
            ? "Writing the \(kindWord), then its host test…"
            : "Writing the \(kindWord), then its host test and the \(chip) build…"
        Task {
            let (result, error) = await bridge.idfComponentEdit(
                root: root,
                name: unit.name,
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
                    dismiss()
                    done(.ok(instruction: described,
                             summary: "Wrote \(unit.name).\n\(summary)\n\(chipBuild)"))
                } else {
                    // The gate refused the change and restored it. That is a *result*, not a failure of
                    // the tool — so it is reported as what happened, with what was checked.
                    failure = "The change was not kept.\n\(summary)\n\(chipBuild)"
                }
            }
        }
    }
}
