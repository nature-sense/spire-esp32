import Foundation
import SwiftUI

/// The activity log, shown under the History control in the right pane's component detail.
///
/// The model is owned by the **pane**, not by the detail that renders it, because the thing it
/// reports — a model writing a file, a compiler judging it, a test deciding — is not about whatever
/// is selected: the lines survive a click to another component, which is exactly when a person
/// wants to keep an eye on them.
///
/// It sits in the detail's own column, directly below Write / History / Remove, and grows downward
/// from there: a short log is a short box, and past its cap it scrolls rather than pushing the
/// controls above it out of the pane.
///
/// A row is one event as it arrived: icon by source, colour by severity, monospaced because most
/// lines are compiler or file text. Nothing is folded or summarised here — the run's own report is
/// the summary, and this is what led to it.
///
/// While a run is going, one line is not an event yet: a breathing ellipsis under the newest line,
/// so a minute of a model writing reads as work rather than as a stall.
struct ActivityLogView: View {
    @Environment(AppTheme.self) private var theme
    let model: ActivityLogViewModel

    /// One click to hide when it is not what is being watched; nothing is lost when it comes back.
    @State private var expanded = true

    /// The log's own content height, measured so a short log is a short box: the pulse then sits
    /// under the last line instead of at the bottom of a fixed-height frame.
    @State private var contentHeight: CGFloat = 0

    /// The log grows with its content up to this, then scrolls.
    private static let maxLogHeight: CGFloat = 200
    /// Scroll anchor for the line that has not been typed yet.
    private static let workingRowID = "activity-still-working"

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            header
            if expanded {
                if model.lines.isEmpty {
                    Text("Waiting for activity — a build, a test, or a write shows up here.")
                        .font(.caption2)
                        .foregroundStyle(theme.textTertiary)
                        .padding(.vertical, 6)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } else {
                    log
                }
            }
        }
        .padding(8)
        .background(theme.surface, in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(theme.border, lineWidth: 0.5)
        )
    }

    private var header: some View {
        HStack(spacing: 6) {
            Button {
                expanded.toggle()
            } label: {
                Image(systemName: expanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(theme.textSecondary)
            }
            .buttonStyle(.plain)
            .help(expanded ? "Hide the activity log" : "Show the activity log")

            Image(systemName: "text.alignleft")
                .font(.system(size: 10))
                .foregroundStyle(theme.accent)
            Text("Activity")
                .font(.caption.weight(.semibold))
                .foregroundStyle(theme.textSecondary)

            if model.isActive {
                ProgressView().controlSize(.mini)
            }
            Spacer(minLength: 0)
            if !model.lines.isEmpty {
                Text("\(model.lines.count)")
                    .font(.caption2.monospacedDigit())
                    .foregroundStyle(theme.textTertiary)
                Button {
                    model.clear()
                } label: {
                    Image(systemName: "trash")
                        .font(.system(size: 10))
                        .foregroundStyle(theme.textSecondary)
                }
                .buttonStyle(.plain)
                .help("Clear the activity log")
            }
        }
    }

    private var log: some View {
        ScrollViewReader { proxy in
            ScrollView(.vertical, showsIndicators: contentHeight > Self.maxLogHeight) {
                VStack(alignment: .leading, spacing: 2) {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(model.lines) { event in
                            row(event)
                                .id(event.id)
                        }
                    }
                    .padding(.vertical, 2)
                    .frame(maxWidth: .infinity, alignment: .leading)

                    // The last item of the *content*, not a sibling below the scroll view: the pulse
                    // belongs under the newest line however few lines there are, not at the bottom of
                    // a fixed 200 pt frame. It lives in this ordinary stack rather than the
                    // `LazyVStack`, which is the part that once declined to lay it out at all.
                    if model.isActive {
                        workingRow
                            .id(Self.workingRowID)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.vertical, 2)
                    }
                }
                .background(
                    GeometryReader { geometry in
                        Color.clear.preference(
                            key: LogContentHeightKey.self,
                            value: geometry.size.height
                        )
                    }
                )
            }
            .frame(maxWidth: .infinity)
            // Grow with the content up to the cap, then hold and scroll. Until the first
            // measurement arrives, hold the cap rather than collapsing to nothing.
            .frame(height: contentHeight > 0 ? min(contentHeight, Self.maxLogHeight) : Self.maxLogHeight)
            .onPreferenceChange(LogContentHeightKey.self) { contentHeight = $0 }
            .onChange(of: model.lines.count) { _, _ in
                // Instant, not animated: a build emits lines in bursts, and animating each one
                // blocks the main thread for as long as the build runs.
                scrollToEnd(proxy)
            }
            .onChange(of: model.isActive) { _, _ in
                // A tick later: the pulse row is inserted in this same update, and `scrollTo` can
                // only reach an id the content already has.
                DispatchQueue.main.async { scrollToEnd(proxy) }
            }
        }
    }

    /// Keep the newest thing in view: the still-working line while a run is going, the last line
    /// once it has finished.
    private func scrollToEnd(_ proxy: ScrollViewProxy) {
        if model.isActive {
            proxy.scrollTo(Self.workingRowID, anchor: .bottom)
        } else if let last = model.lines.last {
            proxy.scrollTo(last.id, anchor: .bottom)
        }
    }

    /// The line that has not arrived yet: three dots that light and lift in turn while the run is
    /// out of sight — a model writing a file, a compiler linking — so a long quiet stretch reads as
    /// work rather than as a stall.
    ///
    /// It is the last item of the log's content, so it sits under the newest line however few lines
    /// there are; the box grows to hold it rather than pinning it to a fixed bottom.
    private var workingRow: some View {
        HStack(spacing: 4) {
            // The rows' icon column, kept so the dots start where their text does.
            Color.clear.frame(width: 10, height: 8)
            BreathingEllipsis()
        }
        .accessibilityLabel("Still working")
    }

    private func row(_ event: SpireBridge.BuildEventLine) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            Image(systemName: icon(for: event))
                .font(.system(size: 8))
                .foregroundStyle(color(for: event))
                .frame(width: 10, alignment: .center)
            Text(event.line)
                .font(.caption2.monospaced())
                .foregroundStyle(textColor(for: event))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .help(event.file ?? "")
    }

    /// What produced the line. The coordinator names its source; a build line predates that and is
    /// a build line by definition.
    private func icon(for event: SpireBridge.BuildEventLine) -> String {
        switch event.level {
        case "error": return "xmark.circle.fill"
        case "warning": return "exclamationmark.triangle.fill"
        case "finished": return "checkmark.circle.fill"
        default: break
        }
        switch event.source {
        case "llm": return "sparkles"
        case "test": return "checkmark.seal"
        case "build": return "hammer.fill"
        default: return "circle.fill"
        }
    }

    private func color(for event: SpireBridge.BuildEventLine) -> Color {
        switch event.level {
        case "error": return .red
        case "warning": return .yellow
        case "finished": return .green
        default: break
        }
        switch event.source {
        case "llm": return theme.accent
        case "test": return .teal
        case "build": return theme.textSecondary
        default: return theme.textTertiary
        }
    }

    private func textColor(for event: SpireBridge.BuildEventLine) -> Color {
        switch event.level {
        case "error", "warning", "finished":
            return color(for: event)
        default:
            return theme.textPrimary
        }
    }
}

