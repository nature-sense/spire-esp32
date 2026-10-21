import SwiftUI
import AppKit
import os
import HighlightSwift

/// Main screen: header, project analysis on top, and the source tree plus
/// dependencies below. Draggable dividers resize the analysis and the two
/// bottom panes.
struct MainView: View {
    let project: ProjectInfo
    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme

    @State private var topHeight: CGFloat = 0   // 0 = default 50% (left top/bottom)
    @State private var bottomPaneFrac: CGFloat = 0.5 // default 50% split (bottom L/R)
    @State private var selectedFilePath: String?
    /// Target-scoped detail (files/deps) fetched from the graph when a
    /// build target is selected — drives the bottom panes.
    @State private var targetDetail: BuildTargetDetail?
    @State private var isDraggingDivider = false

    private let dividerWidth: CGFloat = 4
    private let headerHeight: CGFloat = 44

    /// Selection lives on the bridge, not in this view.
    ///
    /// It used to be `@State` here, mirrored into `bridge.selectedSubproject` for the right
    /// pane to read. Two copies of one selection are two things to keep in step: a removal
    /// from the right pane could clear the bridge's copy while this one kept showing the
    /// component in the navigator and the file tree. The bridge is the one copy now, and the
    /// navigator writes straight to it.
    private var selectedSubproject: SubprojectInfo? { bridge.selectedSubproject }
    private var selectedBuildTarget: String? { bridge.selectedBuildTarget }

