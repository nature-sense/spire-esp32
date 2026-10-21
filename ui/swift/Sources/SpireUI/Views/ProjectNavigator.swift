import SwiftUI

/// The left top pane: **one** navigator for everything the project workspace can select.
///
/// There used to be two overlapping trees here — a "Project Structure" card (per build
/// system) and a "Subprojects" card — while the components themselves were listed twice:
/// once here and once on the right. Neither duplicate answered a question the other did not,
/// so this replaces both with a single list:
///
///   • the **project row** (selection = the project as a whole)
///   • for an ESP-IDF project, the `components/*` that are **the library's own work** — the
///     thing a library grows and an application is *made of*
///   • then, apart from those, the components it merely **holds**: shipped with the framework,
///     or stating no kind of their own
///   • for every other project, its build subprojects
///   • a compact **Build Targets** list for the selected non-IDF subproject, when it has any
///
/// A row in the first group selects that component, and the right pane becomes its detail — the
/// fill-prose box, Write, History, Remove. The project row clears the selection, so the right
/// pane goes back to the project's own actions. Nothing is listed in two places: the navigator
/// is the only selection surface, and the right pane is the only place a component is changed.
///
/// The held group is what keeps that promise honest: those rows are **not selectable**, because
/// neither has a pane. A shipped component arrives complete — nothing writes or removes it — and
/// one that states no kind has no `SPIRE_COMPONENT_KIND` line for a spec to be implied from or an
/// edit to be shaped by. Listed together with the library's own they read as one list of things to
/// work on, half of which opens a pane with nothing in it; grouped apart, each list says what it is.
struct ProjectNavigator: View {
    let project: ProjectInfo
    @Binding var selectedSubproject: SubprojectInfo?
    @Binding var selectedBuildTarget: String?
    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme

    /// **What each row's two dots say**, keyed by component name — one read for the whole list
    /// (`project/componentStatus`), not one per row.
    ///
    /// Empty until the first answer arrives, and empty again if the core cannot answer: a row without
    /// dots is the list as it read before, which is better than a navigator that refuses to draw.
    @State private var statuses: [String: ComponentStatus] = [:]

    /// Re-read when the library changes, or when a component action has completed: a **refused** write
    /// moves the gate just as a kept one does, so the tick is bumped for both.
    private var statusKey: String { "\(project.root)|\(bridge.componentStatusTick)" }

    /// The list is the analyzer's (`project.components`), split by what the library owns: its own work
    /// is selectable, and what the tree merely holds — the framework's, and anything stating no kind —
    /// is listed apart and opens nothing (see `ProjectInfo.ownComponents`).

    /// Subprojects with a real build system, in the analyzer's order. Components are their
    /// own section, so they are not repeated here.
    private var subprojects: [SubprojectInfo] {
        project.subprojects.filter { !$0.buildSystem.isEmpty && $0.kind != .component }
    }

    private var isIDFApplication: Bool {
        project.subprojects.contains { $0.structure == "idf_application" }
    }

    private var isIDFLibrary: Bool {
        project.subprojects.contains { $0.structure == "idf_library" }
    }

    private var isIDFProject: Bool { isIDFApplication || isIDFLibrary }

    private var isProjectSelected: Bool { selectedSubproject == nil }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                projectRow

                if isIDFProject {
                    componentsSection
                } else {
                    subprojectsSection
                }

