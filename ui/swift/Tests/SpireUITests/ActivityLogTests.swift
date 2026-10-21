import Foundation
import Testing
@testable import SpireUI

/// **When the activity log is allowed to say it is still working.**
///
/// The pane's whole job is to be believed during a quiet stretch: a rewrite is one prompt and one
/// answer, and the answer can be a minute away. The spinner is therefore cleared by the run's own
/// verdict, not by the last line's age — a rule that only holds if a run's verdict is recognised
/// and a plain build's silence still is. Both are pinned here, plus the two windows they sit on.
@MainActor
struct ActivityLogTests {
    /// A line as the coordinator emits it: the build-event wire shape, source and phase included.
    private func line(
        _ text: String,
        level: String = "info",
        source: String = "llm",
        phase: String = "generate"
    ) -> SpireBridge.BuildEventLine {
        SpireBridge.BuildEventLine(line: text, level: level, source: source, phase: phase)
    }

    /// `Edit: …` — the one line that opens a code-modify run.
    private var banner: SpireBridge.BuildEventLine {
        line("Edit: add a temperature field", level: "info", phase: "plan")
    }

    /// The run's closing line: what was kept, what was reverted, how long it took.
    private var verdict: SpireBridge.BuildEventLine {
        line("✓ Edit: 1 file(s) kept, verified against build + host tests · 1m 09s",
             level: "finished", phase: "verify")
    }

    /// A log whose clear-me-on-silence window is short enough to actually wait for.
    private func log(
        buildSilence: Duration = .seconds(30),
        runSilence: Duration = .seconds(30)
    ) -> ActivityLogViewModel {
        ActivityLogViewModel(buildSilence: buildSilence, runSilence: runSilence)
    }

    @Test("an announced run stays working through its silent model call, and its verdict ends it")
    func aSilentModelCallDoesNotEndTheRun() async {
        let model = log(buildSilence: .milliseconds(20), runSilence: .seconds(10))

        model.append([banner])
        #expect(model.isActive)
        model.append([line("Writing src/sht20.cpp…")])

        // Longer than the build-only window, far shorter than the minute a model can take: this is
        // the stretch that used to clear the spinner and leave the pane looking stalled.
        try? await Task.sleep(for: .milliseconds(150))
        #expect(model.isActive)

        model.append([verdict])
        #expect(!model.isActive, "the run's own verdict is the one line that ends it")
    }

    @Test("fix & verify's opening banner is not mistaken for its verdict")
    func theBannerIsNotTheVerdict() {
        let model = log()
        // Same phase as the closing line; only the level distinguishes them.
        model.append([line("Fix & Verify: ESP32 · up to 2 round(s)", level: "info", phase: "verify")])
        #expect(model.isActive)
    }

    @Test("a build-only stream is cleared by silence, because nothing else ends it")
    func aBuildIsClearedBySilence() async {
        let model = log(buildSilence: .milliseconds(20))
        model.append([
            SpireBridge.BuildEventLine(line: "Building…", level: "info"),
            SpireBridge.BuildEventLine(line: "Compiling sht20.cpp", level: "info", source: "build",
                                       phase: "compile"),
        ])
        #expect(model.isActive)

        try? await Task.sleep(for: .milliseconds(150))
        #expect(!model.isActive, "no verdict is coming; the end of the lines is the end of the build")
    }

    @Test("a refusal before any work ends the run, and a per-file failure does not")
    func onlyTheClosingFailureEndsTheRun() {
        let model = log()

        model.append([banner])
        model.append([line("✗ src/sht20.cpp: the reply was lost · 12s", level: "error")])
        #expect(model.isActive, "one file failing is not the run failing")

        model.append([line("✗ sht20: the framework is not a component to edit",
                           level: "error", phase: "plan")])
        #expect(!model.isActive, "refused before any work: there is nothing left to wait for")
    }
}
