import SwiftUI
import AppKit

/// The right-hand pane: contextual actions for the current state.
///
///   • no project   → Open Project… / New Project…
///   • creating     → the new-project wizard (Embedded/Native)
///   • opening      → progress
///   • empty folder → Structure Project…
///   • project open → project workflows (build/test/lint/fix/plan),
///                    build log + chat
struct ContextActionPane: View {
    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme

    /// Selection lifted from ContentView (kept consistent with the left pane).
    let selectedSubproject: SubprojectInfo?
    let selectedBuildTarget: String?

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
                    // With a plan, the plan. Without one there is no wizard any more — a project type
                    // is chosen on the welcome screen and scaffolded directly — so this is what is
                    // left of a hand-off that has not happened yet.
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
                    } else {
                        projectWorkflows(project)
                    }
                }
            }
            .padding(12)
        }
        .background(theme.background)
    }

    /// The design's own header row: what the application is written from, and the way to look at it.
    ///
    /// It sits at the top of the pane, above the components and the actions, because the units below it
    /// are *written from* this design — the thing a Write is written from is worth offering before it, not
    /// only through a file a person has to know to open. Read-only: opening it changes nothing.
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
        // The **design**, named at the top of the pane and one click away — see `designHeader`. Gated on
        // the project's declared structure (as the sections below are), not on the selection, so it does
        // not appear and vanish as the user clicks around the tree.
        if project.subprojects.contains(where: { $0.structure == "idf_application" }) {
            designHeader(project)
        }

        if let sub = selectedSubproject ?? project.subprojects.first(where: { !$0.buildSystem.isEmpty }) {
            if sub.buildSystem == "Cargo" {
                dependencyCard
            }
        }

        // An ESP-IDF **component library** grows by adding components, and that is a different
        // surface from the build/lint/plan actions below it: `components/*` *is* the product.
        //
        // Gated on the project's declared structure — a fact about the project — rather than on the
        // selection, so the section does not appear and vanish as the user clicks around the tree.
        if project.subprojects.contains(where: { $0.structure == "idf_library" }) {
            ComponentSection(project: project)
        }

        // An ESP-IDF **application** is *made of* components rather than growing them — an actor per
        // job, a stage per edge of the data path, the message types they share — so what it is built
        // from is worth listing for the same reason a library's product is. Gated on the project's
        // declared structure (as above), not on the selection, so the section does not appear and
        // vanish as the user clicks around the tree.
        //
        // Read-only: it says what the application is made of, and nothing here writes to any of them.
        if project.subprojects.contains(where: { $0.structure == "idf_application" }) {
            ComponentList(project: project)
        }

        // The "Container" card used to sit here: add a board's BSP, or a device driver. It went with
        // the container itself — an ESP-IDF project grows a component with `idf_add_component`, and on
        // ESP-IDF a BSP is a component like any other. A project that still declares `embedded` is
        // *recognized* and still builds; what it no longer has is a tool that adds to it.

        // Full action surface: build/test/lint/fix/plan + live log + chat.
        ActionPanelView(
            project: project,
            selectedSubproject: selectedSubproject,
            selectedBuildTarget: selectedBuildTarget,
            selectedFilePath: .constant(nil),
            onOpenFile: { _ in }
        )
        .frame(maxWidth: .infinity)
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
