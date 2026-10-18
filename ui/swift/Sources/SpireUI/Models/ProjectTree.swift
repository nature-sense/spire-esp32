import Foundation

/// Returns a display name for the build system / language.
func badgeLabel(for buildSystem: String) -> String {
    switch buildSystem {
    case "Cargo": return "Rust"
    case "SwiftPM", "Xcode": return "Swift"
    case "npm", "pnpm", "yarn": return "JS"
    default: return buildSystem
    }
}

/// Returns a badge color for the build system.
func badgeColor(for buildSystem: String) -> String {
    switch buildSystem {
    case "Cargo": return "#DE3C3C"     // Rust orange-red
    case "SwiftPM", "Xcode": return "#F05138"  // Swift orange
    case "npm", "pnpm", "yarn": return "#68A063" // Node green
    default: return "#6B7280"          // gray
    }
}
