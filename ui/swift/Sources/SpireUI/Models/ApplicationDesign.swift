import Foundation

/// The **design phase's answer**, as the person reviewing it sees it.
///
/// Everything here is decided by the Rust side (`createProject/DesignApplication`, and the six rules in
/// `application_spec`); this decodes what a reviewer has to read, and nothing more. The spec travels
/// back to `createProject/Scaffold` **exactly as it was received** — the wizard keeps the raw JSON — so
/// what is approved is what is scaffolded, with no round trip through this type in between.
///
/// The one line this adds is `marker`: the framework reaches the tree as a single stated line, and a
/// reviewer who is confirming a choice should see the line itself rather than a summary of it.
struct ApplicationDesign: Codable, Equatable {
    struct Board: Codable, Equatable {
        let chip: String
        let bsp: String
        var hal: String?

        /// The board as `createProject/*` takes it. `hal` is left out when empty: it is a vendor
        /// abstraction's name (`m5unified`, `bsp`) and a blank string is not one.
        var json: [String: Any] {
            var out: [String: Any] = ["chip": chip, "bsp": bsp]
            if let hal, !hal.trimmingCharacters(in: .whitespaces).isEmpty { out["hal"] = hal }
            return out
        }
    }

    /// A device the *application* declares it composes against. `via` says how it is reached, and
    /// `bus`/`address` are the **board facts** — a bus and an address belong to the product, never to a
    /// component, which is why they live on the resource that declares the device. The Rust side checks
    /// the whole layer; this decodes what a reviewer reads.
    struct Resource: Codable, Equatable {
        let name: String
        var roles: [String]?
        var via: String?
        var bus: String?
        var address: String?

        /// One line of the review: `sps30 on i2c at 0x69`, or `display` when it states neither.
        var summary: String {
            var text = name
            if let bus, !bus.trimmingCharacters(in: .whitespaces).isEmpty { text += " on \(bus)" }
            if let address, !address.trimmingCharacters(in: .whitespaces).isEmpty {
                text += " at \(address)"
            }
            return text
        }
    }

    /// A **message the composition declares** — one of the three shapes the format allows.
    ///
    /// An actor's `message` names one of these by its id, so a reviewer reading `actor on supervision`
    /// cannot tell what actually arrives until the declaration is shown beside it. Rust's `MessageDecl`
    /// allows three shapes and leaves out the half a declaration does not use, so all three arrive here
    /// carrying only the keys they state:
    ///
    /// - a **set** states `kinds` — the several kinds one actor receives, derived as a `std::variant`;
    /// - a **type** states `fields` — the members of a message another actor may name on its own;
    /// - a **stub** states neither — the shape is left to the fill, and only the id is known.
    ///
    /// `kinds` is decoded as optional for exactly that reason: a required `kinds` is the reading a
    /// set-shaped design suggests, and it is what makes a type-shaped file — `Reading` has fields and
    /// no kinds — fail to open at all.
    struct Message: Codable, Equatable {
        let id: String
        /// The kinds of a **set**; absent on a type or a stub.
        var kinds: [String]?
        /// The members of a **type**; absent on a set or a stub.
        var fields: [MessageField]?
        var description: String?

        /// One line of the review — the shape made visible rather than named:
        /// `Reading, Command, Heartbeat — one actor receives any one of them` for a set,
        /// `counter: unsigned` for a type, and nothing for a stub, which states no shape yet.
        var summary: String {
            if let kinds, !kinds.isEmpty {
                return "\(kinds.joined(separator: ", ")) — one actor receives any one of them"
            }
            if let fields, !fields.isEmpty {
                return fields
                    .map { "\($0.name): \($0.type ?? "a type")" }
                    .joined(separator: ", ")
            }
            return ""
        }
    }

    /// A **member** of a type-shaped message, as the composition states it: `counter: unsigned`.
    struct MessageField: Codable, Equatable {
        let name: String
        var type: String?
        var description: String?
    }

    let framework: String
    var justification: String?
    /// The application **in English** — the person's own words, from the composition. It is not read by
    /// the core's rules; it is the sentence the generated code is headed with, shown at review so the
    /// person confirms the *words* as well as the graph.
    var description: String?
    let board: Board
    var resources: [Resource]?
    /// The **messages** the design declares: sets an actor receives as a variant, types it receives
    /// whole, and stubs whose shape the fill writes. An actor's `message` names one of these by id,
    /// which is why the review shows the declaration beside the actor rather than the id alone.
    var messages: [Message]?
    let units: [Unit]
    var wiring: [String]?

    /// The line the application will state its framework with — the whole of what the choice becomes in
    /// the tree, which is why it is shown at review.
    var marker: String { "set(SPIRE_APPLICATION_FRAMEWORK \(framework))" }

    /// The **board facts** the review shows: each device the application's *own* drivers reach, with the
    /// bus and address it states — a fact no component may carry, and now a field of the resource that
    /// declares the device. A `via: board` part is reached through the BSP, so it is not shown here.
    var facts: [Resource] {
        (resources ?? []).filter {
            $0.via != "board"
                && (!($0.bus ?? "").trimmingCharacters(in: .whitespaces).isEmpty
                    || !($0.address ?? "").trimmingCharacters(in: .whitespaces).isEmpty)
        }
    }
    var edges: [String] { wiring ?? [] }
    var declaredMessages: [Message] { messages ?? [] }

