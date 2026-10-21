import Testing
import Foundation
@testable import SpireUI

/// A `UIBackend` that answers from a script instead of the dylib: `config/getAll` returns a canned
/// payload and every `config/set` is recorded, so the bridge's read and write paths can be checked
/// without starting the Rust actor system.
private final class RecordingBackend: UIBackend, @unchecked Sendable {
    var isAvailable: Bool { true }

    /// The payload `config/getAll` answers with.
    var getAllPayload: String = "{}"
    /// Every `config/set` the bridge sent, in order.
    var setCalls: [(key: String, value: String)] = []

    func send(_ data: Data) async throws -> Data {
        let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        let method = object?["method"] as? String
        if method == "config/set",
           let params = object?["params"] as? [String: Any],
           let key = params["key"] as? String,
           let value = params["value"] as? String {
            setCalls.append((key, value))
        }
        if method == "config/getAll" {
            return Data(getAllPayload.utf8)
        }
        return Data(#"{"success":true}"#.utf8)
    }

    func waitForEvent(timeoutMs: UInt32) -> String? { nil }
    func drainBuildEvents() -> String? { nil }
    func waitForBuildEvent(timeoutMs: UInt32) -> String? { nil }
}

/// **The settings sheet's two directions.** The sheet shows `llmConfig` and saves it back as six
/// `config/set` calls. The base URL and base model are the two fields that decide whether the core
/// talks to any server at all — and the base model is the one the agent loop uses, separate from
/// planning/coding — so both are pinned here against the exact keys the core reads.
@Test("fetchLlmConfig reads the base URL and base model")
@MainActor
func fetchLlmConfigReadsEndpointAndModel() async throws {
    let backend = RecordingBackend()
    backend.getAllPayload = """
    {"config":{
      "deepseek.api_url":"http://10.0.10.131:8000/v1/chat/completions",
      "deepseek.api_key":"local-rapid-mlx",
      "deepseek.model":"qwen3.8-27b-4bit",
      "deepseek.planning_model":"qwen3.8-27b-4bit",
      "deepseek.coding_model":"qwen3.8-27b-4bit",
      "tavily.api_key":"tvly-test"
    }}
    """
    let bridge = SpireBridge(backend: backend)

    await bridge.fetchLlmConfig()

    #expect(bridge.llmConfig.apiUrl == "http://10.0.10.131:8000/v1/chat/completions")
    #expect(bridge.llmConfig.model == "qwen3.8-27b-4bit")
    #expect(bridge.llmConfig.planningModel == "qwen3.8-27b-4bit")
    #expect(bridge.llmConfig.codingModel == "qwen3.8-27b-4bit")
    #expect(bridge.llmConfig.apiKey == "local-rapid-mlx")
    #expect(bridge.llmConfig.tavilyApiKey == "tvly-test")
}

/// **All six keys, not five.** A save that silently skips the base model would leave the agent loop
/// on whatever was there before, so both the count and each value are asserted.
@Test("saveLlmConfig writes all six config keys")
@MainActor
func saveLlmConfigWritesAllKeys() async throws {
    let backend = RecordingBackend()
    let bridge = SpireBridge(backend: backend)
    let config = LLMConfig(
        apiUrl: "http://10.0.10.131:8000/v1/chat/completions",
        apiKey: "local-rapid-mlx",
        model: "qwen3.8-27b-4bit",
        planningModel: "qwen3.8-27b-4bit",
        codingModel: "qwen3.8-27b-4bit",
        tavilyApiKey: "tvly-test"
    )

    let saved = await bridge.saveLlmConfig(config)

    #expect(saved)
    #expect(backend.setCalls.count == 6)
    let byKey = Dictionary(uniqueKeysWithValues: backend.setCalls.map { ($0.key, $0.value) })
    #expect(byKey["deepseek.api_url"] == "http://10.0.10.131:8000/v1/chat/completions")
    #expect(byKey["deepseek.api_key"] == "local-rapid-mlx")
    #expect(byKey["deepseek.model"] == "qwen3.8-27b-4bit")
    #expect(byKey["deepseek.planning_model"] == "qwen3.8-27b-4bit")
    #expect(byKey["deepseek.coding_model"] == "qwen3.8-27b-4bit")
    #expect(byKey["tavily.api_key"] == "tvly-test")
}

/// The wire names are the contract between Swift's properties and the core's `deepseek.*`
/// namespace; a rename on either side would decode to defaults rather than fail, so the emitted
/// keys are asserted directly rather than inferred from a round trip succeeding.
@Test("LLMConfig round-trips through the deepseek.* wire keys")
func llmConfigCodableRoundTrip() throws {
    let config = LLMConfig(
        apiUrl: "http://10.0.10.131:8000/v1/chat/completions",
        apiKey: "local-rapid-mlx",
        model: "qwen3.8-27b-4bit",
        planningModel: "qwen3.8-27b-4bit",
        codingModel: "qwen3.8-27b-4bit",
        tavilyApiKey: "tvly-test"
    )

    let data = try JSONEncoder().encode(config)
    let object = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
    #expect(object["deepseek.api_url"] as? String == "http://10.0.10.131:8000/v1/chat/completions")
    #expect(object["deepseek.model"] as? String == "qwen3.8-27b-4bit")

    let decoded = try JSONDecoder().decode(LLMConfig.self, from: data)
    #expect(decoded.apiUrl == config.apiUrl)
    #expect(decoded.model == config.model)
    #expect(decoded.codingModel == config.codingModel)
}
