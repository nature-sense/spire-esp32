import SwiftUI

/// The left top pane: **one** navigator for everything the project workspace can select.
///
/// There used to be two overlapping trees here — a "Project Structure" card (per build
/// system) and a "Subprojects" card — while the components themselves were listed twice:
/// once here and once on the right. Neither duplicate answered a question the other did not,
/// so this replaces both with a single list:
///
///   • the **project row** (selection = the project as a whole)
///   • for an ESP-IDF project, its `components/*` — the thing a library grows and an
///     application is *made of*
///   • for every other project, its build subprojects
///   • a compact **Build Targets** list for the selected non-IDF subproject, when it has any
///
/// A component row selects that component, and the right pane becomes its detail — the
/// fill-prose box, Write, History, Remove. The project row clears the selection, so the right
/// pane goes back to the project's own actions. Nothing is listed in two places: the navigator
/// is the only selection surface, and the right pane is the only place a component is changed.
struct ProjectNavigator: View {
    let project: ProjectInfo
    @Binding var selectedSubproject: SubprojectInfo?
    @Binding var selectedBuildTarget: String?
    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme

    /// A component is a subproject the analyzer labelled as one — not a guess from the path.
    private var components: [SubprojectInfo] {
        project.subprojects.filter { $0.kind == .component }
    }

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
            let count = components.count
            return "\(what) · \(count) component\(count == 1 ? "" : "s")"
        }
        if project.buildSystems.isEmpty {
            return "No build system detected"
        }
        return project.buildSystems.joined(separator: " · ")
    }

    // MARK: - Components (ESP-IDF)

    private var componentsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text("Components").font(.headline)
                Spacer(minLength: 0)
                Text("\(components.count)")
                    .font(.caption)
                    .foregroundStyle(theme.textTertiary)
            }

            if components.isEmpty {
                Text(emptyComponentsHint)
                    .font(.caption)
                    .foregroundStyle(theme.textTertiary)
            } else {
                VStack(spacing: 6) {
                    ForEach(components) { component in
                        componentRow(component)
                    }
                }
            }
        }
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
                    && selectedSubproject?.id == component.id
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

/// One component, as a line: its name, what it says it is, and whether it is the selected one.
///
/// A component reads the same wherever it is listed, so the row is written once. A library's
/// component and an application's unit differ only in what the *right pane* offers once the
/// row is selected — never in how the row itself reads. The framework badge is the same in
/// both: a shipped component is not the caller's to change.
struct ComponentRow: View {
    @Environment(AppTheme.self) private var theme

    let component: SubprojectInfo
    let selected: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: component.componentIcon)
                .font(.caption)
                .foregroundStyle(theme.accent)
            VStack(alignment: .leading, spacing: 1) {
                Text(component.name).font(.callout.weight(.semibold))
                Text(component.componentSummary)
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
                    .lineLimit(2)
            }
            Spacer(minLength: 0)
            if component.componentFramework != nil {
                Label("shipped", systemImage: "lock")
                    .font(.caption2)
                    .foregroundStyle(theme.textTertiary)
                    .help("Part of the library's framework: it comes with every library, and "
                          + "neither its code nor its place is yours to change here.")
            }
            Image(systemName: "chevron.right")
                .font(.caption2)
                .foregroundStyle(theme.textTertiary)
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