    var body: some View {
        VStack(spacing: 0) {
            // ── Full-width header: shows project / subproject (+ target) ──
            header
                .frame(maxWidth: .infinity)
                .frame(height: headerHeight)
                .background(theme.accentBackground)
            Divider()

            GeometryReader { geo in
                let totalW = geo.size.width
                let totalH = geo.size.height

                // Vertical split: Analysis (top) / Sources + Dependencies (bottom).
                let top = topHeight == 0 ? (totalH / 2) : max(140, min(totalH - 140, topHeight))
                let bottomW = totalW - dividerWidth
                let bottomLeftW = max(160, bottomW * bottomPaneFrac)

                VStack(spacing: 0) {
                    // Top: the navigator — the project row, its components (or subprojects),
                    // and the selected subproject's build targets in one list.
                    VStack(spacing: 0) {
                        ProjectNavigator(
                            project: project,
                            selectedSubproject: Binding(
                                get: { bridge.selectedSubproject },
                                set: { bridge.selectSubproject($0) }
                            ),
                            selectedBuildTarget: Binding(
                                get: { bridge.selectedBuildTarget },
                                set: { bridge.selectedBuildTarget = $0 }
                            )
                        )
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                    .frame(height: top)

                    // Draggable horizontal divider (top/bottom).
                    divider
                        .frame(height: dividerWidth)
                        .gesture(dragVertical { delta in
                            topHeight = min(max(140, top + delta), totalH - 140)
                        })

                    // Bottom: file tree + dependencies.
                    HStack(spacing: 0) {
                        VStack(spacing: 0) {
                            paneHeader("Sources")
                            bottomFileTree
                        }
                        .frame(width: bottomLeftW)
                        .background(theme.surface)
                        divider
                            .frame(width: dividerWidth)
                            .gesture(DragGesture(minimumDistance: 0)
                                .onChanged { value in
                                    let delta = value.translation.width / max(bottomW, 1)
                                    bottomPaneFrac = min(max(0.15, bottomPaneFrac + delta), 0.85)
                                })
                        bottomDependencies
                            .frame(maxWidth: .infinity)
                            .background(theme.surface)
                    }
                }
                .frame(maxWidth: .infinity)
                .frame(maxHeight: .infinity)
            }
            .onAppear {
                // The Device group keys off the registered board MCP servers, so
                // make sure that list is loaded before a target is selected.
                Task { if bridge.mcpServers.isEmpty { await bridge.fetchMcpServers() } }
                // No default subproject is selected: the navigator opens on the **project
                // row**, which is the honest starting point now that the project surface is
                // its own branch. The right pane resolves the primary buildable subproject
                // for its Build/Flash card, and the Dependencies pane falls back to it, so
                // nothing is missing until the user picks something.
            }
            .onChange(of: selectedBuildTarget) { _, newValue in
                Task { await loadTargetDetail(name: newValue) }
            }

        }
    }

    /// Full-width header showing the selected project / subproject / target.
    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "square.grid.2x2")
                .foregroundStyle(theme.accent)
            if let sub = selectedSubproject {
                Text(project.name)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                Image(systemName: "chevron.right")
                    .font(.caption2).foregroundStyle(.tertiary)
                Text(sub.name)
                    .font(.headline)
                if let target = selectedBuildTarget {
                    Image(systemName: "chevron.right")
                        .font(.caption2).foregroundStyle(.tertiary)
                    Text(target)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            } else {
                Text(project.name)
                    .font(.headline)
            }
            Spacer()
            // Uncommitted-changes safety net: always visible in the header and
            // refreshed after every action, so a destructive step (a formatter
            // or a mis-aimed "fix") can never pass unnoticed.
            GitChangesBadge(project: project)
        }
        .padding(.horizontal, 12)
    }

    /// Recursively insert every file from a real directory node into `root`,
    /// using the file's full project-relative path (the tree's file paths are
    /// already project-relative).
    private func collectFiles(from dir: FileTreeDirectory, into root: inout FileTreeDirectory) {
        for file in dir.files {
            insertFile(file.path, role: file.role, into: &root)
        }
        for sub in dir.directories {
            collectFiles(from: sub, into: &root)
        }
    }

    /// Bottom-left of the left half: source file tree. Shows the whole project
    /// unless a subproject/target/domain is selected (then that slice's files).
    /// File clicks open in a floating window (never inline).
    private var bottomFileTree: some View {
        Group {
            if selectedBuildTarget != nil, let detail = targetDetail, !detail.files.isEmpty {
                // A build target is selected → synthesize the tree from the
                // authoritative graph-backed target detail (files+deps).
                if let tree = treeFromTargetFiles(detail.files) {
                    FileTreeBrowser(tree: tree, selectedFilePath: $selectedFilePath) { path in
                        openFileInPopup(path)
                    }
                } else {
                    ContentUnavailableView("No files", systemImage: "doc",
                        description: Text("No source files for \(detail.name)"))
                }
            } else if let sub = selectedSubproject {
                // Subproject selected (no target) → reuse/synthesize its tree.
                if let tree = subprojectTree(sub) {
                    FileTreeBrowser(tree: tree, selectedFilePath: $selectedFilePath) { path in
                        openFileInPopup(path)
                    }
                } else {
                    ContentUnavailableView("No files", systemImage: "doc",
                        description: Text("No source files for \(sub.name)"))
                }
            } else if let tree = project.fileTree {
                // Project selected → complete file tree.
                FileTreeBrowser(tree: tree, selectedFilePath: $selectedFilePath) { path in
                    openFileInPopup(path)
                }
            } else {
                ContentUnavailableView("No file tree", systemImage: "tree",
                    description: Text("File tree unavailable for this selection"))
            }
        }
    }

    /// Shared header label used above each pane ("Project", "Sources",
    /// "Dependencies", "Actions").
    private func paneHeader(_ title: String) -> some View {
        VStack(spacing: 0) {
            Text(title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(theme.textSecondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 8)
                .padding(.vertical, 5)
            Divider()
                .overlay(theme.divider)
        }
    }

    /// Build a `FileTreeDirectory` from `BuildTargetFile`s (graph-backed).
    private func treeFromTargetFiles(_ files: [BuildTargetFile]) -> FileTreeDirectory? {
        guard !files.isEmpty else { return nil }
        var root = FileTreeDirectory(name: selectedSubproject?.name ?? "target", path: ".", role: "")
        for file in files {
            guard let path = file.path else { continue }
            insertFile(path, role: file.role ?? "", into: &root)
        }
        return root
    }

    /// Bottom-right of the left half: selected subproject's dependencies
    /// (target-scoped detail when a build target is selected).
    private var bottomDependencies: some View {
        Group {
            // Dependencies appear only for a PLATFORM domain (rpi5 / rock3c).
            // The project root and `common` show files without dependencies.
            let deps: [Dependency]? =
                (selectedBuildTarget != nil ? targetDetail?.dependencies.map {
                    Dependency(name: $0.name, version: $0.version)
                } : nil)
                ?? selectedSubproject?.dependencies
                // Single-project cross builds: the main subproject carries the
                // [dependencies]; fall back to it when nothing is selected.
                ?? project.subprojects.first { $0.kind != .directory }?.dependencies
            if let deps, !deps.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    paneHeader("Dependencies")
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 4) {
                            ForEach(deps) { dep in
                                HStack(spacing: 6) {
                                    Image(systemName: "shippingbox")
                                        .foregroundStyle(.secondary)
                                    Text(dep.name)
                                        .font(.callout)
                                    Spacer()
                                    if let v = dep.version, !v.isEmpty {
                                        Text(v)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                                .padding(.horizontal, 8)
                            }
                        }
                        .padding(.vertical, 4)
                    }
                }
            } else {
                VStack(spacing: 8) {
                    Image(systemName: "shippingbox")
                        .font(.system(size: 28))
                        .foregroundStyle(.tertiary)
                    Text(selectedSubproject == nil
                         ? "No subproject selected"
                         : "No dependencies")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    /// Build the subproject's file tree. First try to reuse the rich project
    /// tree (which carries dirs + totals) by walking its path; if the walk
    /// fails (path mismatch), synthesize a tree from `sub.files` so the pane
    /// still shows exactly the subproject's files.
    private func subprojectTree(_ sub: SubprojectInfo) -> FileTreeDirectory? {
        // 1. Walk the project tree by the subproject's relative path.
        if let root = project.fileTree, !sub.path.isEmpty {
            let cleanPath = sub.path.hasSuffix("/") ? String(sub.path.dropLast()) : sub.path
            let parts = cleanPath.split(separator: "/").map(String.init)
            var current = root
            var matched = true
            for part in parts {
                if let next = current.directories.first(where: { $0.name == part }) {
                    current = next
                } else {
                    matched = false
                    break
                }
            }
            if matched { return current }
        }

        // 2. Fallback: synthesize from the subproject's analyzed file list.
        guard let files = sub.files, !files.isEmpty else { return nil }
        let cleanName = sub.name
        var rootDir = FileTreeDirectory(name: cleanName, path: cleanName, role: "")
        for file in files {
            insertFile(file.path, role: file.role, into: &rootDir)
        }
        return rootDir
    }

    /// Insert a single (relative) file path into a synthesized directory tree,
    /// creating directory nodes as needed. Recursive so no `&` re-binding is
    /// required (Swift forbids binding `inout` locals to array elements).
    private func insertFile(_ path: String, role: String, into dir: inout FileTreeDirectory) {
        let parts = path.split(separator: "/").map(String.init)
        guard let leaf = parts.last else { return }
        let dirParts = Array(parts.dropLast())
        insertDirPath(dirParts, fileLeaf: leaf, fullPath: path, role: role, into: &dir)
    }

    private func insertDirPath(_ dirParts: [String],
                               fileLeaf: String,
                               fullPath: String,
                               role: String,
                               into dir: inout FileTreeDirectory) {
        guard let head = dirParts.first else {
            if !dir.files.contains(where: { $0.path == fullPath }) {
                dir.files.append(FileTreeFile(
                    name: fileLeaf,
                    path: fullPath,
                    extension: (fileLeaf as NSString).pathExtension,
                    language: "",
                    size: 0,
                    linesEstimated: 0,
                    role: role
                ))
            }
            return
        }
        let rest = Array(dirParts.dropFirst())
        let base = (dir.path.isEmpty || dir.path == ".") ? "" : dir.path
        let childPath = base.isEmpty ? head : base + "/" + head
        if let idx = dir.directories.firstIndex(where: { $0.name == head }) {
            insertDirPath(rest, fileLeaf: fileLeaf, fullPath: fullPath, role: role, into: &dir.directories[idx])
        } else {
            var newDir = FileTreeDirectory(name: head, path: childPath, role: "")
            insertDirPath(rest, fileLeaf: fileLeaf, fullPath: fullPath, role: role, into: &newDir)
            dir.directories.append(newDir)
        }
    }

    private func dragVertical(_ move: @escaping (CGFloat) -> Void) -> some Gesture {
        DragGesture(minimumDistance: 1)
            .onChanged { value in
                isDraggingDivider = true
                move(value.translation.height)
            }
            .onEnded { _ in
                isDraggingDivider = false
            }
    }

    /// Fetch the target-scoped detail when a target is selected so the
    /// bottom panes show only that target's files + deps.
    private func loadTargetDetail(name: String?) async {
        guard let name, !name.isEmpty else {
            targetDetail = nil
            return
        }
        if let detail = try? await bridge.fetchBuildTarget(name: name) {
            targetDetail = detail
        } else {
            targetDetail = nil
        }
    }

    /// Open a source file in a separate, large, NON-modal popup window so the
    /// workspace keeps showing the target's tabs instead of being replaced.
    private func openFileInPopup(_ path: String) {
        FilePortal.open(path, projectRoot: project.root, bridge: bridge)
    }

    private var divider: some View {
        Rectangle()
            .fill(isDraggingDivider ? theme.accent : theme.divider)
            .contentShape(Rectangle())
            .onHover { hovering in
                if hovering {
                    NSCursor.resizeLeftRight.push()
                } else {
                    NSCursor.pop()
                }
            }
    }
}

/// Opens source files in large, NON-modal popup windows so the workspace
/// keeps its panes instead of being replaced by a file viewer.
/// Multiple files can be open simultaneously; each has its own resizable window.
private enum FilePortal {
    /// Strong references to open windows so they survive the `open` call.
    private static var windows: [NSWindow] = []

    @MainActor
    static func open(_ path: String, projectRoot: String?, bridge: SpireBridge) {
        // Resolve the absolute path the same way FileDetailPanel does.
        let abs: String
        if path.hasPrefix("/") {
            abs = path
        } else if let root = projectRoot {
            abs = (root as NSString).appendingPathComponent(path)
        } else {
            abs = path
        }

        let title = (abs as NSString).lastPathComponent
        // Large initial size: ~90% of the MAIN WINDOW's frame (falls back to the
        // screen's visible frame if no main window is available yet). Keeps the
        // popup proportionally scaled to the app window. NSHostingController
        // sizing can shrink the window to its SwiftUI fitting size, so we
        // enforce the size AFTER setting the content view controller.
        let baseFrame = NSApp.mainWindow?.frame
            ?? NSScreen.main?.visibleFrame
            ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let initialSize = NSSize(
            width: max(960, min(2200, baseFrame.width * 0.90)),
            height: max(640, min(1600, baseFrame.height * 0.90))
        )
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: initialSize.width, height: initialSize.height),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = title
        window.isReleasedWhenClosed = false
        window.setFrameAutosaveName("FilePopup-\(abs)")

        // Build the pane, injecting the bridge environment (a separate NSWindow
        // does NOT inherit the main window's SwiftUI environment).
        let panel = FileDetailPanel(filePath: abs, projectRoot: projectRoot) {
            window.close()
        }
        .environment(bridge)
        .frame(minWidth: 900, minHeight: 600)
        window.contentViewController = NSHostingController(rootView: panel)
        // Force the large initial size AFTER assigning the hosting controller —
        // otherwise AppKit sizes the window to the controller's preferred size
        // (which can be tiny for SwiftUI content).
        window.setContentSize(initialSize)
        // Center the popup over the main window (fall back to screen center).
        if let mainFrame = NSApp.mainWindow?.frame {
            window.setFrameOrigin(NSPoint(
                x: mainFrame.midX - initialSize.width / 2,
                y: mainFrame.midY - initialSize.height / 2
            ))
        } else {
            window.center()
        }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        // Retain; drop the reference when the window closes.
        windows.append(window)
        NotificationCenter.default.addObserver(
            forName: NSWindow.willCloseNotification,
            object: window,
            queue: .main
        ) { _ in
            windows.removeAll { $0 === window }
        }
    }
}

/// Placeholder detail panel for a selected file.
private struct FileDetailPanel: View {
    @Environment(SpireBridge.self) private var bridge
    let filePath: String
    /// The open project's absolute root (passed synchronously so relative
    /// tree paths resolve even if bridge.projectRoot isn't set yet).
    let projectRoot: String?
    /// Called when the user closes the file viewer.
    var onClose: () -> Void = {}

    @State private var content: String?
    @State private var loading = false

    /// Resolve the (possibly relative) tree path against the project root.
    private var absolutePath: String {
        if filePath.hasPrefix("/") { return filePath }
        if let root = projectRoot ?? bridge.projectRoot {
            return (root as NSString).appendingPathComponent(filePath)
        }
        return filePath
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text((filePath as NSString).lastPathComponent)
                    .font(.headline)
                    .lineLimit(1)
                Spacer()
                Button(action: onClose) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Close file viewer")
            }
            .padding(8)
            Divider()
            if loading {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let content, !content.isEmpty {
                ScrollView([.horizontal, .vertical]) {
                    Text(SyntaxHighlighter.highlight(content, language: SyntaxLanguage.detect(from: filePath)))
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                }
            } else {
                Text("Unable to read file")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .padding(8)
            }
        }
        .task(id: filePath) {
            loading = true
            // Tree paths are relative — resolve to an absolute path first.
            let abs = absolutePath
            NSLog("[FileDetailPanel] path=%@ abs=%@ root=%@", filePath, abs, projectRoot ?? "nil")
            if let viaBridge = await bridge.readFile(at: abs), !viaBridge.isEmpty {
                content = viaBridge
                NSLog("[FileDetailPanel] loaded via bridge (%d chars)", viaBridge.count)
            } else {
                content = try? String(contentsOfFile: abs, encoding: .utf8)
                NSLog("[FileDetailPanel] disk read result: %d chars", content?.count ?? 0)
            }
            loading = false
        }
    }
}

