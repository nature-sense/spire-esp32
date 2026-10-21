import Foundation

/// Mirrors the Rust LLM settings persisted in the memory graph.
struct LLMConfig: Codable {
    /// Base URL of an OpenAI-compatible chat-completions endpoint.
    ///
    /// Defaults to DeepSeek; point it at a local server to run offline, e.g.
    /// Rapid-MLX on the LAN: `http://10.0.10.131:8000/v1/chat/completions`.
    /// The core POSTs straight to this URL (it appends no path), so it must
    /// end in `/v1/chat/completions`.
    var apiUrl: String = "https://api.deepseek.com/v1/chat/completions"
    var apiKey: String = ""
    /// Base model used by the agent/tool loop (`Default` role) and as the
    /// fallback whenever a role-specific model is empty.
    var model: String = "deepseek-chat"
    var planningModel: String = "deepseek-chat"
    var codingModel: String = "deepseek-chat"
    /// Tavily web-search API key (used by the `search/web` tools).
    var tavilyApiKey: String = ""

    /// The model choices available in the settings UI.
    ///
    /// `deepseek-chat` is the DEFAULT for planning/coding: verified live
    /// (2026-08-17) that the reasoning models (`deepseek-v4-pro`/`-flash`/`r1`)
    /// return EMPTY `content` + `finish_reason="length"` (all tokens in
    /// `reasoning_content`) for Spire's structured-JSON planning prompts,
    /// which the parser cannot use. `deepseek-chat` returns the JSON in
    /// `content`.
    static let modelChoices = [
        "deepseek-chat",
        "deepseek-v4-pro",
        "deepseek-v4-flash",
        "deepseek-r1",
    ]

    enum CodingKeys: String, CodingKey {
        case apiUrl = "deepseek.api_url"
        case apiKey = "deepseek.api_key"
        case model = "deepseek.model"
        case planningModel = "deepseek.planning_model"
        case codingModel = "deepseek.coding_model"
        case tavilyApiKey = "tavily.api_key"
    }
}
