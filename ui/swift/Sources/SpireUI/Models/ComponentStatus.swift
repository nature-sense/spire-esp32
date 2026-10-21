import Foundation

/// **What state a component is in**, as `project/componentStatus` answers it — the two facts a
/// navigator row shows.
///
/// The read is one for the whole list because the alternative is one call per row: the same question
/// asked N times, answered at N instants. The rule each answer follows is the core's, not this
/// file's — `filled` is a spec that says more than the component's own `CMakeLists.txt`, `implied` is
/// one nobody has described yet, and `noKind` is a component whose manifest states no kind at all,
/// which are the same three cases `project/componentSpec` tells apart.
struct ComponentStatus: Decodable, Equatable, Identifiable {
    /// How much of the contract exists, as the core decided it.
    enum Fill: String, Decodable {
        case filled
        case implied
        /// `no_kind` on the wire: a component whose manifest states no kind, so there is nothing to
        /// imply and no door can read it back.
        case noKind = "no_kind"

        init(from decoder: Decoder) throws {
            let raw = (try? decoder.singleValueContainer().decode(String.self)) ?? ""
            // Lenient for the same reason `ComponentSpec` is: a state a newer core invents reads as
            // "nothing has been said yet", which is the conservative reading rather than a green dot.
            self = Fill(rawValue: raw) ?? .implied
        }
    }

    /// The newest recorded run of `idf_component_edit` for this component — the gate's verdict, not
    /// the code's. `nil` means there is no run at all, which is not the same as a run that failed.
    struct Gate: Decodable, Equatable {
        var success: Bool
        /// The files already said what was asked: nothing moved, and the request is in effect.
        var upToDate: Bool
        /// How far verification reached, already worded by the core (`HostOnly`, `WithTarget`).
        var verified: String
        /// What the optional second gate did, in the caller's terms.
        var chipBuild: String
        /// When the run happened (RFC 3339), shown as the core stamped it.
        var at: String

        enum CodingKeys: String, CodingKey {
            case success, verified, at
            case upToDate = "up_to_date"
            case chipBuild = "chip_build"
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            success = (try? c.decode(Bool.self, forKey: .success)) ?? false
            upToDate = (try? c.decode(Bool.self, forKey: .upToDate)) ?? false
            verified = (try? c.decode(String.self, forKey: .verified)) ?? ""
            chipBuild = (try? c.decode(String.self, forKey: .chipBuild)) ?? ""
            at = (try? c.decode(String.self, forKey: .at)) ?? ""
        }

        /// `HostOnly` and `WithTarget` are the two words the core writes; anything else is passed
        /// through as it stands rather than guessed at.
        var verificationWord: String {
            switch verified {
            case "HostOnly": return "host test only"
            case "WithTarget": return "host test and the chip build"
            default: return verified.isEmpty ? "checked" : verified
            }
        }
    }

    /// The component's directory name — the identity the analyzer gives its row.
    var name: String
    /// What its manifest says, or the kind its stored spec states for a silent manifest.
    var kind: String?
    var fill: Fill
    var freshness: ComponentSpec.Freshness
    var gate: Gate?

    /// The row identity: a component's name is unique within its library, which is the scope this
    /// read is made in.
    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, kind, fill, freshness, gate
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = (try? c.decode(String.self, forKey: .name)) ?? ""
        kind = try? c.decode(String.self, forKey: .kind)
        fill = (try? c.decode(Fill.self, forKey: .fill)) ?? .implied
        freshness = (try? c.decode(ComponentSpec.Freshness.self, forKey: .freshness)) ?? .absent
        gate = try? c.decode(Gate.self, forKey: .gate)
    }
}

extension ComponentStatus {
    /// The contract dot in one sentence — what exists, and whether the record still ratifies it.
    ///
    /// A `stale` spec is *filled* and not current: both are true, and saying only one of them would be
    /// a half-answer. `absent` for a filled spec is the odd one — a stored spec with no record — and it
    /// is said plainly rather than folded into "current".
    var specSummary: String {
        switch fill {
        case .noKind:
            return "\(name) states no kind in its CMakeLists.txt, so there is no spec to read: nothing "
                 + "can be filled in until it says what it is."
        case .implied:
            return "\(name) is implied from its own CMakeLists.txt: no spec has been decided for it, "
                 + "and its first edit is what decides one."
        case .filled:
            switch freshness {
            case .current:
                return "\(name) is filled: a spec was decided for it, and the record still ratifies it."
            case .stale:
                return "\(name) is filled, but the decision is stale: the spec has changed since it "
                     + "was decided."
            case .absent:
                return "\(name) has a stored spec, but no record decided it."
            }
        }
    }

    /// The gate dot in one sentence: what the newest run decided, and what was checked.
    ///
    /// A refusal is not the code's state — the files were put back — so it is worded as the run's
    /// outcome, which is the thing a reader needs before asking for the same change again.
    var gateSummary: String {
        guard let gate else {
            return "\(name) has never been written: no edit run is recorded for it."
        }
        let checked: String
        if gate.chipBuild.isEmpty {
            checked = "verified with the \(gate.verificationWord)"
        } else {
            checked = "verified with the \(gate.verificationWord) · \(gate.chipBuild)"
        }
        let when = gate.at.isEmpty ? "" : " · \(gate.at)"
        if !gate.success {
            return "\(name)'s last run was refused and reverted — \(checked)\(when)."
        }
        if gate.upToDate {
            return "\(name)'s last run was already true: the files said what was asked — \(checked)\(when)."
        }
        return "\(name)'s last run was kept — \(checked)\(when)."
    }
}
