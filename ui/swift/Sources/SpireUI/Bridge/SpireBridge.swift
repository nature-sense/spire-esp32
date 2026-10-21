import Foundation
import AppKit
import os
import os
import Observation

/// Central observable state object that communicates with the Rust core.
@Observable
final class SpireBridge {
    /// Shared singleton used by the macOS menu-bar commands so they can
    /// reference the exact same instance as the SwiftUI environment.
    static let shared = SpireBridge()

    /// Single source of truth for the UI's project state machine.
    var state: ProjectState = .unconnected

    // MARK: Derived computed properties (kept for backward compatibility
    // with views that read the old booleans; views should migrate to `state`).

    var connected: Bool {
        if case .idle = state { return true }
        return false
    }
    var connectionError: String? {
        if case .error(let m) = state { return m }
        return nil
    }
    var projectInfo: ProjectInfo? {
        if case .idle(let p) = state { return p }
        return nil
    }
    var loading: Bool {
        if case .opening = state { return true }
        return false
    }
    var showWelcome: Bool {
        if case .unconnected = state { return true }
        return false
    }
    var showNewProject: Bool {
        if case .creating = state { return true }
        return false
    }
    var creationPlan: PlanGenerationResult? {
        if case .creating(let plan, _) = state { return plan }
        return nil
    }
    var creationExecuting: Bool {
        if case .creating(_, let exec) = state { return exec }
        return false
    }

    var messages: [ChatMessage] = []
    /// Last plan-creation error from the backend (nil on success).
    var lastPlanError: String?
    /// The latest generated (or approved/executing) plan. Promoted from
    /// PlanSheetView so the right pane's collapsible task list can render the
    /// plan steps live (status updates after approve/execute).
    var activePlan: PlanStatusResult?
    /// Whether the plan sheet/task-list should be expanded/visible.
    var planVisible = false
    var isProcessing: Bool = false
    var selectedSubproject: SubprojectInfo?
    var selectedBuildTarget: String?
    /// Bumped after every completed build action (build/lint/test/clean/fix) so
    /// views that cache per-target build status refresh their state and dates.
    var buildCompletionTick: Int = 0
    /// Bumped when a component's contract may have moved — after an edit run, **kept or refused** (a
    /// refusal still rewrites the gate), and after a remove. The navigator keys its per-row dots on this
    /// so they describe the state after the run rather than before it.
    var componentStatusTick: Int = 0
    var currentMode: SpireMode = .project
    var showChat: Bool = false
    /// The directory of the currently opened project.
    var projectRoot: String?

    // MCP servers state
    var mcpServers: [McpServerInfo] = []
    var mcpServersLoading: Bool = false
    var mcpServersError: String?
    /// All tools from all backends (via tools/list)
    var allTools: [McpToolInfo] = []
    /// True while fetching all tools
    var allToolsLoading: Bool = false

    // LLM settings state
    var llmConfig: LLMConfig = LLMConfig()
    var llmConfigLoading: Bool = false

    // Recent projects state
    var recentProjects: [RecentProject] = []

    /// The latest scaffold spec returned by `createProject/Scaffold` — exposed
    /// so the wizard's structure preview + fill step can read it.
    var scaffoldSpec: ScaffoldSpec?

    private let backend: any UIBackend

    /// Create a BuildService backed by this bridge's UIBackend. The service owns
    /// all build/lint/fix calls + the single live-event consumer (actor-isolated).
    ///
    /// This makes a **private** service: its own drain loop, so it sees events exclusively (the
    /// creation sheet's pipeline does this and stops it before returning). Anything on the
    /// dashboard uses `sharedBuildService` instead.
    func makeBuildService() -> BuildService {
        BuildService(backend: backend)
    }

    /// The dashboard's one build/activity event consumer.
    ///
    /// The Rust side wakes exactly **one** waiter per event (`Notify::notify_one`), so two
    /// services with their own drain loops do not each get a copy — the second steals lines from
    /// the first. The right pane's build log and its activity log therefore share this instance
    /// and subscribe to it for themselves.
    var sharedBuildService: BuildService {
        if let shared = sharedBuildServiceStorage { return shared }
        let made = BuildService(backend: backend)
        sharedBuildServiceStorage = made
        return made
    }

    @ObservationIgnored private var sharedBuildServiceStorage: BuildService?

    /// Create a ProjectService backed by this bridge's UIBackend. The service
    /// owns all project analyze/open/readFile FFI calls (repository pattern).
    func makeProjectService() -> ProjectService {
        ProjectService(backend: backend)
    }

    /// Fetch target-scoped detail (deps/platform/files) for a build target
    /// from the knowledge graph via `project/getBuildTarget`.
    func fetchBuildTarget(name: String) async throws -> BuildTargetDetail {
        try await makeProjectService().fetchBuildTarget(name: name)
    }

    var showSettings: Bool = false

    private var menuObservers: [NSObjectProtocol] = []

    init(backend: any UIBackend = SpireFFIBackend()) {
        self.backend = backend

        let nc = NotificationCenter.default
        self.menuObservers = [
            nc.addObserver(forName: MenuCommand.refreshProject, object: nil, queue: .main) { [weak self] _ in
                Task { await self?.fetchProjectAnalysis() }
            },
            nc.addObserver(forName: MenuCommand.toggleChat, object: nil, queue: .main) { [weak self] _ in
                self?.showChat.toggle()
            },
            nc.addObserver(forName: MenuCommand.showSettings, object: nil, queue: .main) { [weak self] _ in
                self?.showSettings = true
            },
            nc.addObserver(forName: MenuCommand.newProject, object: nil, queue: .main) { [weak self] _ in
                // No wizard to open: the welcome screen *is* the new-project flow now — project
                // type, name, location — so "New project" closes what is open and shows it.
                self?.closeProject()
            },
        ]
    }

    // MARK: - Connectivity check

    /// True when the Rust core (dylib) is reachable. Used to surface a
    /// clear, actionable error at startup instead of a generic failure
    /// when the core hasn't been built.
    private(set) var coreAvailable: Bool = false

    /// Verify the Rust core is reachable. Called on startup; if the core
    /// cannot be contacted the bridge transitions to `.error` with an
    /// actionable message so the user knows to build the Rust side.
    func checkConnection() async {
        guard backend.isAvailable else {
            state = .error(
                "Rust core not available. Please build it with `make rust` " +
                "or `cargo build --release -p spire-code`, then relaunch the app."
            )
            return
        }

        do {
            let body: [String: Any] = ["method": "status", "params": [:]]
            let data = try JSONSerialization.data(withJSONObject: body)
            _ = try await backend.send(data)
            coreAvailable = true
        } catch {
            state = .error(
                "Could not reach the Rust core: \(error.localizedDescription). " +
                "Please ensure the core is built and relaunch."
            )
        }
    }

    // MARK: - Commands