/// Right pane — context-aware action buttons (hardcoded workflows) with the
/// live build output shown inline beneath the button row.
struct ActionPanelView: View {
    @Environment(SpireBridge.self) private var bridge
    @Environment(AppTheme.self) private var theme
    let project: ProjectInfo
    let selectedSubproject: SubprojectInfo?
    /// The build target selected in the left-pane graph (e.g. `ai-trap-rock3c`).
    /// Passed straight to the build module — no target detection needed.
    let selectedBuildTarget: String?
    @Binding var selectedFilePath: String?
    /// Called when a build diagnostic's file is tapped — opens a popup viewer.
    var onOpenFile: ((String) -> Void)? = nil

    /// Action result toast state.
    @State private var resultText: String?
    @State private var showResult = false

    /// True while a hardcoded action is running.
    @State private var runningAction: String?

    /// The verb of the most recently completed action ("Build", "Test",
    /// "Lint", "Clean", "Fix warnings") — used by the log header once the
    /// running action is cleared.
    @State private var lastActionVerb: String = "Build"

    /// Persistent, accumulated build log across tasks for the current
    /// subproject (never auto-cleared between runs). Cleared on subproject
    /// switch or via the log's clear button.
    @State private var logHistory: [SpireBridge.BuildEventLine] = []


    /// Container view model for the build panel. Owns async build state + the
    /// single live-event consumer.
    @State private var buildViewModel: BuildPanelViewModel?
    @State private var showPlanSheet: Bool = false