/// The log's content height, measured so its box can be as tall as what it holds rather than a
/// fixed 200 pt.
private struct LogContentHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = max(value, nextValue())
    }
}

/// The ellipsis that has not been typed yet: three dots that light and lift in turn while work is
/// going on.
///
/// Ticked by a `TimelineView` rather than animated. On this macOS the implicit animations that
/// carried the pulse — `phaseAnimator`, `TimelineView(.animation)`, and a `repeatForever` started
/// from `onAppear` — all rendered and none of them moved; the last one left a perfectly still
/// ellipsis. A periodic timeline is not an animation at all: it re-evaluates the body every tick
/// and the dots are computed from the schedule's own date, so they cannot be parked or suppressed.
///
/// Drawn as three accent circles rather than a "…" glyph: at the log's caption size that glyph's
/// dots are barely a pixel across and vanished against the rows, no matter how large the font. The
/// dots never dim past a third, so even a still frame reads as an ellipsis.
private struct BreathingEllipsis: View {
    @Environment(AppTheme.self) private var theme

    /// One dot's full light-and-lift cycle.
    private static let period: Double = 1.3
    /// 30 Hz: smooth to the eye, and only three circles re-evaluate.
    private static let tick: Double = 1.0 / 30
    private static let diameter: CGFloat = 7
    private static let gap: CGFloat = 4
    /// How far a dot rises at the top of its cycle.
    private static let lift: CGFloat = 2
    /// Each dot lags the one before it, so the wave travels rightwards like typing.
    private static let lag: Double = 0.16

    var body: some View {
        TimelineView(.periodic(from: .now, by: Self.tick)) { context in
            HStack(spacing: Self.gap) {
                ForEach(0..<3, id: \.self) { index in
                    let wave = Self.wave(at: context.date, index: index)
                    Circle()
                        .fill(theme.accent)
                        .frame(width: Self.diameter, height: Self.diameter)
                        .opacity(0.35 + 0.65 * wave)
                        .offset(y: -Self.lift * CGFloat(wave))
                }
            }
            // Hold the room the lift borrows, so the row never jumps as the dots move.
            .frame(height: Self.diameter + 2 * Self.lift)
        }
    }

    /// 0 at the bottom of a dot's cycle, 1 at the top.
    private static func wave(at date: Date, index: Int) -> Double {
        // Take the fraction before scaling by 2π: the raw interval is ~10⁹ s, and keeping it whole
        // would spend precision on turns nobody can see.
        let turns = (date.timeIntervalSinceReferenceDate / period - Double(index) * lag)
            .truncatingRemainder(dividingBy: 1)
        return 0.5 + 0.5 * sin(2 * .pi * turns)
    }
}