    /// Fetch the full project analysis from the Rust core.
    ///
    /// Always passes an explicit `root` to the core so re-analysis is a fresh
    /// disk scan (never a stale cached analysis). Falls back to the last known
    /// project root when none is supplied.
    func fetchProjectAnalysis(projectRoot: String? = nil) async {
        state = .opening

        // Authoritative root: explicit, else last known opened/analyzed root.
        let root = projectRoot ?? self.projectRoot

        do {
            let body: [String: Any] = [
                "method": "AnalyzeProject",
                "params": rootParams(root)
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let decoded: ProjectInfo = try MessageSerializer.decode(reply)
            // DIAGNOSTIC: log exactly what the Swift decode produced so the
            // subproject pipeline can be localized (decode drop vs render).
            // This is the REFRESH / menu path — the dashboard is loaded here.
            NSLog("[Bridge] fetchProjectAnalysis root=%@ subprojects=%d", decoded.root, decoded.subprojects.count)
            for s in decoded.subprojects {
                NSLog("[Bridge]   sub=%@ path=%@ kind=%@ structure=%@ files=%d",
                      s.name, s.path, s.kind.rawValue, s.structure, s.files?.count ?? -1)
            }
            let resolvedRoot = decoded.root
            await MainActor.run {
                self.projectRoot = resolvedRoot
                // MUST be executed on the main actor — @Observable does not
                // reliably invalidate views for off-main writes (this left the
                // app stuck on "Opening project…" even though the core returned
                // in <100ms).
                self.state = .idle(decoded)
            }
        } catch {
            let msg = "Error: \(error.localizedDescription)"
            Logger(subsystem: "spire", category: "startup").error("fetchProjectAnalysis failed: \(String(describing: error))")
            await MainActor.run { self.state = .error(msg) }
        }
    }

    /// Re-run the analyzer **without** the `.opening` transition, for a live refresh.
    ///
    /// The file watcher reports a component directory appearing or disappearing, but the row
    /// list is the analyzer's — the in-place tree patch cannot add or drop a row — so only
    /// another scan can make the navigator show the change. It must not flash "Opening
    /// project…" over a project already on screen, and it must not disturb the selection:
    /// identity is the path, so a selected component keeps its selection when it still exists
    /// and is cleared only when the row it pointed at is gone.
    ///
    /// Quiet on failure too: a background scan that fails must not replace a working project
    /// view with an error pane. The next file event (or a manual refresh) tries again.
    func reanalyzeQuietly(root: String) async {
        // Only refresh a project that is already on screen. `.opening` means a scan is
        // already in flight (with its own spinner) and `.error`/`.unconnected` have nothing
        // to refresh; in a non-project mode this is not the surface being described.
        guard case .idle = state, currentMode == .project, !root.isEmpty else { return }

        do {
            let body: [String: Any] = [
                "method": "AnalyzeProject",
                "params": rootParams(root)
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let decoded: ProjectInfo = try MessageSerializer.decode(reply)
            await MainActor.run {
                // A scan that raced a project switch is not this project's answer.
                guard self.projectRoot == decoded.root else { return }
                let previousSubproject = self.selectedSubproject
                let previousTarget = self.selectedBuildTarget
                self.state = .idle(decoded)
                if let previousSubproject {
                    // Re-resolve against the fresh values: the right pane must describe the
                    // component as it is now, not as it was when it was selected.
                    if let fresh = decoded.subprojects.first(where: { $0.id == previousSubproject.id }) {
                        self.selectedSubproject = fresh
                        if let previousTarget,
                           fresh.buildTargets.contains(where: { $0.name == previousTarget }) {
                            self.selectedBuildTarget = previousTarget
                        } else {
                            self.selectedBuildTarget = nil
                        }
                    } else {
                        // The component that was selected is gone; its detail would describe
                        // a directory that no longer exists.
                        self.selectedSubproject = nil
                        self.selectedBuildTarget = nil
                    }
                }
                // The row list just changed shape (and a filled/fresh dot may have moved with
                // it); the navigator keys its status read on the tick, so bump it.
                self.componentStatusTick &+= 1
            }
        } catch {
            Logger(subsystem: "spire", category: "startup")
                .error("reanalyzeQuietly failed: \(String(describing: error))")
        }
    }

    /// Open a project directory (creating it if needed). Initializes the
    /// graph database for the project, runs analysis, and returns ProjectInfo.
    func openProject(root: String) async {
        Logger(subsystem: "spire", category: "menu").info("openProject called with root=\(root)")
        state = .opening

        do {
            let body: [String: Any] = [
                "method": "project/open",
                "params": ["root": root]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            Logger(subsystem: "spire", category: "menu").info("openProject sending project/open for \(root)")
            let t0 = Date()
            let reply = try await backend.send(data)
            NSLog("[OpenTiming] backend.send returned after %.3fs replyBytes=%d", Date().timeIntervalSince(t0), reply.count)
            let t1 = Date()
            let decoded: ProjectInfo = try MessageSerializer.decode(reply)
            NSLog("[OpenTiming] MessageSerializer.decode returned after %.3fs subprojects=%d", Date().timeIntervalSince(t1), decoded.subprojects.count)
            Logger(subsystem: "spire", category: "menu").info("openProject succeeded: \(decoded.name) at \(decoded.root)")
            // DIAGNOSTIC: log exactly what the Swift decode produced so the
            // subproject pipeline can be localized (decode drop vs render).
            NSLog("[Bridge] openProject root=%@ subprojects=%d", decoded.root, decoded.subprojects.count)
            for s in decoded.subprojects {
                NSLog("[Bridge]   sub=%@ path=%@ kind=%@ structure=%@ files=%d",
                      s.name, s.path, s.kind.rawValue, s.structure, s.files?.count ?? -1)
            }
            await MainActor.run {
                self.projectRoot = decoded.root
                currentMode = .project
                // A freshly opened project starts with nothing selected. Without
                // this, a subproject selected in the PREVIOUS project stays
                // selected, and the right pane — which gates per-structure
                // actions on the selection — keeps rendering the old project's
                // actions for the new one. `closeProject` cleared these; opening
                // a second project in the same session did not.
                selectedSubproject = nil
                selectedBuildTarget = nil
                RecentProject.record(path: decoded.root, name: decoded.name)
                recentProjects = RecentProject.load()
                NSLog("[OpenTiming] .idle state set after %.3fs total", Date().timeIntervalSince(t0))
                // MUST be executed on the main actor — @Observable does not
                // reliably invalidate views for off-main writes (this left the
                // app stuck on "Opening project…" even though the core returned
                // in <100ms).
                state = .idle(decoded)
            }
        } catch {
            let msg = "Error: \(error.localizedDescription)"
            Logger(subsystem: "spire", category: "startup").error("openProject failed: \(String(describing: error))")
            await MainActor.run { state = .error(msg) }
        }
    }

    /// Load the list of recently opened projects from disk.
    func loadRecentProjects() {
        recentProjects = RecentProject.load()
    }

    /// Delete a recent project from the welcome list and refresh the UI.
    /// Removes the entry from ~/.spire/recent-projects.json; the project
    /// directory itself is left untouched.
    func removeRecentProject(path: String) {
        RecentProject.remove(path: path)
        recentProjects = RecentProject.load()
    }

    private func rootParams(_ root: String?) -> [String: Any] {
        if let root, !root.isEmpty {
            return ["root": root]
        }
        return [:]
    }

    /// Generate a step-by-step plan for creating a new project.
    /// Calls FFI: createProject/GeneratePlan { goal, rootDir, language }
    func generateCreationPlan(goal: String, rootDir: String, language: String = "Rust") async -> PlanGenerationResult? {
        await generateProjectPlan(goal: goal, rootDir: rootDir, language: language)
    }

    /// Generate a plan for a new project from the step-wise wizard's choices.
    /// `platforms` is the selected cross-compilation target registry-id list
    /// (e.g. `["rpi5","rock3c"]`; empty → host/native). `structure` is
    /// `"native" | "single_source" | "hal"`; `embedded` is true for
    /// cross-compile projects (no host target). Calls FFI:
    /// createProject/GeneratePlan { goal, rootDir, language, platforms }.
    /// - Parameter embeddedRoot: the container an `EmbeddedApp` depends on. The core requires it for
    ///   that structure (the app's path dependency is read from it) and omits it from every other
    ///   request, so nothing else changes.
    func generateProjectPlan(
        goal: String,
        rootDir: String,
        language: String = "Rust",
        platforms: [String] = [],
        structure: String = "native",
        embedded: Bool = false,
        embeddedRoot: String? = nil
    ) async -> PlanGenerationResult? {
        do {
            var params: [String: Any] = [
                "goal": goal,
                "rootDir": rootDir,
                "language": language,
                "platforms": platforms,
                "structure": structure,
                "embedded": embedded
            ]
            if let embeddedRoot, !embeddedRoot.trimmingCharacters(in: .whitespaces).isEmpty {
                params["embeddedRoot"] = embeddedRoot
            }
            let body: [String: Any] = [
                "method": "createProject/GeneratePlan",
                "params": params
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try MessageSerializer.decode(reply) as PlanGenerationResult
        } catch {
            return nil
        }
    }

    /// Execute an entire creation plan. Calls FFI: createProject/ExecutePlan
    func executeCreationPlan(rootDir: String, steps: [CreationStep]) async -> [StepExecutionResult] {
        do {
            let stepsJson = steps.map { step -> [String: Any] in
                var dict: [String: Any] = [
                    "id": step.id,
                    "stepType": step.stepType.rawValue,
                    "description": step.description,
                    "status": step.status.rawValue,
                ]
                if let params = step.parameters {
                    dict["parameters"] = params.value
                }
                if let result = step.result {
                    dict["result"] = result
                }
                return dict
            }
            let body: [String: Any] = [
                "method": "createProject/ExecutePlan",
                "params": [
                    "rootDir": rootDir,
                    "steps": stepsJson
                ]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return (try MessageSerializer.decode(reply) as [StepExecutionResult])
        } catch {
            // silent: print("[executeCreationPlan] error: \(error)")
            return []
        }
    }

    /// Execute a single plan step and return its result.
    func executeStep(_ step: CreationStep, rootDir: String) async -> StepExecutionResult? {
        do {
            var stepDict: [String: Any] = [
                "id": step.id,
                "stepType": step.stepType.rawValue,
                "description": step.description,
                "status": step.status.rawValue,
            ]
            if let params = step.parameters { stepDict["parameters"] = params.value }
            if let result = step.result { stepDict["result"] = result }
            let body: [String: Any] = [
                "method": "createProject/ExecuteStep",
                "params": ["rootDir": rootDir, "step": stepDict]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try MessageSerializer.decode(reply) as StepExecutionResult
        } catch {
            // silent: print("[executeStep] error: \(error)")
            return nil
        }
    }

    /// Send a chat message and get a response.
    /// When a subproject is selected in the project graph, the chat message
    /// is annotated with the modification scope so the LLM knows it may only
    /// modify files within the selected subproject (Level 2 scope).
    /// Create a modification plan via the task-focused plan/create RPC.
    /// - `goal`: description of the modification to plan
    /// - `scopePath`: absolute path when subproject-scoped; nil for project scope
    func createPlan(goal: String, scopePath: String?, language: String? = nil, buildSystem: String? = nil) async -> PlanStatusResult? {
        lastPlanError = nil
        do {
            // The Rust plan/create handler only forwards `goal` to the LLM (it
            // drops extra params). To guarantee the LLM knows the project's
            // stack, embed the language + build system directly in the goal.
            var effectiveGoal = goal
            var context: [String] = []
            if let language, !language.isEmpty {
                // "🦀 Rust" / "🐍 Python" style labels — strip emoji prefixes.
                let clean = language
                    .unicodeScalars
                    .filter { !$0.properties.isEmoji && $0.properties.isEmojiPresentation == false }
                    .map { String($0) }
                    .joined()
                    .trimmingCharacters(in: .whitespaces)
                if !clean.isEmpty { context.append("Language: \(clean)") }
            }
            if let buildSystem, !buildSystem.isEmpty {
                context.append("Build system: \(buildSystem)")
            }
            if !context.isEmpty {
                effectiveGoal = "[Project context: \(context.joined(separator: ", "))]\n\(goal)"
            }

            var params: [String: Any] = ["goal": effectiveGoal]
            if let path = scopePath {
                params["scope"] = "subproject"
                params["scope_path"] = path
            } else {
                params["scope"] = "project"
            }
            // Give the LLM explicit project context so planning uses the right
            // language and build-tool conventions (e.g. Python + pyproject.toml).
            if let language, !language.isEmpty { params["language"] = language }
            if let buildSystem, !buildSystem.isEmpty { params["build_system"] = buildSystem }
            let body: [String: Any] = ["method": "plan/create", "params": params]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            // Surface the backend's key/value `error` field when present
            if let json = try JSONSerialization.jsonObject(with: reply) as? [String: Any],
               let err = json["error"] as? String {
                lastPlanError = err
                return nil
            }
            return try MessageSerializer.decode(reply) as PlanStatusResult
        } catch {
            lastPlanError = error.localizedDescription
            return nil
        }
    }

    /// Approve a generated plan — the Rust core executes its steps sequentially.
    /// Returns true on success, false on failure (details in `lastPlanError`).
    func approvePlan(planId: String) async -> Bool {
        lastPlanError = nil
        do {
            let body: [String: Any] = [
                "method": "plan/approve",
                "params": ["plan_id": planId]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            if let json = try JSONSerialization.jsonObject(with: reply) as? [String: Any] {
                if let err = json["error"] as? String {
                    lastPlanError = err
                    return false
                }
                return json["ok"] as? Bool ?? false
            }
            return false
        } catch {
            lastPlanError = error.localizedDescription
            return false
        }
    }


    func sendChatMessage(_ text: String) async {
        guard !text.trimmingCharacters(in: .whitespaces).isEmpty else { return }

        let userMsg = ChatMessage(
            id: UUID().uuidString,
            role: .user,
            content: text,
            timestamp: Date()
        )
        messages.append(userMsg)
        isProcessing = true

        // Scope annotation: when a subproject is selected in the project graph,
        // constrain the LLM to modification of that subproject only.
        var effectiveText = text
        if let sub = selectedSubproject {
            effectiveText = "[Scope: subproject \(sub.name) at \(sub.path)]\n\(text)"
        }

        do {
            let command = try MessageSerializer.encode(chat: effectiveText)
            let reply = try await backend.send(command)
            let response = try MessageSerializer.decodeChat(reply)
            messages.append(response)
        } catch {
            let errorMsg = ChatMessage(
                id: UUID().uuidString,
                role: .system,
                content: "Error: \(error.localizedDescription)",
                timestamp: Date()
            )
            messages.append(errorMsg)
        }
        isProcessing = false
    }

    func selectSubproject(_ sub: SubprojectInfo?) {
        selectedSubproject = sub
    }

    /// Close the currently open project and return to the welcome dialog.
    /// Because `SpireBridge.shared` is a singleton, this MUST reset the state
    /// so a fresh "New Window" (Cmd-N) shows WelcomeView instead of the stale
    /// project.
    func closeProject() {
        state = .unconnected
        currentMode = .project
        projectRoot = nil
        selectedSubproject = nil
        // Clear the shared selection mirror too — a stale target would
        // otherwise leak into the next opened project's right pane.
        selectedBuildTarget = nil
        messages = []
        activePlan = nil
        planVisible = false
        buildEvents = []
        buildRunning = false
    }

    // MARK: - Push Event Stream

    /// A single pushed file-change event from the Rust core.
    struct FileChangeEvent: Decodable {
        let kind: String
        var path: String
        /// True when the changed path is a directory; nil when unknown.
        /// Rust watcher events don't include this — `ProjectInfo.apply`
        /// fills it in from the real filesystem so extension-less files
        /// (Makefile, LICENSE, Dockerfile, …) are never misread as folders.
        var isDirectory: Bool?
    }

    /// A streaming build line pushed from the Rust build module.
    struct BuildEventLine: Decodable, Identifiable, Hashable, Sendable {
        let id: UUID
        let line: String
        let level: String
        let target: String?
        /// File path parsed from "  --> path:line:col" (warning/error blocks).
        let file: String?
        /// Line number parsed from the location line.
        let lineNumber: Int?
        /// The message text for a block-level warning/error.
        let message: String?
        /// Full raw block text (may contain code context for warning/error).
        let detail: String?
        /// Which part of the app produced the line, when it says: `llm`, `build`, `test`.
        ///
        /// Build lines leave this nil (they are the build); a line from the coordinator's activity
        /// log names its source so the pane can put an icon on it and a person can tell a model's
        /// answer from a compiler's.
        let source: String?
        /// The step within that source: `plan`, `generate`, `compile`, `test`, `verify`.
        let phase: String?

        init(id: UUID = UUID(), line: String, level: String, target: String? = nil,
             file: String? = nil, lineNumber: Int? = nil, message: String? = nil,
             detail: String? = nil, source: String? = nil, phase: String? = nil) {
            self.id = id
            self.line = line
            self.level = level
            self.target = target
            self.file = file
            self.lineNumber = lineNumber
            self.message = message
            self.detail = detail
            self.source = source
            self.phase = phase
        }
    }

    /// Live lines from the last build, populated when a build tool completes.
    var buildEvents: [BuildEventLine] = []

    /// Aggregate the current build/lint events into per-file diagnostic badge
    /// counts: [filePath: [severity:count]]. Used by the file tree to show
    /// warning/error badges next to file names.
    var diagnosticBadges: [String: [String: Int]] {
        var result: [String: [String: Int]] = [:]
        for ev in buildEvents {
            guard let file = ev.file else { continue }
            let severity = ev.level == "error" ? "error" : "warning"
            result[file, default: [:]][severity, default: 0] += 1
        }
        return result
    }

    /// True while a hardcoded build tool is running.
    var buildRunning: Bool = false

    /// Parse {"line":...,"level":...,"target":...} JSON objects into BuildEventLines.
    private func parseBuildEvents(_ value: Any?) -> [BuildEventLine] {
        guard let arr = value as? [[String: Any]] else { return [] }
        return arr.compactMap { dict in
            guard let line = dict["line"] as? String else { return nil }
            let level = dict["level"] as? String ?? "info"
            let target = dict["target"] as? String
            let file = dict["file"] as? String
            let lineNumber = dict["line_number"] as? Int
            let message = dict["message"] as? String
            let detail = dict["detail"] as? String
            return BuildEventLine(
                line: line, level: level, target: target,
                file: file, lineNumber: lineNumber, message: message, detail: detail
            )
        }
    }

    /// Push-only stream of file-change events. The FFI call blocks until the
    /// next event arrives (no polling), so the Rust actor drives the UI.
    func eventStream(timeoutMs: UInt32 = 10000) -> AsyncStream<FileChangeEvent> {
        AsyncStream { continuation in
            Task.detached {
                while !Task.isCancelled {
                    guard let json = self.backend.waitForEvent(timeoutMs: timeoutMs) else { continue }
                    guard let data = json.data(using: .utf8),
                          let event = try? JSONDecoder().decode(FileChangeEvent.self, from: data)
                    else { continue }
                    continuation.yield(event)
                }
                continuation.finish()
            }
        }
    }

    // MARK: - MCP Server Commands

    /// Read the last persisted action status (success/duration/timestamp) for a
    /// subproject path from the knowledge graph config.
    ///
    /// `kind` selects the action to read — `"build"` (default), `"lint"`,
    /// `"test"` or `"clean"` — mirroring the `<kind>.last.<path>[.<target>]`
    /// keys the backend writes when an action finishes.
    func fetchBuildStatus(path: String, target: String? = nil, kind: String = "build") async -> BuildStatus? {
        do {
            var params: [String: Any] = ["path": path, "kind": kind]
            if let target, !target.isEmpty {
                params["target"] = target
            }
            let body: [String: Any] = [
                "method": "project/buildStatus",
                "params": params
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let dict = try JSONSerialization.jsonObject(with: reply) as? [String: Any]
            guard let success = dict?["success"] as? Bool else { return nil }
            // The graph config stores {path, success, duration_secs, timestamp}
            // (RFC3339 from store_build_status). Rust's to_rfc3339() includes
            // nanosecond fractional seconds, e.g. 2026-08-09T08:03:25.123456789Z,
            // which the default ISO8601DateFormatter cannot parse — enable
            // fractional-seconds support with a plain fallback.
            let isoFraction = ISO8601DateFormatter()
            isoFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            let isoPlain = ISO8601DateFormatter()
            let lastBuild = (dict?["timestamp"] as? String).flatMap { ts in
                isoFraction.date(from: ts) ?? isoPlain.date(from: ts)
            }
            let duration = dict?["duration_secs"] as? Double
            return BuildStatus(
                lastBuild: lastBuild,
                success: success,
                output: dict?["output"] as? String,
                errors: [],
                durationSecs: duration
            )
        } catch {
            return nil
        }
    }

    // MARK: - Git working-tree state

    /// Summary of uncommitted changes in a working tree.
    struct GitSummary: Sendable {
        /// Total `git status --short` entries (modified + staged + deleted + untracked).
        let changedCount: Int
        /// Untracked (`??`) entries.
        let untrackedCount: Int
        /// The raw porcelain lines (trimmed) for a details list.
        let lines: [String]
        var isDirty: Bool { changedCount > 0 }
    }

    /// Parse `git status --short` output into counts + raw lines.
    ///
    /// Porcelain short format is `<XY> <path>`; untracked entries start `??`.
    /// Kept as a pure function so it is unit-testable without a backend.
    static func parseGitStatus(_ output: String) -> GitSummary {
        let lines = output
            .split(separator: "\n")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        let untracked = lines.filter { $0.hasPrefix("??") }.count
        return GitSummary(changedCount: lines.count, untrackedCount: untracked, lines: lines)
    }

    /// `git status --short` for a working tree (nil when not a repo / no git).
    /// This is the safety net that makes a destructive action visible.
    func gitStatus(path: String) async -> GitSummary? {
        guard let json = await callBuildTool("git_status", args: ["path": path]),
              let output = json["output"] as? String else { return nil }
        return Self.parseGitStatus(output)
    }

    /// Full `git diff` text for a working tree (nil when unavailable).
    func gitDiff(path: String) async -> String? {
        guard let json = await callBuildTool("git_diff", args: ["path": path]),
              json["error"] == nil else { return nil }
        return json["output"] as? String
    }

    /// Stage every working-tree change (`git add -A`).
    func gitStage(path: String) async -> Bool {
        guard let json = await callBuildTool("git_stage", args: ["path": path]) else { return false }
        return json["error"] == nil
    }

    /// Commit the staged changes with `message`. Returns git's output on success.
    func gitCommit(path: String, message: String) async -> String? {
        guard let json = await callBuildTool("git_commit", args: ["path": path, "message": message]),
              json["error"] == nil else { return nil }
        return json["output"] as? String
    }

    // MARK: - RAG

    /// Fetch all RAG domain summaries (clean id, chunk/source counts, corpus
    /// version) via `rag/list-domains`.
    func fetchRagDomains() async -> [RagDomainInfo] {
        do {
            let body: [String: Any] = ["method": "rag/list-domains", "params": [:]]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try MessageSerializer.decode(reply)
        } catch {
            return []
        }
    }

    /// Install the bundled application-wisdom manifests (the `spire-idf`
    /// docs) into the KnowledgeStore scan dirs so RagView lists them and their
    /// Ingest buttons build the corpus.
    func installSpireDocsManifests() async -> Bool {
        let body: [String: Any] = ["method": "rag/install-bundle-manifests", "params": [:]]
        do {
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            if let json = try? JSONSerialization.jsonObject(with: reply) as? [String: Any],
               json["error"] is String {
                return false
            }
            return true
        } catch {
            return false
        }
    }

    /// Discover the available ingestion manifests (`ingest.yaml`, one per
    /// corpus directory) via `rag/list-manifests`. RAG is project-independent.
    func fetchRagManifests() async -> [RagManifestInfo] {
        do {
            let body: [String: Any] = [
                "method": "rag/list-manifests",
                "params": [:]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try MessageSerializer.decode(reply)
        } catch {
            return []
        }
    }

    /// Fetch persisted per-source ingest status for a domain via
    /// `rag/list-sources` (no ingest required).
    func fetchRagSources(domain: String) async -> [RagSourceStatus] {
        do {
            let body: [String: Any] = [
                "method": "rag/list-sources",
                "params": ["domain": domain]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            if let json = try? JSONSerialization.jsonObject(with: reply) as? [String: Any],
               json["error"] is String {
                return []
            }
            return try MessageSerializer.decode(reply) as [RagSourceStatus]
        } catch {
            return []
        }
    }

    /// Set the default RAG domain used by `rag/search` when `domain` is omitted
    /// (kept in sync with the corpus selected in the UI).
    func setRagDomain(domain: String) async {
        let body: [String: Any] = [
            "method": "rag/set-domain",
            "params": ["domain": domain]
        ]
        if let data = try? JSONSerialization.data(withJSONObject: body),
           let _ = try? await backend.send(data) {
            // ok
        }
    }

    /// Ingest one canonical `ingest.yaml` via `rag/ingest-graph-config`.
    /// Returns the number of chunks stored (0 when the backend reports an error).
    func ingestRagManifest(path: String) async -> RagIngestReport? {
        do {
            let body: [String: Any] = [
                "method": "rag/ingest-graph-config",
                "params": [
                    "manifest_path": path
                ]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            if let json = try? JSONSerialization.jsonObject(with: reply) as? [String: Any],
               json["error"] is String {
                return nil
            }
            return try MessageSerializer.decode(reply) as RagIngestReport
        } catch {
            return nil
        }
    }

    /// Re-ingest one canonical `ingest.yaml` via `rag/reingest-graph-config`:
    /// the backend clears the manifest's domain corpus first, then ingests from
    /// scratch, so changed content replaces stale chunks.
    func reingestRagManifest(path: String) async -> RagIngestReport? {
        do {
            let body: [String: Any] = [
                "method": "rag/reingest-graph-config",
                "params": [
                    "manifest_path": path
                ]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            if let json = try? JSONSerialization.jsonObject(with: reply) as? [String: Any],
               json["error"] is String {
                return nil
            }
            return try MessageSerializer.decode(reply) as RagIngestReport
        } catch {
            return nil
        }
    }

    /// Semantic search within a RAG domain via `rag/search`.
    func ragSearch(domain: String, query: String, topK: Int = 5) async -> [RagChunkResult] {
        do {
            let body: [String: Any] = [
                "method": "rag/search",
                "params": [
                    "domain": domain,
                    "query": query,
                    "top_k": topK
                ]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            if let json = try? JSONSerialization.jsonObject(with: reply) as? [String: Any],
               json["error"] is String {
                return []
            }
            return (try? MessageSerializer.decode(reply) as [RagChunkResult]) ?? []
        } catch {
            return []
        }
    }

    /// Fetch diagnostics (errors/warnings/lint findings) for a project or
    /// subproject from the knowledge graph. `path` is the absolute directory
    /// (empty string = whole project).
    func fetchDiagnostics(path: String = "") async -> [DiagnosticEntry] {
        do {
            let body: [String: Any] = [
                "method": "project/diagnostics",
                "params": ["path": path]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try MessageSerializer.decode(reply)
        } catch {
            return []
        }
    }

    /// Fetch the list of MCP servers from the Rust core.
    func fetchMcpServers() async {
        mcpServersLoading = true
        mcpServersError = nil
        defer { mcpServersLoading = false }

        do {
            let command = try MessageSerializer.encode(command: "mcp/servers")
            let reply = try await backend.send(command)
            let servers: [McpServerInfo] = try MessageSerializer.decode(reply)
            mcpServers = servers
        } catch {
            mcpServersError = error.localizedDescription
        }
    }

    // MARK: - Device (on-hardware) commands

    /// The MCP server name a platform's board is registered under.
    static func deviceServerName(for platform: String) -> String { "device-\(platform)" }

    /// Whether the board for `platform` is connected right now.
    ///
    /// Boards are registered `autostart: false` and reached deliberately, so
    /// "offline" is the normal resting state — the Device actions key off this.
    func deviceOnline(platform: String) -> Bool {
        let name = Self.deviceServerName(for: platform)
        guard let server = mcpServers.first(where: { $0.name == name }) else { return false }
        return (server.properties?["status"]?.value as? String) == "online"
    }

    /// Connect the board for `platform`. Bounded on the Rust side (~15s), so an
    /// unreachable board returns false rather than hanging.
    func connectDevice(platform: String) async -> Bool {
        let reply = await deviceCommand("device/connect", params: ["platform": platform])
        guard let reply, reply["error"] == nil else { return false }
        // Refresh so `deviceOnline` reflects the new state immediately.
        await fetchMcpServers()
        return deviceOnline(platform: platform)
    }

    /// Trap control (M5): the processes this board is running, and what to do with them.
    ///
    /// All four pass through to the board's own tools, because the board holds the process table —
    /// mirroring it here would give two answers to "what is running" with the stale one on screen.
    /// A tool-level failure arrives as `error` with the board's words (which name the pid, the signal
    /// and the log path), so these return the reply object as-is rather than reshaping it.
    func deviceProcesses(platform: String) async -> [String: Any]? {
        await deviceCommand("device/procs", params: ["platform": platform])
    }

    /// Read a started binary's log. `lines` bounds the tail (the board caps it at 5000).
    func deviceLogs(platform: String, name: String, lines: Int = 100) async -> [String: Any]? {
        await deviceCommand(
            "device/logs",
            params: ["platform": platform, "name": name, "lines": lines]
        )
    }

    /// Stop a started binary by name: SIGTERM, a 2s grace, then SIGKILL. Stopping something that
    /// already exited is not an error — it is the state the caller wanted.
    func deviceStop(platform: String, name: String) async -> [String: Any]? {
        await deviceCommand("device/stop", params: ["platform": platform, "name": name])
    }

    /// Start a binary on the board in the background, under a name, logging its output.
    ///
    /// `path` is relative to the project root (an absolute path works too). Refused when that name is
    /// already running, so `stop` always means what this call started.
    func deviceStart(platform: String, name: String, path: String, args: [String] = []) async -> [String: Any]? {
        var params: [String: Any] = ["platform": platform, "name": name, "path": path]
        if !args.isEmpty { params["args"] = args }
        return await deviceCommand("device/start", params: params)
    }

    /// Deploy a cross-built test binary to the board and run it there.
    ///
    /// `path` is relative to the project root (an absolute path works too). With `build` the call
    /// **cross-builds first** — through the build tools, so the artifact is the one this platform's
    /// module writes for this triple — and then uploads and runs it: one action instead of "build it
    /// somewhere, then hand me the path". Returns the board's reply, which carries `passed` and the
    /// full result; a failed build returns an error naming the build, not the board.
    func runDeviceTests(
        platform: String,
        path: String,
        timeoutSecs: Int = 120,
        build: Bool = false
    ) async -> [String: Any]? {
        var params: [String: Any] = [
            "platform": platform, "path": path, "timeout_secs": timeoutSecs,
        ]
        if build { params["build"] = true }
        return await deviceCommand("device/test", params: params)
    }

    /// Install a production binary on the board.
    ///
    /// `dest` overrides the platform's `device.deploy.dest` from the registry.
    func deployDeviceBinary(platform: String, path: String, dest: String? = nil) async -> [String: Any]? {
        var params: [String: Any] = ["platform": platform, "path": path]
        if let dest, !dest.trimmingCharacters(in: .whitespaces).isEmpty {
            params["dest"] = dest
        }
        return await deviceCommand("device/deploy", params: params)
    }

    /// Send a `device/*` command and return its reply object (nil when the
    /// backend could not be reached).
    private func deviceCommand(_ method: String, params: [String: Any]) async -> [String: Any]? {
        do {
            let body: [String: Any] = ["method": method, "params": params]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try JSONSerialization.jsonObject(with: reply) as? [String: Any]
        } catch {
            return nil
        }
    }

    /// Fetch all cross-compilation platforms from the Rust core (graph-backed).
    func fetchPlatforms() async -> [Platform] {
        do {
            let body: [String: Any] = ["method": "platforms/list", "params": [:]]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try MessageSerializer.decode(reply)
        } catch {
            return []
        }
    }

    /// Fetch every board and chip **as the graph holds it** — the typed entry plus its capability
    /// blocks (`realizes` / `provides` / `carries` / `pins`), read from the graph's edges.
    ///
    /// Distinct from `fetchPlatforms` (`platforms/list`), which sends only the build facts. This is
    /// the configuration screen's source: the graph is canonical, and the registry YAML is only the
    /// seed beside it.
    func fetchPlatformConfigs() async -> [Platform] {
        do {
            let body: [String: Any] = ["method": "platforms/config", "params": [:]]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let decoded: PlatformConfigReply = try MessageSerializer.decode(reply)
            return decoded.platforms
        } catch {
            return []
        }
    }

    /// Fetch one board's **resolved profile** — the capabilities it realizes, the companion silicon
    /// it carries, its wiring, and the parts it declares (`devices:`) — read back from the graph.
    ///
    /// This is the read the experimental board/resource flow seeds a resource list from: the board's
    /// own parts come back already named and roled, so nothing is re-derived in Swift.
    func fetchBoardProfile(board: String) async -> BoardProfile? {
        do {
            let body: [String: Any] = [
                "method": "capabilities/profile",
                "params": ["board": board],
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let decoded: BoardProfile = try MessageSerializer.decode(reply)
            return decoded
        } catch {
            return nil
        }
    }

    /// Fetch the **capability vocabulary** — every capability path a `role` may name, grouped by
    /// category.
    ///
    /// The catalogue, not a board's facts: it is what validates a device's `roles:` and an
    /// application's `resources[].roles`, so a picker fed from it can never offer a name the core
    /// would refuse.
    func fetchVocabulary() async -> [VocabularyCategory] {
        do {
            let body: [String: Any] = ["method": "capabilities/vocabulary", "params": [:]]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let decoded: VocabularyReply = try MessageSerializer.decode(reply)
            return decoded.categories
        } catch {
            return []
        }
    }

    /// Read a file via the in-process filesystem module (filesystem_read) and return its contents.
    func readFile(at path: String) async -> String? {
        do {
            let body: [String: Any] = [
                "method": "tools/call",
                "params": [
                    "tool": "filesystem_read",
                    "args": ["path": path]
                ]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)

            // Parse the result. The in-process filesystem module returns:
            //   Ok → {"Ok": "file contents..."}
            //   Err → {"Err": "Failed to read ..."}
            if let json = try JSONSerialization.jsonObject(with: reply) as? [String: Any] {
                // Error wrapper
                if json["Err"] is String {
                    return nil
                }
                // Success wrapper
                if let text = json["Ok"] as? String {
                    return text
                }
                // Coordinator error
                if json["error"] is String {
                    return nil
                }
            }
            // Some modules may return bare strings
            if let str = String(data: reply, encoding: .utf8) {
                return str
            }
            // silent: print("[readFile] could not parse response")
            return nil
        } catch {
            // silent: print("[readFile] error: \(error)")
            return nil
        }
    }

    /// Send a raw coordinator method (e.g. `build/fixPropose`) bypassing
    /// `tools/call`, and return the JSON object.
    private func callRawMethod(_ method: String, args: [String: Any]) async -> [String: Any]? {
        do {
            let body: [String: Any] = ["method": method, "params": args]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try JSONSerialization.jsonObject(with: reply) as? [String: Any]
        } catch {
            return nil
        }
    }

    private func callBuildTool(_ tool: String, args: [String: Any]) async -> [String: Any]? {
        do {
            let body: [String: Any] = [
                "method": "tools/call",
                "params": [
                    "tool": tool,
                    "args": args
                ]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return try JSONSerialization.jsonObject(with: reply) as? [String: Any]
        } catch {
            return nil
        }
    }

    /// **Add a component** to an ESP-IDF component library, of the stated **kind**: a `driver` (one
    /// device on one bus) arrives with its bus seam and a fake bus, a `library` (pure code) with a plain
    /// unit test and no bus at all. Its code is not written here — that is `idfComponentEdit`.
    ///
    /// `bus` is the driver's and is empty for a library, which has none: a bus passed where it means
    /// nothing would be a contract that lies.
    /// - Parameters:
    ///   - part: a driver's part number — the key its facts are looked up by. Omitted, the component's
    ///     own name is used, which is right when the two are the same word.
    ///   - address: the bus address, when the part has one.
    ///   - roles: the capability paths the component provides, from the shared vocabulary.
    ///   - description: the **fill prose** — what the component should become. It becomes the spec's
    ///     description, so every later edit starts from it rather than from an empty box.
    func idfAddComponent(
        root: String,
        name: String,
        kind: String,
        bus: String,
        part: String = "",
        address: String = "",
        roles: [String] = [],
        description: String = ""
    ) async -> (result: [String: Any]?, error: String?) {
        var args: [String: Any] = ["root": root, "name": name, "kind": kind, "bus": bus]
        if !part.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { args["part"] = part }
        if !address.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { args["address"] = address }
        if !roles.isEmpty { args["roles"] = roles }
        if !description.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            args["description"] = description
        }
        guard let json = await callBuildTool("idf_add_component", args: args) else {
            return (nil, "core unavailable")
        }
        if let err = json["error"] as? String {
            return (nil, err)
        }
        return (json, nil)
    }

    /// **Remove a component**: its directory. Refuses when another component `REQUIRES` it, naming
    /// what depends on it — a shared component is not deleted out from under its users.
    func idfRemoveComponent(root: String, name: String) async -> (result: [String: Any]?, error: String?) {
        guard let json = await callBuildTool(
            "idf_remove_component",
            args: ["root": root, "name": name]
        ) else {
            return (nil, "core unavailable")
        }
        // The row is gone whatever the tool says back: even a refusal can follow a directory already
        // emptied by hand, and the navigator should re-read rather than keep describing it.
        componentStatusTick &+= 1
        if let err = json["error"] as? String {
            return (nil, err)
        }
        return (json, nil)
    }

    /// **Write or change a component's protocol.** The model is given the library's `SPIRE.md`, the
    /// component's header, the bus seam and the invariants; the change is kept only if the component's
    /// host test still compiles and passes, and rolled back byte-for-byte otherwise.
    ///
    /// - Parameter platform: a chip to build for as a *second* gate. Omitted, the host test is the whole
    ///   check and the result says the chip build did not run.
    func idfComponentEdit(
        root: String,
        name: String,
        instruction: String,
        platform: String? = nil
    ) async -> (result: [String: Any]?, error: String?) {
        var args: [String: Any] = ["root": root, "name": name, "instruction": instruction]
        if let platform, !platform.isEmpty {
            args["platform"] = platform
        }
        guard let json = await callBuildTool("idf_component_edit", args: args) else {
            return (nil, "core unavailable")
        }
        // This tool answers in its own shape (`success`/`error`), not with an `error` key only — so the
        // failure path is a `success == false`, and the message is in `error` when there is one.
        //
        // The tick moves **before** the check, on purpose: a refused run is reverted byte-for-byte, but
        // its refusal is a new record and the newest gate the row shows — so the dots must re-read for a
        // failure too, and a refresh that only ran on success would show the run before the refused one.
        componentStatusTick &+= 1
        if let err = json["error"] as? String {
            return (nil, err)
        }
        return (json, nil)
    }

    /// **A component's recorded edits**, newest first — the `idf_component_edit` runs for one
    /// component of a library. Read-only: the same records the edit tool echoes in its response, but
    /// fetched on demand (and beyond the inline window) for a history pane.
    func componentEditHistory(
        root: String,
        name: String,
        limit: Int = 50
    ) async -> [ComponentEditRecord] {
        guard let json = await callRawMethod(
            "idf-component/history",
            args: ["root": root, "name": name, "limit": limit]
        ) else { return [] }
        if json["error"] is String { return [] }
        guard let history = json["history"] as? [[String: Any]],
              let data = try? JSONSerialization.data(withJSONObject: history) else { return [] }
        return (try? JSONDecoder().decode([ComponentEditRecord].self, from: data)) ?? []
    }

    /// **What a component is** — the spec the graph holds, or the least its tree states.
    ///
    /// The component pane's whole answer: the fill prose the edit box starts from, the kind, the
    /// device its facts are looked up by, the capabilities it provides, the corpus documents it was
    /// written from, and the record that says who decided it. Reading **writes nothing**: a component
    /// that predates the graph comes back `stored == false` with a spec implied from its own
    /// `CMakeLists.txt`, and the first edit is what decides it.
    func fetchComponentSpec(
        root: String,
        name: String
    ) async -> (reading: ComponentSpecReading?, error: String?) {
        guard let json = await callRawMethod(
            "project/componentSpec",
            args: ["root": root, "name": name]
        ) else {
            return (nil, "core unavailable")
        }
        if let error = json["error"] as? String { return (nil, error) }
        do {
            let data = try JSONSerialization.data(withJSONObject: json)
            return (try MessageSerializer.decode(data), nil)
        } catch {
            Self.logScaffold("project/componentSpec DECODE FAILED: \(error)")
            return (nil, "the component spec did not decode: \(error)")
        }
    }

    /// **What state each component of a library is in** — the navigator's two dots per row: how much
    /// of the contract exists, and what the last write's gate decided.
    ///
    /// One read for the whole list (`project/componentStatus`), not one call per row: the same
    /// question asked N times would cost N graph walks and answer them at N instants. The list is the
    /// tree's, so a component nothing has been written about yet still gets a row — and the read
    /// stores nothing, so drawing the list cannot decide a contract nobody asked for.
    ///
    /// **Empty on any failure.** The dots are an addition to a list that reads without them; a
    /// navigator that refused to draw because a status did not arrive would be the worse answer.
    func fetchComponentStatuses(root: String) async -> [ComponentStatus] {
        guard let json = await callRawMethod(
            "project/componentStatus",
            args: ["root": root]
        ) else { return [] }
        if json["error"] is String { return [] }
        guard let components = json["components"] as? [[String: Any]],
              let data = try? JSONSerialization.data(withJSONObject: components) else { return [] }
        guard let statuses = try? JSONDecoder().decode([ComponentStatus].self, from: data) else {
            Self.logScaffold("project/componentStatus DECODE FAILED")
            return []
        }
        return statuses
    }

    // MARK: - Knowledge graph (project/queryGraph, project/getRelationships)

    /// Query the knowledge graph directly (`project/queryGraph`): nodes by type / subtype / name, and —
    /// with `includeRelationships` — the edges between them. This is what `ProjectGraphView` renders in
    /// knowledge mode; the graph is the core's own record, not the in-memory project tree.
    func queryGraph(
        nodeType: String? = nil,
        subtype: String? = nil,
        name: String? = nil,
        limit: Int = 250,
        includeRelationships: Bool = true
    ) async -> KnowledgeGraphResult? {
        var args: [String: Any] = ["limit": limit, "includeRelationships": includeRelationships]
        if let nodeType { args["nodeType"] = nodeType }
        if let subtype { args["subtype"] = subtype }
        if let name { args["name"] = name }
        guard let json = await callProjectTool("project/queryGraph", args: args),
              let data = try? JSONSerialization.data(withJSONObject: json) else { return nil }
        return try? JSONDecoder().decode(KnowledgeGraphResult.self, from: data)
    }

    /// The relationships around a node (`project/getRelationships`) — from the project node when no id
    /// is given. Depth-limited and traversed both ways by default, so one call yields the edges the
    /// graph view draws between the nodes it already has.
    func graphRelationships(
        nodeId: String? = nil,
        depth: Int = 3,
        direction: String = "both"
    ) async -> GraphRelationshipsResult? {
        var args: [String: Any] = ["depth": depth, "direction": direction]
        if let nodeId { args["nodeId"] = nodeId }
        guard let json = await callProjectTool("project/getRelationships", args: args),
              let data = try? JSONSerialization.data(withJSONObject: json) else { return nil }
        return try? JSONDecoder().decode(GraphRelationshipsResult.self, from: data)
    }

    /// Call a project-query tool over `tools/call` and unwrap the reply. The coordinator returns the
    /// tool's payload directly; older paths wrap it in the tool-module envelope
    /// (`{"result":{"content":[{"type":"text","text":"{…}"}]}}`), so both shapes are accepted — the
    /// same handling `ProjectService.fetchBuildTarget` needs.
    private func callProjectTool(_ tool: String, args: [String: Any]) async -> [String: Any]? {
        guard let json = await callRawMethod(
            "tools/call",
            args: ["tool": tool, "args": args]
        ) else { return nil }
        if let err = json["error"] as? String, !err.isEmpty { return nil }
        if let result = json["result"] as? [String: Any],
           let content = result["content"] as? [[String: Any]],
           let first = content.first,
           let text = first["text"] as? String,
           let inner = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] {
            return inner
        }
        return json
    }

    /// Ask the backend to propose a WHOLE-FILE rewrite that fixes the current
    /// compiler diagnostics for one C/C++ file (`build/fixPropose`).
    ///
    /// Read-only: nothing is written until the user accepts the proposal.
    func proposeCompileFix(root: String, file: String) async -> FixProposeResult? {
        guard let json = await callRawMethod("build/fixPropose", args: ["root": root, "file": file]) else { return nil }
        guard let data = try? JSONSerialization.data(withJSONObject: json),
              let result = try? JSONDecoder().decode(FixProposeResult.self, from: data) else { return nil }
        return result
    }

    /// **The design brief**, over `createProject/BriefApplication`: a description in, the brief the
    /// board and actor steps are built from out — long before a board is chosen.
    ///
    /// One parameter, deliberately: the brief is about what the application *has to do*, and naming a
    /// board here would move a later step's decision onto the first screen. The refusal is returned
    /// as-is (the core names what was wrong), and a decoded brief carries only capability paths the
    /// core already checked against the vocabulary — so the badges it drives cannot point at a
    /// misspelling.
    func briefApplication(
        description: String
    ) async -> (brief: DesignBrief?, error: String?) {
        guard
            let json = await callRawMethod(
                "createProject/BriefApplication", args: ["description": description])
        else {
            return (nil, "core unavailable")
        }
        if let error = json["error"] as? String { return (nil, error) }
        guard let brief = json["brief"] as? [String: Any] else {
            return (nil, "the brief came back without a brief")
        }
        do {
            let decoded = try MessageSerializer.decode(Data(
                try JSONSerialization.data(withJSONObject: brief)
            )) as DesignBrief
            Self.logScaffold(
                "createProject/BriefApplication OK: framework=\(decoded.framework ?? "?"), "
                    + "\(decoded.requires.count) required, \(decoded.suggests.count) suggested"
            )
            return (decoded, nil)
        } catch {
            Self.logScaffold("createProject/BriefApplication DECODE FAILED: \(error)")
            return (nil, "the brief did not decode: \(error)")
        }
    }

    /// **The design**, over `createProject/ParseComposition`: the contents of a `composition.spire` in,
    /// the validated spec a review starts from out.
    ///
    /// A person holding a composition — the file the scaffold writes and a project is edited through —
    /// hands it over rather than describing it again in prose. The Rust side parses it with the same
    /// parser and the same six rules it reads a project's own file with, so what is reviewed here is
    /// held to exactly what a file on a tree is.
    ///
    /// The raw `spec` is returned beside the decoded one on purpose: the wizard sends *that* value to
    /// `createProject/Scaffold`, so the thing scaffolded is the thing that was reviewed — the decoded
    /// type is for reading, never for the trip back. `source` is the door the core names for this leg
    /// (`composition_file`), which the scaffold records, and `name` is the file's own name, so a refusal
    /// names the file to open rather than the step that read it.
    func parseComposition(
        text: String,
        name: String? = nil
    ) async -> (design: ApplicationDesign?, spec: [String: Any]?, source: String?, error: String?) {
        var params: [String: Any] = ["text": text]
        if let name, !name.trimmingCharacters(in: .whitespaces).isEmpty { params["name"] = name }
        guard let json = await callRawMethod("createProject/ParseComposition", args: params) else {
            return (nil, nil, nil, "core unavailable")
        }
        if let error = json["error"] as? String { return (nil, nil, nil, error) }
        guard let spec = json["spec"] as? [String: Any] else {
            return (nil, nil, nil, "the composition came back without a spec")
        }
        let source = json["source"] as? String
        do {
            let design = try MessageSerializer.decode(Data(
                try JSONSerialization.data(withJSONObject: spec)
            )) as ApplicationDesign
            Self.logScaffold(
                "createProject/ParseComposition OK: framework=\(design.framework), "
                    + "\(design.units.count) units, source=\(source ?? "?")"
            )
            return (design, spec, source, nil)
        } catch {
            Self.logScaffold("createProject/ParseComposition DECODE FAILED: \(error)")
            return (nil, spec, source, "the composition did not decode: \(error)")
        }
    }

    /// **The design an open project states, read back for reading only** — `project/applicationDesign`.
    ///
    /// The companion to `parseComposition`, and the one that reads a design *off a tree* rather than out
    /// of a text a person hands in: the composition panel shows what an application in front of the user
    /// is made of. The Rust side reads it with the same reader a unit's Write is built from
    /// (`idf_projects::read_application`), so what is shown is what the code is written from.
    ///
    /// It writes nothing, and it answers `file` beside the design for the reason the panel names the
    /// source: the file a person opens to change the design is the thing to show them. `yaml` is the
    /// composition **as the file holds it**, shown beneath the reading.
    ///
    /// `design == nil` with no `error` is an answer rather than a failure: a project nobody has designed
    /// yet states nothing, and the panel says so.
    func fetchApplicationDesign(
        root: String
    ) async -> (design: ApplicationDesign?, yaml: String?, file: String?, error: String?) {
        guard let json = await callRawMethod("project/applicationDesign", args: ["root": root]) else {
            return (nil, nil, nil, "core unavailable")
        }
        if let error = json["error"] as? String { return (nil, nil, nil, error) }
        let file = json["file"] as? String
        let yaml = json["yaml"] as? String
        guard let spec = json["spec"] as? [String: Any] else {
            // Nothing stated: not an error, and nothing to show but the "not designed yet" state.
            return (nil, yaml, file, nil)
        }
        do {
            let design = try MessageSerializer.decode(Data(
                try JSONSerialization.data(withJSONObject: spec)
            )) as ApplicationDesign
            return (design, yaml, file, nil)
        } catch {
            Self.logScaffold("project/applicationDesign DECODE FAILED: \(error)")
            return (nil, yaml, file, "the design did not decode: \(error)")
        }
    }

    /// **Apply a reviewed design** to the component library: `idf_apply_design` writes a typed stub for
    /// every component the design says has to be written, and reports what it found and what it did.
    ///
    /// It is a separate step from the scaffold, and it happens *before* it: a component the application's
    /// composition includes has to exist — with its manifest, so a `REQUIRES` resolves — by the time the
    /// application's tree is written.
    func applyDesignedComponents(
        libraryRoot: String,
        application: [String: Any]
    ) async -> (result: [String: Any]?, error: String?) {
        guard let json = await callBuildTool(
            "idf_apply_design",
            args: ["root": libraryRoot, "application": application]
        ) else {
            return (nil, "core unavailable")
        }
        if let error = json["error"] as? String { return (nil, error) }
        // **A component the design says is already here and is not.** `apply` reports this in
        // `problems`, not `error`, because the library directory is still written: what breaks is the
        // *build*, when the application's `REQUIRES` names a component nobody has — a raw CMake line
        // that never mentions the design. The problems are the actionable half ("the design says the
        // library already has `moving_average` and it does not"), and the log is where a person reads
        // them when the sheet stops on them.
        let problems = (json["problems"] as? [String]) ?? []
        Self.logScaffold(
            "idf_apply_design OK: added=\((json["added"] as? [Any])?.count ?? 0)"
                + (problems.isEmpty
                    ? ""
                    : "\n\(problems.count) problem(s):\n"
                        + problems.map { "  • \($0)" }.joined(separator: "\n"))
        )
        return (json, nil)
    }

    /// **Did the composition land?** The design's framework and components against the sources the fill
    /// actually wrote, over `createProject/VerifyApplication`.
    ///
    /// A *structural* check, and it exists because a fill that ignores the design still **compiles**: a
    /// live run was handed the reviewed decomposition and answered with one flat FreeRTOS poll loop
    /// instead, and nothing else would have noticed. Empty `gaps` means "not obviously absent" — never
    /// "correct"; the build and a reader are what say that.
    ///
    /// `projectName` is the **same name this sheet handed `scaffoldProject`**, carried back with the
    /// request: the core files the design's decision under that name, and the tree's own directory is
    /// not always that name — a sheet with a `fixedLocation` keeps the folder it was given while the
    /// name field stays editable, so a folder is not a name. The core falls back to the root's own name
    /// when this says nothing, which is right for the default flow (`<location>/<name>`) and wrong the
    /// moment the two differ: the pass would look the decision up under one key, find none, and report
    /// `absent` beside the gaps for a design that was decided.
    func verifyComposition(
        root: String,
        application: [String: Any],
        projectName: String
    ) async -> (gaps: [String], error: String?) {
        guard let json = await callRawMethod(
            "createProject/VerifyApplication",
            args: ["rootDir": root, "application": application, "projectName": projectName]
        ) else {
            return ([], "core unavailable")
        }
        if let error = json["error"] as? String { return ([], error) }
        let gaps = (json["gaps"] as? [String]) ?? []
        // The gaps themselves, in the log: when the sheet stops on them, the log is the one place a
        // person can read why without the message having to be copied out of the UI.
        Self.logScaffold(
            "createProject/VerifyApplication: \(gaps.count) gap(s)"
                + (gaps.isEmpty ? "" : "\n" + gaps.map { "  • \($0)" }.joined(separator: "\n"))
        )
        return (gaps, nil)
    }

    /// **What the composition actually needs.** Rewrite `main/idf_component.yml` from the sources the
    /// fill wrote, over `createProject/FinalizeManifest`.
    ///
    /// The scaffold states the design's dependencies before the composition exists — the board's BSP
    /// unconditionally, because a design that named a board intends to use it. The fill is what decides
    /// whether it *did*: a sensor-only app whose display and touch are still stubs would otherwise pin
    /// the board's whole peripheral stack, download and compile it, and dead-strip every byte — which is
    /// the first build's minutes rather than its seconds. Run once the tree is written and before the
    /// build; the returned list is what the manifest now declares (empty when nothing needs one).
    func finalizeApplicationManifest(
        root: String,
        application: [String: Any]
    ) async -> (dependencies: [String], error: String?) {
        guard let json = await callRawMethod(
            "createProject/FinalizeManifest",
            args: ["rootDir": root, "application": application]
        ) else {
            return ([], "core unavailable")
        }
        if let error = json["error"] as? String { return ([], error) }
        let dependencies = (json["dependencies"] as? [String]) ?? []
        Self.logScaffold(
            "createProject/FinalizeManifest: \(dependencies.count) managed dependency(ies)"
                + (dependencies.isEmpty
                    ? " — none: the composition reaches for no board support"
                    : "\n" + dependencies.map { "  • \($0)" }.joined(separator: "\n"))
        )
        return (dependencies, nil)
    }

    /// Phase 1: scaffold a new project offline via `createProject/Scaffold`.
    /// The Rust core resolves the build module's scaffold_layout, writes all
    /// structural + source-stub files to `root`, runs AnalyzeProject, and
    /// returns the ScaffoldSpec (locked files, fill roots, platforms, layout).
    /// Returns nil on success, error string on failure.
    ///
    /// `embeddedRoot` is the container an `EmbeddedApp` depends on. It travels only for that structure
    /// — the core refuses an application without it — and is omitted entirely otherwise, so every
    /// other request is byte-for-byte what it was.
    /// `designSource` is the **door the reviewed design came through** — `composition_file` for a file a
    /// person wrote, which is the name `parseComposition` answers with, and `answers`/`model` for a graph
    /// written while the design form existed. The core records it as the decision behind the composition
    /// the scaffold writes, and it cannot be derived from the spec, so it is omitted entirely when there
    /// is nothing to say: the core then stores the composition with no decision attached, rather than
    /// inventing one.
    func scaffoldProject(buildSystem: String, projectName: String, root: String,
                         platforms: [String] = [], structure: String = "native",
                         embedded: Bool = false, embeddedRoot: String? = nil,
                         application: [String: Any]? = nil,
                         designSource: String? = nil) async -> String? {
        do {
            var params: [String: Any] = [
                "projectName": projectName,
                "rootDir": root,
                "language": buildSystem,
                "platforms": platforms,
                "structure": structure,
                "embedded": embedded
            ]
            if let embeddedRoot, !embeddedRoot.trimmingCharacters(in: .whitespaces).isEmpty {
                params["embeddedRoot"] = embeddedRoot
            }
            // The **reviewed design**, when there is one. It is the spec the design was read as,
            // handed back untouched: the scaffold states the framework, carries the decomposition as
            // `SPIRE.application.json`, names the design's components in the application's own
            // `REQUIRES`, and hands the fill phase the units, the wiring and the board facts. An
            // application scaffolded without one is the older shape — a blank `app_main` and no
            // stated framework — which is why the parameter is optional and omitted, not empty.
            if let application {
                params["application"] = application
            }
            // **The door that design came through**, carried back from the design phase untouched: the
            // core records it as the decision behind the composition this scaffold writes. Omitted when
            // there is nothing to say — a caller that does not know which door its design came through
            // gets a composition with no decision recorded, never a guessed one.
            if let designSource, !designSource.trimmingCharacters(in: .whitespaces).isEmpty {
                params["designSource"] = designSource
            }
            let body: [String: Any] = [
                "method": "createProject/Scaffold",
                "params": params
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let rawText = String(data: reply, encoding: .utf8) ?? "nil"
            let json = try JSONSerialization.jsonObject(with: reply) as? [String: Any]
            if let err = json?["error"] as? String { return err }
            // Decode the spec and expose it on the bridge. On failure, log the FULL raw reply + the
            // underlying DecodingError so the JSON/Rust mismatch is visible in
            // ~/.spire/logs/spire-scaffold.log.
            do {
                let spec: ScaffoldSpec = try MessageSerializer.decode(reply)
                Self.logScaffold("createProject/Scaffold OK: \(spec.files.count) files, platforms=\(spec.platformTargets)")
                // **Only the spec is exposed — the state deliberately does not move.** Scaffolding is
                // one phase of the sheet's own creation pipeline, and the sheet *is* the surface for
                // it: the modal stays up from design to build, streaming the phases into its log.
                // Moving the state to `.scaffolding` here swapped the workspace pane away from the
                // view that hosts that sheet (`ContentView` shows `WelcomeView` only while the state
                // is `.unconnected`), so the sheet was torn down about a second in and every phase
                // after it — fill, verify, finalize, build — ran with nothing on screen. The state
                // moves once, at the end, when `openProject` opens what was built.
                //
                // The observable is written on the main actor: Observation does not reliably
                // invalidate views for off-main writes.
                await MainActor.run {
                    self.scaffoldSpec = spec
                }
                return nil
            } catch {
                Self.logScaffold("createProject/Scaffold DECODE FAILED\n  buildSystem=\(buildSystem) platforms=\(platforms) root=\(root)\n  error=\(error)\n  reply=\(rawText)")
                return "Unexpected scaffold response: \(rawText)"
            }
        } catch {
            return error.localizedDescription
        }
    }

    /// Append a line to `<core config dir>/logs/spire-scaffold.log` (created on
    /// demand). Dedicated file so scaffold decode details survive even when the
    /// UI error surface can't be copy-pasted.
    static func logScaffold(_ line: String) {
        let url = URL(fileURLWithPath: SpireCorePaths.configDir)
            .appendingPathComponent("logs")
            .appendingPathComponent("spire-scaffold.log")
        try? FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true
        )
        let stamp = ISO8601DateFormatter().string(from: Date())
        if let handle = try? FileHandle(forWritingTo: url) {
            handle.seekToEndOfFile()
            handle.write(Data("\(stamp) \(line)\n".utf8))
            try? handle.close()
        } else {
            try? Data("\(stamp) \(line)\n".utf8).write(to: url, options: .atomic)
        }
    }

    /// The `ScaffoldSpec` as the Rust side reads it.
    ///
    /// Hand-built rather than `JSONEncoder`-ed, because the wire shape is not the Swift shape: Rust's
    /// `ScaffoldSpec` uses snake_case keys and requires a `content` on **every** file — without it the
    /// guard would deserialize an all-empty spec and lose the structural contract the fill (and a
    /// repair) has to respect.
    ///
    /// **`structure` is not optional decoration, and leaving it out was a whole live run.** The core gates
    /// the framework block, the reviewed composition, the composition rules and the library's own headers
    /// on `structure == idf_application`; a spec that arrives without it deserializes as `native` (the
    /// field's `#[serde(default)]`), so a fill asked for `actors` was handed generic rules and a goal and
    /// nothing else — and wrote a bare FreeRTOS poll loop with its own sensor classes. It round-trips
    /// verbatim: the value is the core's own key, decoded from the scaffold response.
    private func scaffoldSpecJSON(_ spec: ScaffoldSpec) -> [String: Any] {
        var json: [String: Any] = [
            "structural_files": spec.structuralFiles,
            "fill_roots": spec.fillRoots,
            "dependency_sections": spec.dependencySections,
            "platform_targets": spec.platformTargets,
            "build_system": spec.buildSystem,
            "structure": spec.structure,
            "embedded": spec.embedded,
            "files": spec.files.map {
                ["path": $0.path, "content": $0.content, "structural": $0.structural]
            }
        ]
        // **The core's report, handed back exactly as it came** — when there was one. It is optional
        // on the wire in both directions (Rust's `Option` with `skip_serializing_if`, this model's
        // `decodeIfPresent`), so it is *omitted* rather than sent as null: a spec without a warning
        // produces the same request it did before the field existed, and a fill or a repair that
        // hands it back cannot turn "nothing to say" into "the report is nil".
        if let warning = spec.designWarning {
            json["design_warning"] = warning
        }
        return json
    }

    /// **Repair a build that failed**, over `createProject/RepairFromBuild`: the compiler's own output
    /// in, whole-file rewrites out, as steps for `executeCreationPlan`.
    ///
    /// The build is the caller's — it owns the toolchain and knows how this machine builds — and this is
    /// the other half: which file each error belongs to, and which of those the scaffold left open. The
    /// diagnostics are passed **verbatim**, notes and include chains included, because the note that says
    /// what the compiler expected is usually the whole repair.
    func repairProject(
        root: String,
        diagnostics: String,
        spec: ScaffoldSpec
    ) async -> (repair: ApplicationRepair?, error: String?) {
        do {
            let body: [String: Any] = [
                "method": "createProject/RepairFromBuild",
                "params": [
                    "rootDir": root,
                    "diagnostics": diagnostics,
                    "spec": scaffoldSpecJSON(spec)
                ]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            if let json = try JSONSerialization.jsonObject(with: reply) as? [String: Any],
               let error = json["error"] as? String {
                return (nil, error)
            }
            let repair: ApplicationRepair = try MessageSerializer.decode(reply)
            Self.logScaffold("createProject/RepairFromBuild OK: \(repair.summary)")
            return (repair, nil)
        } catch {
            Self.logScaffold("createProject/RepairFromBuild FAILED: \(error)")
            return (nil, "the repair could not be read: \(error)")
        }
    }

    /// Phase 2: ask the LLM to fill the materialized scaffold inside its fill
    /// roots (`createProject/Fill`). Returns a constrained plan, nil on error.
    ///
    /// `libraryRoot` is the library an ESP-IDF application is built against, and it is **not decoration**:
    /// the fill prompt is where the model is shown the library's own headers — the framework's
    /// (`actor.hpp`, which carries a worked `spire::Actor`/`spire::Scheduler` example) and the
    /// components the design names — plus the library's `SPIRE.md`. Without it those sections are simply
    /// absent, which is what a live run found: a fill handed the reviewed composition and the prose idiom
    /// but **no API** wrote the only embedded code it knew — a bare FreeRTOS poll loop and its own sensor
    /// classes — instead of `spire::Actor` on a `spire::Scheduler`. The scaffold was already told the
    /// library; the fill was not.
    func fillProject(
        goal: String,
        root: String,
        spec: ScaffoldSpec,
        libraryRoot: String? = nil
    ) async -> PlanGenerationResult? {
        Self.logScaffold(
            "createProject/Fill CALLED (goal='\(goal)' root=\(root) library=\(libraryRoot ?? "none"))"
        )
        do {
            let specDict = scaffoldSpecJSON(spec)
            var params: [String: Any] = [
                "goal": goal,
                "rootDir": root,
                "spec": specDict,
            ]
            if let libraryRoot, !libraryRoot.isEmpty {
                // The same field `createProject/Scaffold` takes: the core reads it as the library whose
                // headers and `SPIRE.md` the fill prompt is written from.
                params["embeddedRoot"] = libraryRoot
            }
            let body: [String: Any] = [
                "method": "createProject/Fill",
                "params": params,
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let plan: PlanGenerationResult = try MessageSerializer.decode(reply)
            Self.logScaffold("createProject/Fill OK: \(plan.steps.count) steps")
            // Do NOT transition bridge.state to .filling here. ContentView
            // switches on bridge.state; re-resolving ProjectWizardView mid-fill
            // destroys its local @State fillPlan (spinner stops, plan never
            // renders). The wizard stores the returned plan in its own local
            // @State on the main actor instead.
            return plan
        } catch {
            Self.logScaffold("createProject/Fill FAILED\n  goal=\(goal) root=\(root)\n  error=\(error)")
            return nil
        }
    }

    /// Deterministic codegen from a validated AppSpec JSON (`createProject/
    /// GenerateCode`): returns the `write_source_file` skeleton steps
    /// (types/actors/FFI dispatch + Swift wrappers/screens). Execute them with
    /// `executeCreationPlan`.
    func generateCodeSteps(projectName: String, spec: [String: Any]) async -> [CreationStep]? {
        Self.logScaffold("createProject/GenerateCode CALLED (project='\(projectName)')")
        let body: [String: Any] = [
            "method": "createProject/GenerateCode",
            "params": ["projectName": projectName, "spec": spec]
        ]
        do {
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let steps: [CreationStep] = try MessageSerializer.decode(reply)
            Self.logScaffold("createProject/GenerateCode OK: \(steps.count) skeleton steps")
            return steps
        } catch {
            Self.logScaffold("createProject/GenerateCode FAILED: \(error)")
            return nil
        }
    }

    /// Result of running a hardcoded build tool (build, test, clean, lint, format).
    struct BuildToolResult {
        let success: Bool
        let output: String
        let command: String?
        let durationSecs: Double?
        let error: String?
        /// Streaming build lines: [{line, level, target}]
        let buildEvents: [BuildEventLine]
    }

    /// Run a hardcoded build tool against a subproject directory.
    /// `tool` is one of: build_build, build_test, build_clean, build_lint, build_format.
    /// `path` is the ABSOLUTE path to the subproject directory.
    func runBuildTool(tool: String, path: String, language: String = "Rust", package: String? = nil) async -> BuildToolResult? {
        do {
            var body: [String: Any] = [
                "method": "tools/call",
                "params": [
                    "tool": tool,
                    "args": [
                        "path": path,
                        "language": language
                    ] as [String: Any]
                ]
            ]
            // Pass the workspace package name when targeting a specific subproject
            // member (e.g. `cargo build --package spire-code`).
            if let package {
                if var params = body["params"] as? [String: Any],
                   var args = params["args"] as? [String: Any] {
                    args["package"] = package
                    params["args"] = args
                    body["params"] = params
                }
            }
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            let json = try JSONSerialization.jsonObject(with: reply) as? [String: Any]
            if let err = json?["error"] as? String {
                // silent: print("[runBuildTool] error: \(err)")
                return BuildToolResult(success: false, output: "", command: nil, durationSecs: nil, error: err, buildEvents: [])
            }
            // The module returns {"result": {"content":[{"type":"text","text": "{...json...}"}], "isError":false}}
            // where the inner text is JSON: {"success":true,"output":"...","command":"..."}
            if let result = json?["result"] as? [String: Any],
               let content = result["content"] as? [[String: Any]],
               let first = content.first,
               let text = first["text"] as? String,
               let payload = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] {
                return BuildToolResult(
                    success: payload["success"] as? Bool ?? false,
                    output: payload["output"] as? String ?? text,
                    command: payload["command"] as? String,
                    durationSecs: payload["durationSecs"] as? Double,
                    error: payload["error"] as? String,
                    buildEvents: parseBuildEvents(payload["buildEvents"])
                )
            }
            // Maybe the reply itself has success/output
            if let success = json?["success"] as? Bool {
                return BuildToolResult(
                    success: success,
                    output: json?["output"] as? String ?? String(data: reply, encoding: .utf8) ?? "",
                    command: json?["command"] as? String,
                    durationSecs: json?["durationSecs"] as? Double,
                    error: json?["error"] as? String,
                    buildEvents: parseBuildEvents(json?["buildEvents"])
                )
            }
            return nil
        } catch {
            // silent: print("[runBuildTool] error: \(error)")
            return nil
        }
    }

    // MARK: - LLM Settings

    /// Load LLM settings from the Rust core via `config/getAll`.
    func fetchLlmConfig() async {
        llmConfigLoading = true
        defer { llmConfigLoading = false }
        do {
            let command = try MessageSerializer.encode(command: "config/getAll")
            let reply = try await backend.send(command)
            let dict = (try JSONSerialization.jsonObject(with: reply) as? [String: Any])?["config"] as? [String: Any] ?? [:]
            llmConfig.apiUrl = dict["deepseek.api_url"] as? String
                ?? "https://api.deepseek.com/v1/chat/completions"
            llmConfig.apiKey = dict["deepseek.api_key"] as? String ?? ""
            llmConfig.model = dict["deepseek.model"] as? String ?? "deepseek-chat"
            llmConfig.planningModel = dict["deepseek.planning_model"] as? String ?? "deepseek-chat"
            llmConfig.codingModel = dict["deepseek.coding_model"] as? String ?? "deepseek-chat"
            llmConfig.tavilyApiKey = dict["tavily.api_key"] as? String ?? ""
        } catch {
            // silent: print("[fetchLlmConfig] error: \(error)")
        }
    }

    /// Save one config key via `config/set`.
    func saveLlmConfigKey(_ key: String, _ value: String) async -> Bool {
        do {
            let body: [String: Any] = [
                "method": "config/set",
                "params": ["key": key, "value": value]
            ]
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            return (try JSONSerialization.jsonObject(with: reply) as? [String: Any])?["success"] as? Bool ?? false
        } catch {
            // silent: print("[saveLlmConfigKey] error: \(error)")
            return false
        }
    }

    /// Save all LLM settings (endpoint + models + Tavily web-search key).
    ///
    /// Works with any OpenAI-compatible backend: `deepseek.*` is the core's
    /// transport namespace, not a provider restriction.
    func saveLlmConfig(_ config: LLMConfig) async -> Bool {
        let urlOk = await saveLlmConfigKey("deepseek.api_url", config.apiUrl)
        let keyOk = await saveLlmConfigKey("deepseek.api_key", config.apiKey)
        let modelOk = await saveLlmConfigKey("deepseek.model", config.model)
        let planningOk = await saveLlmConfigKey("deepseek.planning_model", config.planningModel)
        let codingOk = await saveLlmConfigKey("deepseek.coding_model", config.codingModel)
        let tavilyOk = await saveLlmConfigKey("tavily.api_key", config.tavilyApiKey)
        return urlOk && keyOk && modelOk && planningOk && codingOk && tavilyOk
    }

    /// Fetch every available tool from all backends (core modules + build
    /// modules + internal + MCP servers) via `tools/list`.
    func fetchAllTools() async {
        allToolsLoading = true
        defer { allToolsLoading = false }

        do {
            let command = try MessageSerializer.encode(command: "tools/list")
            let reply = try await backend.send(command)
            let tools: [McpToolInfo] = try MessageSerializer.decode(reply)
            allTools = tools
        } catch {
            allTools = []
        }
    }

    // MARK: - Spec design RPC (free-form AppSpec design step)

    /// One spec-design RPC round trip: returns the parsed reply object (or nil)
    /// plus a non-nil error string on failure.
    private func specDesignCall(method: String, params: [String: Any]) async -> (value: Any?, error: String?) {
        let body: [String: Any] = ["method": method, "params": params]
        do {
            let data = try JSONSerialization.data(withJSONObject: body)
            let reply = try await backend.send(data)
            guard let object = try JSONSerialization.jsonObject(with: reply) as? [String: Any] else {
                return (nil, "spec-design: unreadable reply")
            }
            if let err = object["error"] as? String {
                return (nil, err)
            }
            return (object, nil)
        } catch {
            return (nil, error.localizedDescription)
        }
    }

    /// Open a design session for the project. Returns the initial state.
    func specDesignStart(projectName: String, goal: String, reset: Bool = false) async -> (state: SpecDesignState?, error: String?) {
        let (value, error) = await specDesignCall(method: "spec-design/start", params: ["projectName": projectName, "goal": goal, "reset": reset])
        guard error == nil else { return (nil, error) }
        return (SpecDesignState(json: value as Any), nil)
    }

    /// Convert a freeform spec into the textual AppSpec. Returns the generated
    /// markdown + validation issues + the updated session state.
    func specDesignConvert(projectName: String, specText: String) async -> (outcome: ConvertOutcome?, error: String?) {
        let (value, error) = await specDesignCall(method: "spec-design/convert", params: ["projectName": projectName, "spec_text": specText])
        guard error == nil else { return (nil, error) }
        return (ConvertOutcome(json: value as Any), nil)
    }

    /// Accept the generated AppSpec: validate + persist + decide. Returns the
    /// accepted AppSpec dictionary (feeds the codegen step).
    func specDesignAccept(projectName: String) async -> (spec: [String: Any]?, error: String?) {
        let (value, error) = await specDesignCall(method: "spec-design/accept", params: ["projectName": projectName])
        guard error == nil else { return (nil, error) }
        return (value as? [String: Any], nil)
    }

    /// Return to free-form editing (a decided spec may not meet requirements).
    func specDesignReopen(projectName: String) async -> (state: SpecDesignState?, error: String?) {
        let (value, error) = await specDesignCall(method: "spec-design/reopen", params: ["projectName": projectName])
        guard error == nil else { return (nil, error) }
        return (SpecDesignState(json: value as Any), nil)
    }

    // MARK: - AppSpec design (post-open, project graph backed)

    /// The project name the current design session targets (leaf of the
    /// project root, matching the Rust session key).
    var designProjectName: String {
        guard let root = projectRoot else { return "" }
        return (root as NSString).lastPathComponent
    }

    /// After Decide, materialize the derived AppSpec skeleton into the OPEN
    /// project and refresh the analysis.
    func runSpecDesignCodegen(spec: [String: Any]) async {
        let name = designProjectName
        guard !name.isEmpty else { return }
        Self.logScaffold("Design AppSpec decided for '\(name)' — running codegen")
        if let steps = await generateCodeSteps(projectName: name, spec: spec), !steps.isEmpty,
           let root = projectRoot {
            _ = await executeCreationPlan(rootDir: root, steps: steps)
            await openProject(root: root)
        }
    }

}
