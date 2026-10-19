import Foundation

/// **A component's contract**, as `project/componentSpec` answers it.
///
/// The spec is the component's source of truth: what it is, what a fill should make it, the device
/// its facts are looked up by, the capabilities it provides, and the corpus documents it was written
/// from. `SPIRE.component.yaml` is a rendered copy of this — never its source.
///
/// Decoding is **lenient** in the same way `ComponentEditRecord` is: a value written by an older
/// build may be missing what a newer one states, and a pane that refuses to open is worse than one
/// with a gap in it. The one field that is required is `spec`, because a reading without a spec is
/// not a reading at all.
struct ComponentSpec: Codable, Equatable {
    /// The device behind a driver: the part number is the authoritative `device-facts` key, which is
    /// not always the word the component's directory uses.
    struct Device: Codable, Equatable {
        var part: String
        var bus: String
        var address: String?

        /// One line of the reading: `sps30 on i2c at 0x69`.
        var summary: String {
            var text = "\(part) on \(bus)"
            if let address, !address.isEmpty { text += " at \(address)" }
            return text
        }
    }

    /// One corpus document the spec stands on — a reference, not the text.
    struct Fact: Codable, Equatable {
        var corpus: String
        var source: String
    }

    /// The decision that ratified the spec, mirroring the shared `Decision` in the core.
    struct Record: Codable, Equatable {
        var source: String
        var version: Int
        var decidedAt: String
        var fingerprint: String

        enum CodingKeys: String, CodingKey {
            case source, version, fingerprint
            case decidedAt = "decided_at"
        }

        /// `filled, v2` — the record in the words a reader of the pane needs.
        var summary: String { "\(source), v\(version)" }
    }

    /// Whether the record still ratifies the spec beside it — the shared `DesignFreshness`, one
    /// level down. `absent` is the honest "nobody has decided this yet", not an error.
    enum Freshness: Decodable, Equatable {
        case absent
        case current
        case stale(recorded: String, current: String)

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            switch (try? c.decode(String.self, forKey: .state)) ?? "absent" {
            case "current":
                self = .current
            case "stale":
                self = .stale(
                    recorded: (try? c.decode(String.self, forKey: .recorded)) ?? "",
                    current: (try? c.decode(String.self, forKey: .current)) ?? ""
                )
            default:
                self = .absent
            }
        }

        private enum CodingKeys: String, CodingKey {
            case state, recorded, current
        }

        /// How the pane says it.
        var label: String {
            switch self {
            case .absent: return "not decided yet"
            case .current: return "current"
            case .stale: return "stale — the spec has changed since it was decided"
            }
        }
    }

    var name: String
    var kind: String
    var description: String
    var invariants: [String]
    var device: Device?
    var provides: [String]
    var facts: [Fact]

    enum CodingKeys: String, CodingKey {
        case name, kind, description, invariants, device, provides, facts
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = (try? c.decode(String.self, forKey: .name)) ?? ""
        kind = (try? c.decode(String.self, forKey: .kind)) ?? ""
        description = (try? c.decode(String.self, forKey: .description)) ?? ""
        invariants = (try? c.decode([String].self, forKey: .invariants)) ?? []
        device = try? c.decode(Device.self, forKey: .device)
        provides = (try? c.decode([String].self, forKey: .provides)) ?? []
        facts = (try? c.decode([Fact].self, forKey: .facts)) ?? []
    }

    /// Whether the spec says anything beyond the least the tree states — a description, a device, a
    /// role or a fact. A component that predates the graph has none of them, and the pane says so
    /// rather than showing four empty rows.
    var hasContract: Bool {
        !description.isEmpty || device != nil || !provides.isEmpty || !facts.isEmpty
    }
}

/// The whole answer to `project/componentSpec`: the spec, the file it is rendered to, and the
/// decision beside it.
///
/// `stored` is what tells "the spec that was decided" from "the least the tree states": a component
/// that predates this graph is *implied* from its own `CMakeLists.txt` and has no record yet.
///
/// `spec` is **null** for a component whose manifest states no kind: there is no contract to imply,
/// and the pane already says "states no kind" from the tree. That is an answer, not an error.
struct ComponentSpecReading: Decodable, Equatable {
    var spec: ComponentSpec?
    var yaml: String?
    var file: String?
    var stored: Bool
    var record: ComponentSpec.Record?
    var freshness: ComponentSpec.Freshness

    enum CodingKeys: String, CodingKey {
        case spec, yaml, file, stored, record, freshness
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        spec = try? c.decode(ComponentSpec.self, forKey: .spec)
        yaml = try? c.decode(String.self, forKey: .yaml)
        file = try? c.decode(String.self, forKey: .file)
        stored = (try? c.decode(Bool.self, forKey: .stored)) ?? false
        record = try? c.decode(ComponentSpec.Record.self, forKey: .record)
        freshness = (try? c.decode(ComponentSpec.Freshness.self, forKey: .freshness)) ?? .absent
    }
}