    /// Free-text `modify/code` request: the sheet, and what the user typed.
    @State private var showModify: Bool = false
    @State private var modifyPrompt: String = ""
    /// The project-type list, for a folder that has no project in it yet.
    @State private var showingStructurePicker: Bool = false
    /// Text for the chat prompt input box.
    @State private var chatInput: String = ""

    /// Lazily create the build panel VM from the bridge's **shared** service.
    ///
    /// Shared, not private: the pane's activity log reads the same stream, and the Rust side wakes
    /// exactly one waiter per event — a second service would take lines away from this one.
    private func ensureBuildViewModel() {
        if buildViewModel == nil {
            buildViewModel = BuildPanelViewModel(service: bridge.sharedBuildService)
            buildViewModel?.startEventConsumer()
        }
    }

    /// Human verb for the currently running/completed action (build/lint/fix).
    private var actionVerb: String {
        switch runningAction {
        case "build_lint": return "Lint"
        case "build_verify": return "Verify"
        case "build_autofix": return "Fix & Verify"
        case "build_fix": return "Fix warnings"
        case "build_format": return "Format"
        case "build_clean": return "Clean"
        case "build_test": return "Test"
        case "build_build": return "Build"
        case "build_flash": return "Flash"
        default: return "Build"
        }
    }

    private var language: String {
        if let sub = selectedSubproject {
            // Normalize the analyzer's language label (e.g. "🐍 Python", "🦀 Rust")
            // so Python/other runtimes get the correct language passed to tools.
            let raw = sub.language
            if raw.contains("Python") { return "Python" }
            if raw.contains("Rust") { return "Rust" }
            if raw.contains("Swift") { return "Swift" }
            if raw.contains("JavaScript") || raw.contains("Node") { return "JavaScript" }
            if raw.contains("Go") { return "Go" }
            if raw.contains("Java") { return "Java" }
            if raw.contains("Ruby") { return "Ruby" }
            return sub.buildSystem
        }
        return "Rust"
    }

    /// True when the current selection is something that can actually be built:
    /// a build target or a subproject with an executable of its own.
    private var isBuildableSelection: Bool {
        if selectedBuildTarget != nil { return true }
        guard let sub = selectedSubproject else { return false }
        // A legacy HAL structure has no single executable of its own.
        return sub.structure != "hal"
    }

    /// The platform a selected BUILD TARGET builds for (nil when it is a host
    /// target). Used so building a target selects the right Meson build dir
    /// (`build-<plat>`) + cross file — the target knows its own platform.
    private func platformForSelectedTarget() -> String? {
        guard let name = selectedBuildTarget else { return nil }
        for sub in project.subprojects {
            if let bt = sub.buildTargets.first(where: { $0.name == name }),
               !bt.platform.isEmpty, bt.platform != "host" {
                return bt.platform
            }
        }
        return nil
    }

    // ── Device (on-hardware) ──────────────────────────────────────────────
    // A board is a platform that declares `device:` in the registry. Boards are
    // frequently powered off, so nothing here dials one until asked: Connect is
    // explicit, and the two board actions stay disabled until it succeeds.
    /// True while a board connect is in flight.
    @State private var connectingDevice = false
    /// The board action currently running ("test" / "deploy"), if any.
    @State private var runningDevice: String?
    /// Result line under the Device group (success, failure, or error text).
    @State private var deviceNote: String?

    /// The platform whose board the current selection maps to, when that
    /// platform declares one. No board → no Device group (a host is not a
    /// device). Keyed off the registered `device-<platform>` MCP servers, which
    /// is exactly what the backend would connect to.
    private var selectedDevicePlatform: String? {
        let candidate = platformForSelectedTarget()
        guard let candidate, !candidate.isEmpty, candidate != "host" else { return nil }
        let server = SpireBridge.deviceServerName(for: candidate)
        guard bridge.mcpServers.contains(where: { $0.name == server }) else { return nil }
        return candidate
    }

    /// The Meson build directory for a platform (`build-<plat>`).
    private func buildDir(_ platform: String) -> String { "build-\(platform)" }

    /// The cross-built TEST binary for a platform.
    ///
    /// Convention: `<build dir>/<platform>/<project>-<platform>-tests`, which is
    /// what a project's `tests/meson.build` produces for a project named
    /// `<project>` (ai-traps → `ai-traps-rpi5-tests`). A project that names its
    /// tests differently gets a "not found" result naming the full path from the
    /// board, which is self-correcting.
    private func testBinaryPath(_ platform: String, projectName: String) -> String {
        let slug = projectName.lowercased().replacingOccurrences(of: " ", with: "-")
        return "\(buildDir(platform))/\(platform)/\(slug)-\(platform)-tests"
    }

    /// The Meson executable target that builds for `platform` — the production
    /// binary that `deploy` installs on the board.
    private func targetForPlatform(_ platform: String) -> String? {
        if let name = selectedBuildTarget, !name.isEmpty { return name }
        for sub in project.subprojects {
            if let bt = sub.buildTargets.first(where: { $0.platform == platform }) {
                return bt.name
            }
        }
        return nil
    }

    /// The production binary's path within the project for `platform`.
    private func productionBinaryPath(_ platform: String) -> String? {
        guard let target = targetForPlatform(platform) else { return nil }
        return "\(buildDir(platform))/\(platform)/\(target)"
    }

    /// A one-line summary of a board reply (shown under the Device group).
    private func deviceSummary(_ reply: [String: Any]?, verb: String) -> String {
        guard let reply else { return "Board unreachable — is spire-target-mcp running?" }
        if let error = reply["error"] as? String { return error }
        if verb == "deploy", let dest = reply["dest"] as? String {
            return "Deployed to \(dest)"
        }
        if let passed = reply["passed"] as? Bool {
            return passed ? "Board tests passed" : "Board tests failed"
        }
        return "Done"
    }

    /// The Device group: connect the board, then run its tests or deploy to it.
    ///
    /// Both board actions are gated on the connection: there is nothing useful
    /// to do to a board that is not answering, and a disabled button says so
    /// without hiding the capability.
    @ViewBuilder
    private func deviceActions(_ platform: String) -> some View {
        let online = bridge.deviceOnline(platform: platform)
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Image(systemName: "cpu")
                    .foregroundStyle(online ? Color.green : theme.textSecondary)
                Text("Device · \(platform)")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(theme.textSecondary)
                Spacer()
                Text(online ? "connected" : "offline")
                    .font(.caption2)
                    .foregroundStyle(online ? Color.green : theme.textSecondary)
            }