                // The compact target list belongs to a *buildable* selection. An ESP-IDF
                // component has no targets of its own — its host test is the gate, and that
                // runs inside Write — so this is the non-IDF shape only.
                if !isIDFProject,
                   let sub = selectedSubproject,
                   !sub.buildTargets.isEmpty {
                    PlatformBuildStatusList(
                        project: project,
                        subproject: sub,
                        selectedTarget: selectedBuildTarget,
                        onSelect: { selectTarget($0, in: sub) }
                    )
                    .cardStyle(theme: theme)
                }
            }
            .padding(12)
        }
        .background(theme.background)
        // The rows' dots, read once for the list. Keyed on the root and the component tick so a write
        // — kept or refused — is reflected without the pane polling.
        .task(id: statusKey) {
            guard isIDFProject else { return }
            let found = await bridge.fetchComponentStatuses(root: project.root)
            var byName: [String: ComponentStatus] = [:]
            for status in found { byName[status.name] = status }
            statuses = byName
        }
    }

    // MARK: - The project

    private var projectRow: some View {
        Button {
            selectedBuildTarget = nil
            bridge.selectedBuildTarget = nil
            selectedSubproject = nil
            bridge.selectSubproject(nil)
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "square.grid.2x2")
                    .font(.body.weight(.medium))
                    .foregroundStyle(theme.accent)
                VStack(alignment: .leading, spacing: 2) {
                    Text(project.name)
                        .font(.callout.weight(.semibold))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Text(projectSubtitle)
                        .font(.caption2)
                        .foregroundStyle(theme.textTertiary)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                if isProjectSelected {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.caption)
                        .foregroundStyle(theme.accent)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(isProjectSelected ? theme.accentBackground : theme.surface)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(isProjectSelected ? theme.accent : theme.border, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .help("The project as a whole — its actions, not any one component's")
    }

    /// What the project is, in one line: the IDF shape and how many components it holds, or
    /// the build systems that make it up.
    private var projectSubtitle: String {
        if isIDFApplication || isIDFLibrary {
            let what = isIDFApplication ? "ESP-IDF application" : "ESP-IDF component library"
            let count = project.components.count
            return "\(what) · \(count) component\(count == 1 ? "" : "s")"
        }
        if project.buildSystems.isEmpty {
            return "No build system detected"
        }
        return project.buildSystems.joined(separator: " · ")
    }

    // MARK: - Components (ESP-IDF)

    /// The library's own components — and, under their own heading, the ones it only holds.
    ///
    /// The count beside "Components" is the first group's: it is how many things this library adds,
    /// and it would stop meaning that if the framework's arrived in it.
    private var componentsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text("Components").font(.headline)
                Spacer(minLength: 0)
                Text("\(project.ownComponents.count)")
                    .font(.caption)
                    .foregroundStyle(theme.textTertiary)
            }

            if project.ownComponents.isEmpty {
                Text(emptyComponentsHint)
                    .font(.caption)
                    .foregroundStyle(theme.textTertiary)
            } else {
                VStack(spacing: 6) {
                    ForEach(project.ownComponents) { component in
                        componentRow(component)
                    }
                }
            }

            if !project.heldComponents.isEmpty {
                heldComponentsGroup
            }
        }
    }

    /// **The components the tree holds but this library does not own.**
    ///
    /// Two kinds of row land here for one reason: neither opens anything. A framework component is
    /// shipped complete — nothing writes it and nothing removes it — and a component that states no
    /// kind has no line for a spec to be implied from or an edit to be shaped by (`components/lvgl`,
    /// whose real sources are the `upstream/` tree beside it, reads exactly like this).
    ///
    /// Listed anyway, because they *are* in the tree and a reader who sees `components/lvgl` on disk
    /// should find it here rather than wonder where it went — but with no press behind the row, because
    /// a click that opened a pane with nothing in it is the thing this group exists to stop.
    private var heldComponentsGroup: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text("Shipped or unstated").font(.headline)
                Spacer(minLength: 0)
                Text("\(project.heldComponents.count)")
                    .font(.caption)
                    .foregroundStyle(theme.textTertiary)
            }
            Text("Part of the library's framework, or a component whose own CMakeLists.txt states no "
                 + "kind. Neither is yours to change — and neither opens: no spec can be read from "
                 + "them and no edit can be written into them.")
                .font(.caption2)
                .foregroundStyle(theme.textTertiary)
                .fixedSize(horizontal: false, vertical: true)
            VStack(spacing: 6) {
                ForEach(project.heldComponents) { component in
                    ComponentRow(component: component, selected: false, openable: false)
                }
            }
        }
        .padding(.top, 10)
    }

    private var emptyComponentsHint: String {
        if isIDFLibrary {
            return "None yet. The framework is here; what this library adds is on top of it — "
                 + "a driver, whose protocol is written from what you know about the device, or "
                 + "a library component, whose code is written from what you say it should do."
        }
        return "None yet. An application is what its components make it — an actor per job, a "
             + "stage per edge of the data path, and the message types they share."
    }

    private func componentRow(_ component: SubprojectInfo) -> some View {
        Button {
            select(component)
        } label: {
            ComponentRow(
                component: component,
                selected: selectedSubproject?.kind == .component
                    && selectedSubproject?.id == component.id,
                // Keyed by the component's own name, which is what its directory is called — the same
                // name the analyzer reports and the key the status read answers under.
                status: statuses[component.name]
            )
        }
        .buttonStyle(.plain)
    }


    // MARK: - Subprojects (everything that is not ESP-IDF)

    private var subprojectsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text("Subprojects").font(.headline)
                Spacer(minLength: 0)
                if !subprojects.isEmpty {
                    Text("\(subprojects.count)")
                        .font(.caption)
                        .foregroundStyle(theme.textTertiary)
                }
            }

            if subprojects.isEmpty {
                Text("Not detected yet")
                    .font(.caption)
                    .foregroundStyle(theme.textTertiary)
            } else {
                VStack(spacing: 6) {
                    ForEach(subprojects) { sub in
                        subprojectRow(sub)
                    }
                }
            }
        }
    }

    private func subprojectRow(_ sub: SubprojectInfo) -> some View {
        Button {
            select(sub)
        } label: {
            HStack(spacing: 8) {
                Image(systemName: subprojectIcon(sub.buildSystem))
                    .foregroundStyle(subprojectColor(sub.buildSystem))
                VStack(alignment: .leading, spacing: 2) {
                    Text(sub.name).font(.callout.weight(.medium))
                    if !sub.description.isEmpty {
                        Text(sub.description)
                            .font(.caption)
                            .foregroundStyle(theme.textSecondary)
                            .lineLimit(2)
                    }
                }
                Spacer(minLength: 0)
                Text(sub.buildSystem)
                    .font(.caption2)
                    .foregroundStyle(theme.textSecondary)
            }
            .padding(.vertical, 8)
            .padding(.horizontal, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .rowStyle(selected: selectedSubproject?.id == sub.id, theme: theme)
        }
        .buttonStyle(.plain)
    }

    // MARK: - Selection

    /// Selecting anything but the project row clears the target: the target belongs to the
    /// subproject that was selected before, and a stale one would scope the bottom panes to
    /// a target that is no longer in view.
    private func select(_ sub: SubprojectInfo) {
        selectedBuildTarget = nil
        bridge.selectedBuildTarget = nil
        selectedSubproject = sub
        bridge.selectSubproject(sub)
    }

    private func selectTarget(_ target: BuildTarget, in sub: SubprojectInfo) {
        selectedBuildTarget = target.name
        bridge.selectedBuildTarget = target.name
        selectedSubproject = sub
        bridge.selectSubproject(sub)
    }

    // MARK: - Presentation

    private func subprojectIcon(_ bs: String) -> String {
        switch bs {
        case "Cargo": return "gearshape.fill"
        case "SwiftPM", "Xcode": return "hammer.fill"
        case "npm", "pnpm", "yarn": return "square.and.pencil"
        default: return "folder.fill"
        }
    }

    private func subprojectColor(_ bs: String) -> Color {
        switch bs {
        case "Cargo": return Color(red: 0.70, green: 0.25, blue: 0.15)
        case "SwiftPM", "Xcode": return .orange
        case "npm", "pnpm", "yarn": return .green
        default: return .gray
        }
    }
}

