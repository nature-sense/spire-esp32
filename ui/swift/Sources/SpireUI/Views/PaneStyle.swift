import SwiftUI

// MARK: - Shared card styling

/// The darker "card" treatment used by the navigator and the panes built like it:
/// rounded `theme.surface` fill + subtle border + padding.
///
/// It lives here rather than beside one pane because the navigator, the build
/// target list and the detail pane all draw the same card — one definition is
/// what keeps them from drifting apart as the panes change.
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
