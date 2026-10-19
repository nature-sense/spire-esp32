import SwiftUI
import AppKit

/// The right-hand pane: **selection-driven**.
///
///   • no project   → Open Project… / New Project…
///   • creating     → the new-project wizard (Embedded/Native)
///   • opening      → progress
///   • empty folder → Structure Project…
///   • project row  → the project's own surface: the design it is written from, the way to add
///                    a component, and the build/test/lint/fix/plan actions
///   • a component  → that component's detail: the fill-prose box, Write, History, Remove
///
/// That last branch is why this pane reads the selection: the navigator on the left is the one
/// selection surface, and what is selected decides what the pane is *for*. A component is not a
/// smaller project — it is the thing a library grows and an application is made of — so it gets
/// its own detail rather than the project's action cards with a name in front of them.
struct ContextActionPane: View {
    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme

    /// Selection lifted from ContentView (kept consistent with the navigator).
    let selectedSubproject: SubprojectInfo?
    let selectedBuildTarget: String?

    /// The add-component sheet, and its result — shown on the project's surface, where the
    /// button is, so a failed add is readable without reopening anything.
    @State private var addingComponent = false
    @State private var addFailure: String?
    @State private var addOutcome: String?

    /// The open project, when there is one to act on.
    private var openProject: ProjectInfo? {
        if case .idle(let project) = bridge.state, !project.isEmpty { return project }
        return nil
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                switch bridge.state {
                case .unconnected:
                    // Unreachable: `ContentView` renders the welcome screen full-width in this
                    // state, with no pane split. Open and New both live there now.
                    EmptyView()
                case .opening:
                    statusPad("Opening project…")
                case .creating, .scaffolding, .filling:
                    // With a plan, the plan. Without one there is no wizard any more — a project
                    // type is chosen on the welcome screen and scaffolded directly — so this is
                    // what is left of a hand-off that has not happened yet.
                    if bridge.creationPlan != nil {
                        PlanView()
                            .frame(maxWidth: .infinity)
                    } else {
                        statusPad("Starting a new project…")
                    }
                case .error(let message):
                    errorPad(message)
                case .idle(let project):
                    if project.isEmpty {
                        emptyProjectActions(project)
                    } else if let component = selectedComponent(in: project) {
                        ComponentDetailView(project: project, component: component)
                    } else {
                        projectWorkflows(project)
                    }
                }
            }
            .padding(12)
        }
        .background(theme.background)
        .sheet(isPresented: $addingComponent) {
            if let project = openProject {
                ComponentSheet(root: project.root) { finishAdding($0) }
            }
        }
    }

    /// The selected component, resolved against the *current* analysis.
    ///
    /// The selection is a snapshot (`bridge.selectedSubproject`), so a component written since
    /// it was selected would otherwise be described by stale fields. Resolving by id — the path,
    /// which cannot collide — reads the current tree; the snapshot is kept as the fallback for
    /// the moment between a selection and the next analysis refresh.
    private func selectedComponent(in project: ProjectInfo) -> SubprojectInfo? {
        guard let selected = selectedSubproject, selected.kind == .component else { return nil }
        return project.subprojects.first { $0.id == selected.id } ?? selected
    }

    /// The project row was selected (nothing is) — or a non-component subproject was. The
    /// actions belong to the *buildable* thing: the selection when there is one, otherwise the
    /// project's own subproject (the app or library root), which is what Build/Flash drive.
    ///
    /// The root is preferred over "the first subproject with a build system" because an ESP-IDF
    /// project's *components* also carry a build system: the first one in the analyzer's order
    /// is not the thing a project-level Build should build.
    private func activeSubproject(in project: ProjectInfo) -> SubprojectInfo? {
        if let selectedSubproject { return selectedSubproject }
        return project.subprojects.first {
            $0.kind == .project && ($0.path.isEmpty || $0.path == "/")
        } ?? project.subprojects.first {
            $0.kind != .component && !$0.buildSystem.isEmpty
        } ?? project.subprojects.first { !$0.buildSystem.isEmpty }
    }

    private func isIDFApplication(_ project: ProjectInfo) -> Bool {
        project.subprojects.contains { $0.structure == "idf_application" }
    }

    private func isIDFLibrary(_ project: ProjectInfo) -> Bool {
        project.subprojects.contains { $0.structure == "idf_library" }
    }

    /// The design's own header row: what the application is written from, and the way to look at it.
    ///
    /// It sits at the top of the pane, above the components and the actions, because the units
    /// below it are *written from* this design — the thing a Write is written from is worth
    /// offering before it, not only through a file a person has to know to open. Read-only:
    /// opening it changes nothing.
    private func designHeader(_ project: ProjectInfo) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "ruler")
                .font(.caption)
                .foregroundStyle(theme.accent)
            Text("Application design")
                .font(.headline)
            Spacer(minLength: 0)
            Button {
                CompositionViewerPortal.open(bridge: bridge, theme: theme, root: project.root)
            } label: {
                Label("View design", systemImage: "doc.text.magnifyingglass")
            }
            .buttonStyle(.borderless)
            .controlSize(.small)
            .help("The composition this application is written from — read-only")
        }
    }

    // MARK: - Empty project

    /// A folder is open and there is no project in it. The list is the same one the welcome screen
    /// shows, with the location already answered — the folder *is* the location — so a row here only
    /// needs a name. That is why "Structure Project…" is gone: it led to a wizard, and this is the
    /// same decision without one.
    private func emptyProjectActions(_ project: ProjectInfo) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Set Up Project")
                .font(.headline)
            Text("This folder has no project in it. Choose what it should be.")
                .font(.caption)
                .foregroundStyle(.secondary)
            ProjectTypePicker(fixedLocation: project.root)
                .padding(.top, 2)
        }
    }

    // MARK: - Open project: workflows, log + chat

    @ViewBuilder
    private func projectWorkflows(_ project: ProjectInfo) -> some View {
        // The **design**, named at the top of the pane and one click away — see `designHeader`.
        // Gated on the project's declared structure, not on the selection, so it does not appear
        // and vanish as the user clicks around the navigator.
        if isIDFApplication(project) {
            designHeader(project)
        }

        // An ESP-IDF **component library** grows by adding components, and that is a different
        // surface from the build/lint/plan actions below it: `components/*` *is* the product.
        // The components themselves are listed once, in the navigator; this is the one action
        // that makes a new one, and it stays here because adding is a project-level decision.
        if isIDFLibrary(project) {
            addComponentCard
        }

        if activeSubproject(in: project)?.buildSystem == "Cargo" {
            dependencyCard
        }

        // Full action surface: build/test/lint/fix/plan + live log + chat.
        ActionPanelView(
            project: project,
            selectedSubproject: activeSubproject(in: project),
            selectedBuildTarget: selectedBuildTarget,
            selectedFilePath: .constant(nil),
            onOpenFile: { _ in }
        )
        .frame(maxWidth: .infinity)
    }

    /// The one action a library's project surface owns: make a new component.
    ///
    /// It is a card rather than a row of buttons because it carries the result of the last add —
    /// the sheet is gone by then, and what was asked belongs beside the way back in.
    private var addComponentCard: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Label("Components", systemImage: "shippingbox")
                    .font(.callout.weight(.semibold))
                Spacer(minLength: 0)
                Button {
                    addFailure = nil
                    addOutcome = nil
                    addingComponent = true
                } label: {
                    Label("Add", systemImage: "plus").font(.caption)
                }
                .buttonStyle(.bordered)
                .help("Add a driver or a library component")
            }
            Text("A driver is one device on one bus, and its protocol is written from what you know "
                 + "about the device. A library component is pure code, written from what you say "
                 + "it should do. Either way its host test is the gate.")
                .font(.caption)
                .foregroundStyle(theme.textTertiary)
            if let addFailure {
                Text(addFailure)
                    .font(.caption)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
            }
            if let addOutcome {
                Text(addOutcome)
                    .font(.caption2)
                    .foregroundStyle(theme.textSecondary)
                    .textSelection(.enabled)
                    .lineLimit(20)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).fill(theme.surface))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(theme.border, lineWidth: 0.5))
    }

    private func finishAdding(_ result: ComponentSheet.Result) {
        switch result {
        case .ok(let instruction, let summary):
            // The sheet is gone by the time this is read, so what was asked is echoed above what
            // the run did. A stub add asks for nothing, so its empty instruction contributes no
            // line.
            let asked = instruction.trimmingCharacters(in: .whitespacesAndNewlines)
            addOutcome = asked.isEmpty ? summary : "You asked:\n\(asked)\n\n\(summary)"
            Task { await bridge.fetchProjectAnalysis() }
        case .failed(let message):
            addFailure = message
        }
    }

    private var dependencyCard: some View {
        card(title: "Dependencies", icon: "shippingbox") {
            Text("Use the Dependencies tab in the detail views.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    // MARK: - Status helpers

    private func statusPad(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Actions")
                .font(.headline)
            ProgressView()
                .controlSize(.small)
            Text(text)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func errorPad(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Connection Error")
                .font(.headline)
            Text(message)
                .font(.caption)
                .foregroundStyle(.secondary)
            Button("Retry") {
                Task { await bridge.checkConnection() }
            }
            .buttonStyle(.borderedProminent)
        }
    }

    // MARK: - Card helpers

    private func card<Content: View>(title: String, icon: String,
                                     @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(title, systemImage: icon)
                .font(.callout.weight(.semibold))
            content()
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).fill(theme.surface))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(theme.border, lineWidth: 0.5))
    }
}