/// One component, as a line: its name, what it says it is, and whether it is the selected one — with,
/// once the core has answered, two dots saying what state its contract is in.
///
/// A component reads the same wherever it is listed, so the row is written once. A library's
/// component and an application's unit differ only in what the *right pane* offers once the
/// row is selected — never in how the row itself reads. The framework badge is the same in
/// both: a shipped component is not the caller's to change.
///
/// **A row that opens nothing says so by doing nothing**: no chevron, no dots, no press. The held
/// group (shipped, or stating no kind) renders through this same shape, so the two lists cannot drift
/// into two vocabularies — the only difference is that one has a pane behind it.
struct ComponentRow: View {
    @Environment(AppTheme.self) private var theme

    let component: SubprojectInfo
    let selected: Bool
    /// What the core says about it — `nil` before the read answers, or when it could not. The row then
    /// reads exactly as it did before the dots existed.
    var status: ComponentStatus?
    /// Whether a click opens this component. `false` for the held group: those rows are listed to be
    /// seen, and a chevron on one would promise a pane that does not exist.
    var openable: Bool = true

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: component.componentIcon)
                .font(.caption)
                .foregroundStyle(openable ? theme.accent : theme.textTertiary)
            VStack(alignment: .leading, spacing: 1) {
                Text(component.name)
                    .font(.callout.weight(.semibold))
                    .foregroundStyle(openable ? theme.textPrimary : theme.textSecondary)
                Text(component.componentSummary)
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
                    .lineLimit(2)
            }
            Spacer(minLength: 0)
            if let status, openable {
                // Two facts, two glyphs: how much of the contract exists, and what the last write's
                // gate decided. Each one's sentence is in its tooltip — a row is a place to glance at,
                // and a glyph with a sentence behind it is what stays readable at this size.
                HStack(spacing: 3) {
                    Image(systemName: status.specSymbol)
                        .foregroundStyle(status.specColor(theme))
                        .help(status.specSummary)
                    Image(systemName: status.gateSymbol)
                        .foregroundStyle(status.gateColor(theme))
                        .help(status.gateSummary)
                }
                .font(.system(size: 8))
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(status.specSummary) \(status.gateSummary)")
            }
            if component.componentFramework != nil {
                Label("shipped", systemImage: "lock")
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
                    .help("Part of the library's framework: it comes with every library, and "
                          + "neither its code nor its place is yours to change here.")
            }
            if openable {
                Image(systemName: "chevron.right")
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 8)
                .fill(selected ? theme.accentBackground : theme.surface)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(selected ? theme.accent : theme.border, lineWidth: 1)
        )
    }
}

