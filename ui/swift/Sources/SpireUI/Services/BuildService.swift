import Foundation
import Observation

/// Domain service owning all build/lint/fix operations plus the single live
/// build-event consumer. Being an actor serializes access to the shared, mutable
/// build-event state, eliminating the race between the event waiter and the
/// view's buildEvents mutation.
///
/// The Rust side uses tokio::sync::Notify (async push): the forwarder task
/// pushes events to a shared buffer and signals BUILD_NOTIFY; the FFI waiter
/// drains first, then waits — so there is exactly one consumer here.
struct BuildToolResult {
    let success: Bool
    let output: String
    let command: String?
    let durationSecs: Double?
    let error: String?
    let buildEvents: [SpireBridge.BuildEventLine]
}

actor BuildService {
    let backend: any UIBackend

    /// Single, long-lived consumer for build events. Notify only wakes ONE
    /// waiter per notify_one(), so there must be exactly one of these.
    private var drainTask: Task<Void, Never>?
    /// Who is listening. A map rather than one handler because the right pane has two readers with
    /// different jobs — the build log and the activity log — and the Rust side wakes **one** waiter
    /// per event, so a second consumer would steal lines instead of sharing them.
    private var subscribers: [UUID: @Sendable ([SpireBridge.BuildEventLine]) -> Void] = [:]

    init(backend: any UIBackend) {
        self.backend = backend
    }

    // MARK: - FFI calls

    /// Run a build tool (build/lint/fix/clean/test) and return the result.
    func runTool(_ tool: String, path: String, language: String = "Rust", package: String? = nil, platform: String? = nil, target: String? = nil, prompt: String? = nil) async throws -> BuildToolResult {
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
        // Target a specific workspace package, e.g. `cargo build --package spire-code`.
        if let package {
            if var params = body["params"] as? [String: Any],
               var args = params["args"] as? [String: Any] {
                args["package"] = package
                params["args"] = args
                body["params"] = params
            }
        }
        // Cross-platform build target (e.g. "host" or "rpi5" for Meson projects).
        if let platform {
            if var params = body["params"] as? [String: Any],
               var args = params["args"] as? [String: Any] {
                args["platform"] = platform
                params["args"] = args
                body["params"] = params
            }
        }
        // Specific build target within the project (e.g. Meson executable name).
        if let target {
            if var params = body["params"] as? [String: Any],
               var args = params["args"] as? [String: Any] {
                args["target"] = target
                params["args"] = args
                body["params"] = params
            }
        }
        // Free-text request for `modify_code`: the user's own words are the prompt, and
        // what should change is not expressible as a tool name.
        if let prompt {
            if var params = body["params"] as? [String: Any],
               var args = params["args"] as? [String: Any] {
                args["prompt"] = prompt
                params["args"] = args
                body["params"] = params
            }
        }
        let data = try JSONSerialization.data(withJSONObject: body)
        let reply = try await backend.send(data)
        let json = try JSONSerialization.jsonObject(with: reply) as? [String: Any]
        if let err = json?["error"] as? String {
            return BuildToolResult(success: false, output: "", command: nil, durationSecs: nil, error: err, buildEvents: [])
        }
        // Module returns {"result":{"content":[{"type":"text","text":"{...}"}],"isError":false}}
        // where inner text is JSON: {"success":true,"output":"...","command":"...","buildEvents":[...]}
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
        throw BuildServiceError.unrecognizedResponse
    }

    // MARK: - Live event consumer (async push, no polling)

    /// Attach a handler and return its token.
    ///
    /// The drain loop starts with the **first** subscriber and keeps running while any remain, so
    /// `waitForBuildEvent` is never called twice: the Rust side wakes exactly one waiter, and a
    /// second loop would take lines away from the first rather than copy them.
    @discardableResult
    func startEventConsumer(handler: @escaping @Sendable ([SpireBridge.BuildEventLine]) -> Void) -> UUID {
        let token = UUID()
        subscribers[token] = handler
        if drainTask == nil {
            drainTask = Task.detached { [weak self] in
                while !Task.isCancelled {
                    guard let self else { break }
                    guard let json = self.backend.waitForBuildEvent(timeoutMs: 10000),
                          let data = json.data(using: .utf8),
                          let arr = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]],
                          !arr.isEmpty
                    else { continue }
                    let lines = arr.compactMap { BuildService.parse($0) }
                    if !lines.isEmpty {
                        // Deliver on the main actor so UI state mutations are serial.
                        await self.deliver(lines)
                    }
                }
            }
        }
        return token
    }

    /// Detach one handler. The loop stops when the last handler goes, so a view that is torn down
    /// does not leave a waiter behind.
    func removeEventConsumer(_ token: UUID) {
        subscribers[token] = nil
        if subscribers.isEmpty {
            drainTask?.cancel()
            drainTask = nil
        }
    }

    /// Stop the single build-event consumer and drop every handler — for a private service whose
    /// pipeline has finished (`CreateProjectSheet`), not for a shared one.
    func stopEventConsumer() {
        drainTask?.cancel()
        drainTask = nil
        subscribers.removeAll()
    }

    func deliver(_ lines: [SpireBridge.BuildEventLine]) async {
        let handlers = Array(subscribers.values)
        guard !handlers.isEmpty else { return }
        await MainActor.run {
            for handler in handlers {
                handler(lines)
            }
        }
    }

    private static func parse(_ d: [String: Any]) -> SpireBridge.BuildEventLine? {
        guard let line = d["line"] as? String else { return nil }
        return SpireBridge.BuildEventLine(
            line: line,
            level: d["level"] as? String ?? "info",
            target: d["target"] as? String,
            file: d["file"] as? String,
            lineNumber: d["line_number"] as? Int,
            message: d["message"] as? String,
            detail: d["detail"] as? String,
            source: d["source"] as? String,
            phase: d["phase"] as? String
        )
    }

    private func parseBuildEvents(_ value: Any?) -> [SpireBridge.BuildEventLine] {
        guard let arr = value as? [[String: Any]] else { return [] }
        return arr.compactMap { Self.parse($0) }
    }
}

enum BuildServiceError: Error {
    case unrecognizedResponse
}