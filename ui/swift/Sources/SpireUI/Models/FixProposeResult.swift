import Foundation

/// Proposal result from the coordinator's whole-file compile-fix flow
/// (`build/fixPropose`): the LLM's rewritten file, for review before writing.
struct FixProposeResult: Codable {
    var status: String   // "proposed" | "clean" | "error"
    var path: String?
    var proposedContent: String?
    var error: String?

    enum CodingKeys: String, CodingKey {
        case status, path
        case proposedContent = "proposed_content"
        case error
    }
}