extension SubprojectInfo {
    /// The icon a component reads as: a shipped framework, or its kind.
    var componentIcon: String {
        if componentFramework != nil { return "square.stack.3d.up" }
        switch componentKind {
        case "library": return "function"
        case "actor": return "person.2"
        case "stage": return "arrow.left.arrow.right"
        default: return "shippingbox"
        }
    }

    /// What the component says it is — and, for the framework, that it is not the library's
    /// own work.
    ///
    /// From the component's own `CMakeLists.txt`, so it is shown rather than guessed at — and
    /// for one that states nothing (written by hand) the row says that, which is a fact too.
    var componentSummary: String {
        if let framework = componentFramework {
            return "\(path) · the \(framework) framework · shipped with the library"
        }
        let what: String
        switch componentKind {
        case "driver": what = "a driver: one device, one bus"
        case "library": what = "a library: pure code, no bus"
        case "actor": what = "an actor: a message, its state, and a task of its own"
        case "stage": what = "a stage: what it pulls, what it pushes, and the code between"
        default: what = "states no kind"
        }
        return "\(path) · \(what)"
    }
}

extension ComponentStatus {
    /// The contract dot's glyph: a filled circle once a decided spec is there, half of one when the
    /// record no longer ratifies it, hollow when nothing has been decided, and a question mark when the
    /// manifest states nothing to imply from.
    var specSymbol: String {
        switch fill {
        case .filled:
            switch freshness {
            case .current: return "circle.fill"
            // Filled and stale: the spec is there, the decision behind it is not — half-filled is the
            // one glyph that says both without the row having to explain itself.
            case .stale: return "circle.lefthalf.filled"
            case .absent: return "circle.dotted"
            }
        case .implied: return "circle"
        case .noKind: return "questionmark.circle"
        }
    }

    func specColor(_ theme: AppTheme) -> Color {
        guard case .filled = fill else { return theme.textTertiary }
        switch freshness {
        case .current: return .green
        case .stale: return .orange
        case .absent: return theme.textTertiary
        }
    }

    /// The gate dot: nothing yet, kept, or refused — the last two in the same colours the edit history
    /// uses, so the row and the history sheet cannot describe one run two ways.
    var gateSymbol: String {
        guard let gate else { return "minus.circle" }
        return gate.success ? "checkmark.circle.fill" : "xmark.circle.fill"
    }

    func gateColor(_ theme: AppTheme) -> Color {
        guard let gate else { return theme.textTertiary }
        return gate.success ? .green : .red
    }
}