    /// The components, and the units that use them. The split is the one a reader needs: the first list
    /// is what the *library* is made of (and what has to exist before a build), the second is what this
    /// application is.
    var components: [Unit] { units.filter(\.isComponent) }
    var composition: [Unit] { units.filter { !$0.isComponent } }

    /// The components the design says have to be **written** — the stubs. An application with these and
    /// no library has nowhere to put them, which is worth saying at review rather than discovering at a
    /// build that cannot resolve a `REQUIRES`.
    var componentsToWrite: [Unit] { components.filter { $0.source == "stub" } }

    /// The components the design draws from the **registry** rather than the library or a stub — the
    /// managed dependencies the application's `main/idf_component.yml` will name. Shown at review so a
    /// person sees which upstream component is coming, and deliberately *not* in `componentsToWrite`:
    /// nothing is written into the library for these.
    var componentsFromRegistry: [Unit] { components.filter { $0.source == "published" } }

    var frameworkLabel: String {
        switch framework {
        case "actors": return "actors — units that hold state and react to messages"
        case "ramen": return "ramen — values pushed through wired stages"
        default: return framework
        }
    }

    /// A unit of the decomposition. One type with optional fields rather than three types, because the
    /// *spec* has one: the kind decides which fields are meaningful, and a reviewer reads whichever are
    /// there.
    struct Unit: Codable, Equatable {
        let id: String
        /// `component` (framework-agnostic), `actor`, or `stage`.
        let kind: String
        /// Components: `driver` or `library`.
        var role: String?
        /// Components: `existing` (the library has it), `stub` (to be written), or `published` (a
        /// managed dependency from the ESP Component Registry).
        var source: String?
        /// `published` components only: the registry's own `namespace/name` — what `idf_component.yml`
        /// resolves. A reviewer reads it to see *which* upstream component, not just that there is one.
        var registry: String?
        var provides: String?
        /// The unit **in English** — what this actor (or driver, or stage) does, in the person's own
        /// words. It heads the unit's generated class doc-comment; optional, since a driver's `provides`
        /// line is usually all it needs.
        var description: String?
        var bus: String?
        var message: String?
        var state: String?
        var pulls: String?
        var pushes: String?
        var uses: [String]?
        var sendsTo: [String]?

        enum CodingKeys: String, CodingKey {
            case id, kind, role, source, registry, provides, description, bus, message, state, pulls,
                pushes, uses
            case sendsTo = "sends_to"
        }

        var isComponent: Bool { kind == "component" }

        /// One line of the review, saying what this unit *is*: a driver on a bus, a library, an actor on
        /// a message, a stage that pulls and pushes. The shape is the part a reviewer checks — a unit
        /// described only by its name is a unit the person cannot disagree with.
        var summary: String {
            switch kind {
            case "component":
                var text = role ?? "component"
                if let bus, !bus.isEmpty { text += " on \(bus)" }
                if source == "existing" {
                    text += " — the library has it"
                } else if source == "stub" {
                    text += " — to be written"
                } else if source == "published" {
                    text += " — from the registry (\(registry ?? "?"))"
                }
                if let provides, !provides.isEmpty { text += ": \(provides)" }
                return text
            case "actor":
                var text = "actor on \(message ?? "?")"
                if let state, !state.isEmpty { text += ", holding \(state)" }
                if let uses, !uses.isEmpty { text += ", uses \(uses.joined(separator: ", "))" }
                if let sendsTo, !sendsTo.isEmpty {
                    text += ", sends to \(sendsTo.joined(separator: ", "))"
                }
                return text
            default:
                return "stage: pulls \(pulls ?? "nothing"), pushes \(pushes ?? "nothing")"
            }
        }

        /// The **words a unit's Write box starts from**: the design's own sentence about the unit
        /// (its `description`) and the state it says the unit holds, joined with an em dash.
        ///
        /// The box asks what the unit *does*, which the design cannot supply on its own — but a design
        /// that already states a sentence has said most of it, and an empty box would make the person
        /// retype what the file already holds. A side the design does not state is left out rather
        /// than written as a dangling dash; `nil` when the unit states neither, so an undescribed unit
        /// keeps the empty box (and the empty box still means "nothing is invented").
        var prefillText: String? {
            let parts = [description, state]
                .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }
            return parts.isEmpty ? nil : parts.joined(separator: " — ")
        }

        /// Whether this unit is the component the tree holds under `componentName`.
        ///
        /// A unit id is the design's (`air-quality`); a component directory is the scaffold's
        /// `component_ident` (`air_quality`). The Write sheet knows only the directory name, so the
        /// id is folded the same way before the two are compared.
        func matches(componentName: String) -> Bool {
            id.replacingOccurrences(of: "-", with: "_") == componentName
        }
    }
}
