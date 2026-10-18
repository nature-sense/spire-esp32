import SwiftUI

/// Data-dense project analysis pane — replaces the sparse node graph in the
/// top-left region. Uses only data the Rust analyzer already provides on
/// `ProjectInfo` / `SubprojectInfo`:
///   • header: project name (single row, no filler subtitle)
///   • project structure: per build system → languages + subproject count
///     + package description — always visible, no scrolling to see it
///   • subprojects (selectable → drives the center "Subproject" context)
///   • build targets (selectable → drives Sources/Dependencies/Builds)
/// The same selection bindings the graph previously exposed are preserved so
/// the center and right panes keep working unchanged.
struct ProjectAnalysisView: View {
    let project: ProjectInfo
    @Binding var selectedSubproject: SubprojectInfo?
    @Binding var selectedBuildTarget: String?
    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme

    init(project: ProjectInfo,
         selectedSubproject: Binding<SubprojectInfo?>,
         selectedBuildTarget: Binding<String?>) {
        self.project = project
        self._selectedSubproject = selectedSubproject
        self._selectedBuildTarget = selectedBuildTarget
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            headerSection
                .fixedSize(horizontal: false, vertical: true)

            structureSection
                .fixedSize(horizontal: false, vertical: true)

            // The lower lists are the only parts that may scroll when long.
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    subprojectsSection
                    buildTargetsSection
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(theme.background)
    }

    // MARK: - Project Structure (grouped by build system)

    /// Per build system → the subprojects under it, languages, and first
    /// non-empty description. Drives the structure summary.
    private var structureGroups: [(system: String, subs: [SubprojectInfo])] {
        let subs = project.subprojects.filter { !$0.buildSystem.isEmpty }
        // Preserve project.buildSystems ordering; fall back to insertion order.
        let order = project.buildSystems.isEmpty
            ? subs.map(\.buildSystem).reduce(into: []) { $0.append($1) }
            : project.buildSystems
        var seen = Set<String>()
        var groups: [(String, [SubprojectInfo])] = []
        for system in order where !seen.contains(system) {
            let members = subs.filter { $0.buildSystem == system }
            if !members.isEmpty {
                groups.append((system, members))
                seen.insert(system)
            }
        }
        return groups
    }

    private var structureSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Project Structure")
                .font(.headline)

            if structureGroups.isEmpty {
                Text("No build structure detected yet")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(structureGroups.indices, id: \.self) { idx in
                    let group = structureGroups[idx]
                    let langs = uniqueLanguages(group.subs)
                    let description = group.subs
                        .map(\.description)
                        .first { !$0.isEmpty }

                    HStack(alignment: .top, spacing: 8) {
                        Image(systemName: subprojectIcon(group.system))
                            .font(.title3)
                            .foregroundStyle(subprojectColor(group.system))
                            .frame(width: 22)

                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text(group.system)
                                    .font(.callout.weight(.semibold))
                                Text("· \(group.subs.count) subproject\(group.subs.count == 1 ? "" : "s")")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            if !langs.isEmpty {
                                Text(langs)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            if let description {
                                Text(description)
                                    .font(.caption)
                                    .foregroundStyle(.tertiary)
                                    .lineLimit(2)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.vertical, 3)
                }
            }
        }
        .cardStyle(theme: theme)
    }

    /// Distinct language labels (e.g. "🦀 Rust") across a build system's
    /// subprojects, joined with a separator.
    private func uniqueLanguages(_ subs: [SubprojectInfo]) -> String {
        let langs = subs.map(\.language).filter { !$0.isEmpty }
        var seen = Set<String>()
        let distinct = langs.filter { seen.insert($0).inserted }
        return distinct.joined(separator: " · ")
    }

    // MARK: - Header (compact single row)

    private var headerSection: some View {
        HStack(spacing: 8) {
            Image(systemName: "square.grid.2x2")
                .foregroundStyle(theme.accent)
            Text(project.name)
                .font(.title3.weight(.semibold))
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .cardStyle(theme: theme)
    }

    // MARK: - Subprojects

    private var subprojectsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Subprojects")
                .font(.headline)

            if project.subprojects.isEmpty {
                Text("None")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } else {
                let list = project.subprojects.filter { $0.buildSystem.isEmpty == false }
                if list.isEmpty {
                    Text("Not detected yet")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(list) { sub in
                        Button {
                            selectedSubproject = sub
                            selectedBuildTarget = nil
                            bridge.selectedBuildTarget = nil
                            bridge.selectSubproject(sub)
                        } label: {
                            HStack(spacing: 8) {
                                Image(systemName: subprojectIcon(sub.buildSystem))
                                    .foregroundStyle(subprojectColor(sub.buildSystem))
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(sub.name).font(.callout.weight(.medium))
                                    if !sub.description.isEmpty {
                                        Text(sub.description)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                            .lineLimit(2)
                                    }
                                }
                                Spacer()
                                Text(sub.buildSystem)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                            .padding(.vertical, 8)
                            .padding(.horizontal, 8)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .rowStyle(selected: selectedSubproject?.id == sub.id, theme: theme)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .cardStyle(theme: theme)
    }

    /// Select a build target from a status row — mirrors exactly what the tree
    /// rows do, including the bridge mirror the right-hand action pane reads.
    private func selectTarget(_ target: BuildTarget, in sub: SubprojectInfo) {
        selectedBuildTarget = target.name
        bridge.selectedBuildTarget = target.name
        selectedSubproject = sub
        bridge.selectSubproject(sub)
    }

    // MARK: - Build targets

    private var buildTargetsSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Build Targets")
                .font(.headline)

            let sub = selectedSubproject ?? project.subprojects.first { $0.buildSystem.isEmpty == false }
            if let sub, !sub.buildTargets.isEmpty {
                // Per-target rows carry the last build state + date, and select
                // the target when tapped.
                PlatformBuildStatusList(
                    project: project,
                    subproject: sub,
                    selectedTarget: selectedBuildTarget,
                    onSelect: { selectTarget($0, in: sub) }
                )
            } else if let sub {
                Text("No targets — build the whole subproject")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                Text("None")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        }
        .cardStyle(theme: theme)
    }

    // MARK: - Helpers

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

// MARK: - Shared card styling

/// The darker "card" treatment used by the analysis pane sections:
/// rounded `theme.surface` fill + subtle border + padding.
extension View {
    func cardStyle(theme: AppTheme) -> some View {
        self
            .padding(10)
            .background(RoundedRectangle(cornerRadius: 8).fill(theme.surface))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(theme.border, lineWidth: 0.5))
    }

    /// Selectable row inside a card: darker highlight when selected.
    func rowStyle(selected: Bool, theme: AppTheme) -> some View {
        self
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(selected ? theme.accentBackground : theme.nodeBackground)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(selected ? theme.accent : theme.border, lineWidth: 1)
            )
    }
}