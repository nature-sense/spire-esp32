import Foundation

/// One row in the welcome screen's **New project** list.
///
/// A flat list, deliberately. The shape of a project is one of a handful of things, and a wizard
/// that asks three questions to arrive at one of them is a wizard to get past — so the group
/// heading is decoration and the row is the decision.
struct ProjectType: Identifiable, Hashable {
    /// Stable key. The `ProjectStructure` spelling where the row scaffolds for real, and a
    /// placeholder name where it does not.
    let id: String
    let title: String
    let subtitle: String
    /// The `ProjectStructure` key this row scaffolds with — `"idf_library"` or
    /// `"idf_application"` — or `nil` while the row is a **stub**.
    ///
    /// `nil` is the whole gate: a row that has one opens the name-and-location sheet and scaffolds
    /// through `createProject/Scaffold`; a row that does not is inert and says so on hover. That is
    /// one place to look rather than a list of enabled rows kept somewhere else.
    let structure: String?
}

/// A sub-heading in the **New project** list, and the rows under it.
struct ProjectTypeGroup: Identifiable {
    let id: String
    let title: String
    let types: [ProjectType]

    /// The whole list, in the order it is shown.
    ///
    /// **ESP32/ESP-IDF only.** This tree is the ESP32 domain: a component library and the
    /// applications built against it. Both rows carry a real structure key, so both scaffold —
    /// there is no stub row waiting for a structure this binary does not have.
    static let all: [ProjectTypeGroup] = [
        ProjectTypeGroup(
            id: "embedded",
            title: "Embedded",
            types: [
                ProjectType(
                    id: "idf_library",
                    title: "ESP32 Components",
                    subtitle: "A library of reusable ESP-IDF components",
                    structure: "idf_library"
                ),
                ProjectType(
                    id: "idf_application",
                    title: "ESP32 Application",
                    subtitle: "An ESP-IDF application, built on a component library",
                    structure: "idf_application"
                ),
            ]
        ),
    ]
}
