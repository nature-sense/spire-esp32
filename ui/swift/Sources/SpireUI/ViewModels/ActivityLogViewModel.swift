import Foundation
import Observation

/// The right pane's **activity log**: what the model wrote, what the compiler said about it, and
/// what the tests decided — one scrolling timeline, live.
///
/// It reads the same stream the build panel reads, from the same `BuildService`, because the Rust
/// side pushes both through one build-event log (`CoordinatorMessage::SetActivitySink`). That is
/// what makes the order mean something: a rewrite and the compile that judged it arrive as
/// adjacent lines rather than as two panels a person has to correlate.
///
/// Lines are kept, not merged: a run is a sequence, and a summary computed here would be a second
/// account of what the report already says.
@MainActor
@Observable
final class ActivityLogViewModel {
    /// Oldest first, newest last. Bounded — a long edit emits a line per file per attempt, and a
    /// log that grows without a limit is a memory leak with a scrollbar.
    private(set) var lines: [SpireBridge.BuildEventLine] = []

    /// True while a run is still going — drives the header's spinner, and the breathing ellipsis
    /// the log keeps under its newest line.
    private(set) var isActive = false

    /// How many lines are kept: enough to scroll back through a whole edit run.
    static let capacity = 2_000

    /// How long silence clears the spinner on a stream that never announced a run. A build's last
    /// line is the build's end, not the run's, so the absence of lines is the only signal there is.
    private let buildSilence: Duration

    /// The safety net for a run that *did* announce itself. Such a run closes with its own verdict,
    /// and this only catches one that died without emitting it — so it is long on purpose: between
    /// a prompt and its answer the log is silent for a minute or more, and that silence is the
    /// model writing, not the run ending.
    private let runSilence: Duration

    /// Set by the one `info` line that opens a run: `Edit: …`, `Component …: …`, or
    /// `Fix & Verify: …`. A stream without that banner (a plain build, a one-off propose) has no
    /// verdict to wait for and is cleared by silence instead.
    private var announcedRun = false

    /// The shared service and this subscription's token. The token is what lets this view go away
    /// without stopping the drain loop the build panel is also reading from.
    private var service: BuildService?
    private var token: UUID?

    /// Set when the view goes away while the subscription is still being attached. The attach
    /// completes on the service's actor, so the token may not exist yet when `stop()` runs —
    /// without this, that handler would outlive the view it was made for.
    private var cancelling = false

    /// Clears `isActive` once the run ends or goes quiet for too long.
    private var idleTask: Task<Void, Never>?

    init(
        buildSilence: Duration = .seconds(2),
        runSilence: Duration = .seconds(180)
    ) {
        self.buildSilence = buildSilence
        self.runSilence = runSilence
    }

    /// Subscribe to the shared build/activity stream. Idempotent: a second `.task` on a view that
    /// was re-created does not attach a second handler.
    func start(using service: BuildService) {
        guard token == nil else { return }
        self.service = service
        cancelling = false
        Task {
            let token = await service.startEventConsumer { [weak self] incoming in
                // `BuildService.deliver` already hops to the MainActor; the closure is @Sendable,
                // so the hop is restated here to satisfy the isolation checker.
                Task { @MainActor [weak self] in
                    self?.append(incoming)
                }
            }
            if cancelling {
                await service.removeEventConsumer(token)
                return
            }
            self.token = token
        }
    }

    /// Detach this view's handler. The drain loop stops only when nobody is listening.
    func stop() {
        idleTask?.cancel()
        idleTask = nil
        cancelling = true
        guard let token, let service else { return }
        self.token = nil
        Task { await service.removeEventConsumer(token) }
    }

    func clear() {
        lines = []
        isActive = false
        // The run, if any, is still going — `announcedRun` is kept so its next line is not mistaken
        // for the tail of a plain build and cleared two seconds later.
        idleTask?.cancel()
        idleTask = nil
    }

    /// Take one burst from the stream.
    ///
    /// Internal rather than private so a test can hand the log a burst without standing up a
    /// service and its drain loop; the only caller in the app is the subscription in `start`.
    func append(_ incoming: [SpireBridge.BuildEventLine]) {
        guard !incoming.isEmpty else { return }
        lines.append(contentsOf: incoming)
        if lines.count > Self.capacity {
            lines.removeFirst(lines.count - Self.capacity)
        }

        idleTask?.cancel()
        idleTask = nil

        if incoming.contains(where: Self.closesRun) {
            isActive = false
            announcedRun = false
            return
        }
        if incoming.contains(where: Self.opensRun) {
            announcedRun = true
        }
        isActive = true

        // A run that announced itself goes quiet for as long as its model takes; only a run that
        // did not is cleared on silence. Neither is cleared by the last line arriving, because a
        // build's last line is the build's end, not the run's — a rewrite is followed by a compile.
        let silence = announcedRun ? runSilence : buildSilence
        idleTask = Task { [weak self] in
            try? await Task.sleep(for: silence)
            guard !Task.isCancelled else { return }
            self?.isActive = false
            self?.announcedRun = false
        }
    }

    /// The line that opens a run in the pane: one `info` line naming what was asked, emitted before
    /// any work and never repeated. `verify` is in here because Fix & Verify announces itself with
    /// the phase of its verdict — the level is what tells the two apart.
    private static func opensRun(_ event: SpireBridge.BuildEventLine) -> Bool {
        event.level == "info" && (event.phase == "plan" || event.phase == "verify")
    }

    /// The line that closes one: the run's own verdict (`✓`/`✗` at `verify`), or the refusal that
    /// ended it before any work (`✗ …` at `plan`). Per-file failures carry `generate`, because the
    /// run goes on past them.
    private static func closesRun(_ event: SpireBridge.BuildEventLine) -> Bool {
        (event.level == "finished" || event.level == "error")
            && (event.phase == "plan" || event.phase == "verify")
    }
}