            Button {
                Task {
                    connectingDevice = true
                    deviceNote = "Connecting to \(platform)…"
                    let connected = await bridge.connectDevice(platform: platform)
                    connectingDevice = false
                    deviceNote = connected
                        ? "Connected to \(platform)"
                        : "Could not reach \(platform) — is the board powered on?"
                }
            } label: {
                Label(
                    connectingDevice ? "Connecting…" : (online ? "Connected" : "Connect to board"),
                    systemImage: "cable.connector"
                )
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .disabled(connectingDevice || online)

            Button {
                runningDevice = "test"
                deviceNote = "Building, deploying and running tests on \(platform)…"
                Task {
                    // `build: true` — the action is "run the tests on this board", so it produces
                    // the binary the same way the platform module does rather than depending on one
                    // having been built by hand first. A build failure comes back naming the build,
                    // which is the useful error ("no target installed"), not "file not found".
                    let reply = await bridge.runDeviceTests(
                        platform: platform,
                        path: testBinaryPath(platform, projectName: project.name),
                        build: true
                    )
                    runningDevice = nil
                    deviceNote = deviceSummary(reply, verb: "test")
                    bridge.buildCompletionTick += 1
                }
            } label: {
                Label("Run tests on board", systemImage: "play.circle")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .disabled(!online || runningDevice != nil)

            Button {
                guard let binary = productionBinaryPath(platform) else {
                    deviceNote = "No build target for \(platform) — build it first"
                    return
                }
                runningDevice = "deploy"
                deviceNote = "Deploying \(binary) to \(platform)…"
                Task {
                    let reply = await bridge.deployDeviceBinary(platform: platform, path: binary)
                    runningDevice = nil
                    deviceNote = deviceSummary(reply, verb: "deploy")
                }
            } label: {
                Label("Deploy binary", systemImage: "arrow.down.to.line")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .disabled(!online || runningDevice != nil)

            // Trap control (M5): what the board is running, and the three actions that address it.
            // Below Connect/Run/Deploy because it acts on what those produced.
            Divider()
            DeviceProcessesSection(
                platform: platform,
                artifactPath: productionBinaryPath(platform)
            )

            if let deviceNote {
                Text(deviceNote)
                    .font(.caption2)
                    .foregroundStyle(theme.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 2)
    }

    var body: some View {
        VStack(spacing: 0) {
            // ── Pane header ──
            VStack(spacing: 0) {
                Text("Actions")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(theme.textSecondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                Divider()
                    .overlay(theme.divider)
            }

            // ── Action cards ──
            // Empty project (only .spire metadata): offer the project types, with this folder as the
            // location. There is no wizard any more — the list is the whole decision, and the sheet
            // it opens asks only for a name.
            if project.isEmpty {
                Button {
                    showingStructurePicker = true
                } label: {
                    Label("Structure Project…", systemImage: "wand.and.stars")
                        .font(.headline)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 4)
                }
                .buttonStyle(.borderedProminent)
                .padding(8)
                .sheet(isPresented: $showingStructurePicker) {
                    ProjectTypePicker(fixedLocation: project.root)
                        .padding(20)
                        .frame(width: 460)
                        .background(theme.background)
                }
            } else if selectedSubproject != nil && isBuildableSelection {
                // Build/Test/Lint are shown only for a buildable selection: a
                // build target or a subproject with an executable of its own.
                appCard
            } else if selectedFilePath != nil {
                fileActions
            } else {
                projectActions
            }
            Divider()

            // ── Chat input box ──
            chatInputBox

            Divider()

            // ── Collapsible task list (plan steps) ──
            if bridge.activePlan != nil {
                taskListView
            }

            // ── Live build output (inline, directly below the button row) ──
            // Shown while a tool runs, while a chat-driven run is in flight (`isProcessing`: its
            // model and build lines arrive here too), and whenever there is anything to read.
            let vm = buildViewModel
            if (vm == nil && (!bridge.buildEvents.isEmpty || bridge.buildRunning))
                || (vm != nil && (!vm!.liveEvents.isEmpty || vm!.isRunning))
                || bridge.isProcessing
                || !logHistory.isEmpty {
                buildLogView
            }
            if showPlanSheet {
                PlanSheetView(project: project, selectedSubproject: selectedSubproject)
            }
            Spacer(minLength: 0)
        }
        .sheet(isPresented: $showModify) { modifySheet }
        .task {
            // Start the stream on appearance, not on the first build: a write started from the
            // component pane or from chat has lines too, and a consumer that only exists after a
            // Build click is not there to receive them.
            ensureBuildViewModel()
        }
        .onDisappear {
            // Detach this panel only. The pane's activity log keeps the shared stream alive.
            buildViewModel?.stopEventConsumer()
        }
        .onChange(of: selectedSubproject?.id) { _, _ in
            // New subproject selected — clear stale build output and state so
            // the right pane starts fresh for the new target.
            resultText = nil
            showResult = false
            runningAction = nil
            logHistory = []
            buildViewModel?.liveEvents = []
            bridge.buildEvents = []
        }
    }

    // MARK: - Chat input box

    /// Prompt input row: TextField + Send, wired to the bridge chat.
    private var chatInputBox: some View {
        HStack(spacing: 6) {
            TextField("Send a prompt to the assistant…", text: $chatInput)
                .textFieldStyle(.roundedBorder)
                .onSubmit { sendChat() }
            Button {
                sendChat()
            } label: {
                Image(systemName: "paperplane.fill")
            }
            .buttonStyle(.borderedProminent)
            .disabled(chatInput.trimmingCharacters(in: .whitespaces).isEmpty || bridge.isProcessing)
        }
        .padding(8)
    }

    private func sendChat() {
        let text = chatInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        chatInput = ""
        Task { await bridge.sendChatMessage(text) }
    }

    // MARK: - Collapsible task list

    /// Collapsible plan-step list. Collapsed when there is no active plan.
    private var taskListView: some View {
        DisclosureGroup {
            if let plan = bridge.activePlan {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(Array(plan.steps.enumerated()), id: \.element.id) { _, step in
                        HStack(spacing: 6) {
                            Image(systemName: statusIcon(step.status))
                                .foregroundStyle(statusColor(step.status))
                            Text("\(step.order). \(step.description)")
                                .font(.callout)
                                .lineLimit(2)
                                .textSelection(.enabled)
                        }
                        .padding(.vertical, 1)
                    }
                }
                .padding(.vertical, 4)
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "checklist")
                    .foregroundStyle(theme.accent)
                Text("Tasks")
                    .font(.subheadline.weight(.semibold))
                Spacer()
                if let plan = bridge.activePlan {
                    Text("\(plan.completedSteps)/\(plan.totalSteps)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .contentShape(Rectangle())
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
    }

    private func statusIcon(_ status: String) -> String {
        switch status {
        case "completed": return "checkmark.circle.fill"
        case "failed": return "xmark.circle.fill"
        case "running", "executing": return "play.circle.fill"
        default: return "circle"
        }
    }

    private func statusColor(_ status: String) -> Color {
        switch status {
        case "completed": return .green
        case "failed": return .red
        case "running", "executing": return .orange
        default: return .secondary
        }
    }

    // MARK: - Live build log

    private var buildLogView: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                if buildViewModel?.isRunning ?? bridge.buildRunning {
                    ProgressView().controlSize(.small)
                    Text("\(actionVerb)…").font(.caption.weight(.semibold))
                } else {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                    Text("\(lastActionVerb) complete").font(.caption.weight(.semibold))
                }
                Spacer()
                Button {
                    logHistory = []
                    buildViewModel?.liveEvents = []
                    bridge.buildEvents = []
                } label: {
                    Image(systemName: "trash")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help("Clear build log")
            }
            ScrollViewReader { proxy in
                ScrollView(.vertical, showsIndicators: true) {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        // Persistent historical log + live streaming lines for
                        // the currently running task.
                        let combined = logHistory + (buildViewModel?.liveEvents ?? [])
                        ForEach(Array(combined.enumerated()), id: \.element.id) { _, ev in
                            buildEventRow(ev)
                        }
                    }
                    .padding(4)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .onChange(of: bridge.buildEvents.count) { _, _ in
                    if let last = bridge.buildEvents.last {
                        // Instant scroll — animating every line blocks the main thread.
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
                .onChange(of: buildViewModel?.liveEvents.count ?? 0) { _, _ in
                    // Incremental streaming: scroll on every batch of live events
                    // delivered while the tool runs (not just the final result).
                    if let last = (buildViewModel?.liveEvents ?? bridge.buildEvents).last {
                        proxy.scrollTo(last.id, anchor: .bottom)
                    }
                }
            }
        }
        .padding(8)
        .background(theme.surface, in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(theme.accent.opacity(0.45), lineWidth: 1)
        )
        .padding(8)
    }

    @ViewBuilder
    private func buildEventRow(_ ev: SpireBridge.BuildEventLine) -> some View {
        switch ev.level {
        case "error":
            structuredBlockRow(ev, icon: "xmark.circle.fill", color: .red)
        case "warning":
            structuredBlockRow(ev, icon: "exclamationmark.triangle.fill", color: .yellow)
        case "compiling":
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Image(systemName: "hammer.fill").font(.system(size: 8)).foregroundStyle(.secondary)
                Text(ev.line).font(.caption2.monospaced()).foregroundStyle(.primary).textSelection(.enabled)
            }
        case "finished":
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Image(systemName: "checkmark.circle.fill").font(.system(size: 8)).foregroundStyle(.green)
                Text(ev.line).font(.caption2).foregroundStyle(.green).textSelection(.enabled)
            }
        default:
            Text(ev.line).font(.caption2.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
        }
    }

    /// Renders a warning/error as a compact structured row: icon + message,
    /// with a file:line location when available. The full raw block (code
    /// context) is shown indented beneath for detail.
    private func structuredBlockRow(_ ev: SpireBridge.BuildEventLine,
                                    icon: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                Image(systemName: icon).font(.system(size: 8)).foregroundStyle(color)
                // Compact: show the extracted message; fall back to raw line.
                Text(ev.message ?? ev.line)
                    .font(.caption2)
                    .foregroundStyle(color)
                    .textSelection(.enabled)
                    .lineLimit(3)
            }
            if let file = ev.file {
                Button {
                    // Open the warning's file in a large non-modal popup window.
                    // Path from cargo is relative to the subproject; resolve
                    // against the project root.
                    let root = project.root.hasSuffix("/") ? String(project.root.dropLast()) : project.root
                    let abs = file.hasPrefix("/") ? file : root + "/" + file
                    onOpenFile?(abs)
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "doc.text").font(.system(size: 7)).foregroundStyle(.secondary)
                        Text("\(file)\(ev.lineNumber.map { ":\($0)" } ?? "")")
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Open \(file)\(ev.lineNumber.map { ":\($0)" } ?? "")")
                .padding(.leading, 12)
            }
        }
    }

    // MARK: - Contextual action groups

    private var projectActions: some View {
        HStack(spacing: 6) {
                planButton

            actionButton("Refresh", systemImage: "arrow.clockwise", tool: "project_refresh")
            actionButton("Analyze", systemImage: "magnifyingglass", tool: "build_analyze")
        }
        .padding(8)
    }

    /// True when the selected subproject is built by a toolchain that HAS a real
    /// automatic warning/error fixer. Meson/C++ does not (no clang-tidy), and neither
    /// does ESP-IDF — its module declares `supports_fix: false`, so `build_fix` would
    /// be refused by the manager. On those the card offers `build_build` instead,
    /// which *is* the toolchain's own invocation (`idf.py build`).
    ///
    /// ESP-IDF is asked by **structure**, not by the build-system label: its analysis
    /// belongs to the cmake module (an IDF project *is* a CMake project), so
    /// `buildSystem` reads `"CMake"` and a label check would never see it.
    private var selectionHasAutoFixer: Bool {
        let structure = selectedSubproject?.structure ?? ""
        if structure == "idf_application" || structure == "idf_library" {
            return false
        }
        let bs = (selectedSubproject?.buildSystem ?? "").lowercased()
        return !bs.contains("meson")
    }

    /// True when the selected subproject produces firmware that can be put on a
    /// connected board, so the card offers **Flash** beside Build.
    ///
    /// Asked by **structure**, for the same reason [`selectionHasAutoFixer`] is:
    /// an ESP-IDF application is analysed by the cmake module (it *is* a CMake
    /// project), so `buildSystem` reads `"CMake"` and a label check would never
    /// see it. Only the application image is installable — a `idf_library` is a
    /// component that ships inside one, so it has no board to flash.
    ///
    /// The backend is the arbiter of the rest: `build_flash` needs the platform
    /// registry id, which this card already threads for a build (the analysis
    /// records the chip in `platformTargets`), and the esp-idf module is the one
    /// that declares `supports_flash`.
    private var selectionSupportsFlash: Bool {
        selectedSubproject?.structure == "idf_application"
    }

    /// True when the selection is Meson/C++ — the toolchain with no reliable
    /// auto-fixer, so compile errors are fixed through the LLM review loop.
    private var selectionIsMeson: Bool {
        (selectedSubproject?.buildSystem ?? "").lowercased().contains("meson")
    }

    /// Card 1 — the APP: what has been built, the one action that builds it, and
    /// the board it runs on. Everything here is about the target binary.
    private var appCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            appCardHeader

            // One principal build action, not a toolbar of near-synonyms.
            // `build_autofix` IS the build pipeline — compile → fix errors → lint
            // → fix safe warnings → verify — and it ends with a built binary, so
            // separate Build/Verify/Lint buttons beside it were redundant.
            // `Clean` stays: clearing the build dir is a genuinely separate
            // operation. Tests are deliberately NOT here — a platform's tests run
            // on the board (see the Device group below), not on the Mac.
            HStack(spacing: 8) {
                primaryActionButton
                // The other half of "build": the image it just produced goes onto
                // the board. `build_flash` is the toolchain's own installer
                // (`idf.py flash`), so it is a sibling action, not a menu — and it
                // is shown only for a target that actually has a board to flash.
                if selectionSupportsFlash {
                    squareActionButton("Flash", systemImage: "square.and.arrow.down",
                                       tool: "build_flash", help: "Install the built firmware on the connected board (idf.py flash)")
                }
                // Free-text change: describe it, and it is applied and verified as one
                // change (rolled back unless the project still builds).
                squareActionButton("Modify…", systemImage: "pencil.and.outline",
                                   tool: "modify_code", action: { showModify = true })
                squareActionButton("Clean", systemImage: "trash", tool: "build_clean")
                Spacer()
            }

            // The board this target runs on (only for a platform that declares one).
            if let devicePlatform = selectedDevicePlatform {
                Divider().overlay(theme.divider)
                deviceActions(devicePlatform)
            }
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).fill(theme.surface))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(theme.border, lineWidth: 0.5))
        // Refresh the build readout on selection change and after every action.
        .task(id: "\(project.root)|\(selectedBuildTarget ?? "")|\(bridge.buildCompletionTick)") {
            await loadLastBuild()
        }
    }

    private var fileActions: some View {
        HStack(spacing: 6) {
            Button {
                if let path = selectedFilePath {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(path, forType: .string)
                    flashResult("Copied \(path)")
                }
            } label: {
                actionLabel("Copy Path", systemImage: "doc.on.doc")
            }
            .buttonStyle(.plain)
        }
        .padding(8)
    }

    /// Free-text request for `modify_code`.
    ///
    /// The change is one intent: it is proposed, applied, and then kept or rolled back as
    /// a whole. The note under the field says what "verified" will mean, because the
    /// answer differs depending on whether a board is connected.
    private var modifySheet: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Modify code")
                .font(.headline)
            Text("Describe the change in your own words. It is applied as one change and kept only if the project still builds and the tests that can run still pass — otherwise it is rolled back. Target tests run when a board is connected.")
                .font(.caption)
                .foregroundStyle(theme.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            TextEditor(text: $modifyPrompt)
                .font(.system(.body, design: .monospaced))
                .frame(minHeight: 130)
                .padding(4)
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(theme.border, lineWidth: 0.5)
                )
            HStack {
                Spacer()
                Button("Cancel") { showModify = false }
                Button("Modify") {
                    let request = modifyPrompt.trimmingCharacters(in: .whitespacesAndNewlines)
                    guard !request.isEmpty else { return }
                    showModify = false
                    runTool("modify_code", prompt: request)
                }
                .keyboardShortcut(.defaultAction)
                .disabled(modifyPrompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(16)
        .frame(minWidth: 460)
    }



    // MARK: - Helpers

    private func actionButton(_ title: String, systemImage: String, tool: String) -> some View {
        Button {
            runTool(tool)
        } label: {
            actionLabel(title, systemImage: systemImage)
        }
        .buttonStyle(.plain)
        .disabled(runningAction != nil)
    }

    // MARK: - App card pieces

    /// Last recorded build for the selected target (nil = unknown / never built).
    @State private var lastBuild: BuildStatus?

    /// The app's principal action: the build+fix pipeline where the toolchain has
    /// one (`build_autofix` for Meson; `build_fix` where an autofixer exists),
    /// falling back to a plain build otherwise. This *is* "build" — the steps it
    /// runs internally are not offered as separate buttons beside it.
    @ViewBuilder
    private var primaryActionButton: some View {
        if selectionIsMeson {
            squareActionButton("Fix & Verify", systemImage: "wand.and.stars",
                               tool: "build_autofix", prominent: true)
        } else if selectionHasAutoFixer {
            squareActionButton("Fix Warnings", systemImage: "wrench.and.screwdriver",
                               tool: "build_fix", prominent: true)
        } else {
            squareActionButton("Build", systemImage: "hammer.fill",
                               tool: "build_build", prominent: true)
        }
    }

    /// Title row of the app card: what this card is, and how the last build went.
    private var appCardHeader: some View {
        HStack(spacing: 6) {
            Image(systemName: "hammer.fill").foregroundStyle(theme.accent)
            Text("App").font(.callout.weight(.semibold))
            if let target = selectedBuildTarget {
                Text(target).font(.caption).foregroundStyle(theme.textSecondary)
            }
            Spacer()
            if let text = lastBuildText {
                Text(text)
                    .font(.caption2)
                    .foregroundStyle(lastBuildColor)
            }
        }
    }

    /// "built · 2 min ago · 12.3s", from the build manager's stored status.
    private var lastBuildText: String? {
        guard let status = lastBuild else { return nil }
        var parts: [String] = []
        switch status.success {
        case .some(true): parts.append("built")
        case .some(false): parts.append("build failed")
        default: parts.append("never built")
        }
        if let when = status.lastBuild {
            let fmt = RelativeDateTimeFormatter()
            fmt.unitsStyle = .short
            parts.append(fmt.localizedString(for: when, relativeTo: Date()))
        }
        if let secs = status.durationSecs { parts.append(String(format: "%.1fs", secs)) }
        return parts.joined(separator: " · ")
    }

    private var lastBuildColor: Color {
        switch lastBuild?.success {
        case .some(true): return .green
        case .some(false): return .red
        default: return theme.textSecondary
        }
    }

    /// Load the selected target's last build state. Re-run whenever the selection
    /// changes or an action completes (`buildCompletionTick`).
    private func loadLastBuild() async {
        guard let sub = selectedSubproject, let target = selectedBuildTarget else {
            lastBuild = nil
            return
        }
        lastBuild = await bridge.fetchBuildStatus(
            path: sub.absolutePath(in: project.root), target: target
        )
    }

    /// A square action tile: icon over a short caption, sized so the primary
    /// actions read as actions rather than as body text.
    ///
    /// `prominent` marks the card's principal action, which is drawn larger and
    /// accented. `help` is the tooltip for actions whose purpose is not obvious
    /// from the caption alone.
    private func squareActionButton(_ title: String, systemImage: String, tool: String,
                                    prominent: Bool = false,
                                    help: String? = nil,
                                    action: (() -> Void)? = nil) -> some View {
        return Button {
            // A tile that needs something from the user (a prompt) opens its sheet
            // instead of running the tool straight away.
            if let action {
                action()
            } else {
                runTool(tool)
            }
        } label: {
            VStack(spacing: 4) {
                Image(systemName: systemImage)
                    .font(.system(size: prominent ? 18 : 15, weight: .medium))
                Text(title)
                    .font(.caption2.weight(prominent ? .semibold : .medium))
                    .lineLimit(1)
            }
            .frame(width: prominent ? 96 : 64, height: prominent ? 56 : 50)
            .foregroundStyle(theme.textPrimary)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(prominent ? theme.accentBackground : theme.buttonBackground)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(prominent ? theme.accent : theme.border,
                            lineWidth: prominent ? 1 : 0.5)
            )
        }
        .buttonStyle(.plain)
        .disabled(runningAction != nil)
        .help(help ?? title)
    }

    private var planButton: some View {
        Button {
            showPlanSheet = true
        } label: {
            actionLabel("Plan", systemImage: "map.fill")
        }
        .buttonStyle(.plain)
        .help("Generate an LLM modification plan")
    }

    private func actionLabel(_ title: String, systemImage: String) -> some View {
        Label(title, systemImage: systemImage)
            .font(.caption.weight(.medium))
            .foregroundStyle(theme.textPrimary)
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(theme.buttonBackground)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(theme.border, lineWidth: 0.5)
            )
    }

    private func runTool(_ tool: String, prompt: String? = nil) {
        guard let sub = selectedSubproject else { return }

        // Resolve the subproject directory to an absolute path — EXACTLY the
        // form under which build status / diagnostics are keyed in the graph
        // (see SubprojectInfo.absolutePath(in:)).
        let absPath = sub.absolutePath(in: project.root)

        // DEBUG: log which subproject the tool is targeting.
        Logger(subsystem: "spire", category: "runTool").info("tool=\(tool) sub=\(sub.name) selectedTarget=\(selectedBuildTarget ?? "nil") path=\(sub.path) abs=\(absPath)")
        // Ensure the single BuildService consumer exists. It is idempotent —
        // exactly ONE waiter on the Rust Notify, so notifications can't be stolen.
        ensureBuildViewModel()
        guard let vm = buildViewModel else { return }
        runningAction = tool
        flashResult("\(tool) started\n\(absPath)")
        // Platform resolution: derive it from the selected BUILD TARGET (each
        // target knows its own platform, e.g. ai-trap-rpi5 → rpi5) so the build
        // selects the right Meson dir (`build-rpi5`) and provisions its cross
        // file; finally fall back to the platform targets the analysis recorded.
        //
        // A **single** platform target is normally the only one there is, so it
        // is not an argument a build needs — except an ESP-IDF application's one
        // target is a **chip**: `esp-idf` is a platform module, not a build
        // system, and passing `nil` is what sent a dashboard build of an earlier
        // session's project to `cmake` instead of `idf.py`. The analysis records
        // the chip in `platformTargets` (from the tree's `sdkconfig`), so it
        // arrives here the same way a Meson target's `rpi5` does.
        let platform: String? = {
            if let fromTarget = platformForSelectedTarget() {
                return fromTarget
            }
            let targets = sub.platformTargets
            if sub.structure == "idf_application" {
                return targets.first
            }
            return targets.count > 1 ? targets.first : nil
        }()
        Task {
            await vm.runTool(tool, path: absPath, language: language, package: sub.name, platform: platform, target: selectedBuildTarget, prompt: prompt)
            await MainActor.run {
                // Capture the verb BEFORE clearing the running flag —
                // actionVerb falls back to "Build" once runningAction is nil.
                let verb = actionVerb
                runningAction = nil
                lastActionVerb = verb
                // Nudge the per-platform build-status views to re-read the
                // status the backend just persisted for this target.
                bridge.buildCompletionTick &+= 1
                if let result = vm.state.value {
                    // Populate the build log with ALL collected lines from the build.
                    // For Meson lint/analyze the diagnostics live in `output`
                    // (not the streaming events), so materialize them as lines
                    // here — giving a persistent, visible build-log entry.
                    var events = result.buildEvents
                    for line in result.output.split(separator: "\n") {
                        let trimmed = line.trimmingCharacters(in: .whitespaces)
                        if trimmed.isEmpty { continue }
                        events.append(SpireBridge.BuildEventLine(
                            line: String(line),
                            level: result.success ? "info" : "error",
                            target: nil
                        ))
                    }
                    // Append this task's output to the persistent log with a
                    // timestamp separator — previous runs stay visible above.
                    let df = DateFormatter()
                    df.dateFormat = "HH:mm:ss"
                    let stamp = df.string(from: Date())
                    let statusLabel = result.success ? "✅ succeeded" : "❌ failed"
                    logHistory.append(SpireBridge.BuildEventLine(
                        line: "──── \(verb) \(stamp) \(statusLabel) ────",
                        level: "finished",
                        target: nil
                    ))
                    logHistory.append(contentsOf: events)
                    buildViewModel?.liveEvents = []
                    bridge.buildEvents = logHistory
                    // Surface the tool's actual output in the status banner so
                    // lint/fix results are immediately visible, not buried.
                    let preview = result.output.split(separator: "\n").prefix(25).joined(separator: "\n")
                    flashResult((result.success ? "✅ \(verb) succeeded" : "❌ \(verb) failed") + "\n\n" + preview)
                    // Also push the result into chat so the user can review full output,
                    let msg = ChatMessage(
                        id: UUID().uuidString,
                        role: .system,
                        content: (result.success ? "✅ \(verb) succeeded" : "❌ \(verb) failed") + "\n\n```\n" + result.output + "\n```",
                        timestamp: Date()
                    )
                    bridge.messages.append(msg)
                } else {
                    flashResult("No response from backend")
                }
            }
        }
    }

    private func flashResult(_ text: String) {
        resultText = text
        showResult = true
        // Auto-dismiss after a few seconds.
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) {
            withAnimation {
                showResult = false
            }
        }
    }
}
