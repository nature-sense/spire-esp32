import SwiftUI

/// Modal sheet for configuring LLM settings (API key + planning/coding models).
struct LLMSettingsView: View {
    @Environment(SpireBridge.self) private var bridge
    @Environment(\.dismiss) private var dismiss

    @State private var apiUrl = "https://api.deepseek.com/v1/chat/completions"
    @State private var apiKey = ""
    @State private var planningModel = "deepseek-chat"
    @State private var codingModel = "deepseek-chat"
    @State private var baseModel = "deepseek-chat"
    @State private var tavilyApiKey = ""
    @State private var saving = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Header
            HStack {
                Image(systemName: "key.horizontal")
                Text("LLM Settings").font(.title2.bold())
            }

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    // Endpoint — the core POSTs straight to this URL (it does
                    // not append a path), so it must include the full
                    // `/v1/chat/completions` suffix.
                    labeled("API Base URL") {
                        TextField(
                            "https://api.deepseek.com/v1/chat/completions",
                            text: $apiUrl
                        )
                        .textFieldStyle(.roundedBorder)
                        Text("Any OpenAI-compatible endpoint, e.g. http://10.0.10.131:8000/v1/chat/completions")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    // API key — DeepSeek's real key, or any non-empty
                    // placeholder for a local server (the core rejects an
                    // empty key before it ever dials out).
                    labeled("API Key") {
                        SecureField("sk-... (any non-empty value works for a local server)", text: $apiKey)
                            .textFieldStyle(.roundedBorder)
                    }

                    // Models are free text: any name the endpoint serves.
                    modelField("Planning Model", $planningModel)
                    modelField("Coding Model", $codingModel)
                    modelField("Base Model (agent/tools)", $baseModel)

                    // Tavily web-search API key (for the search/web tools)
                    labeled("Tavily API Key (web search)") {
                        SecureField("tvly-...", text: $tavilyApiKey)
                            .textFieldStyle(.roundedBorder)
                    }
                }
            }

            Divider()

            // Buttons
            HStack {
                Button("Cancel") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button(saving ? "Saving…" : "Save") {
                    save()
                }
                .buttonStyle(.borderedProminent)
                .disabled(saving || apiKey.isEmpty)
            }
        }
        .padding(24)
        .frame(width: 540, height: 620)
        .onAppear { load() }
    }

    /// A titled field group, optionally followed by caption text.
    @ViewBuilder
    private func labeled<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.headline)
            content()
        }
    }

    /// Free-text model entry with a preset menu for the known DeepSeek names.
    private func modelField(_ title: String, _ binding: Binding<String>) -> some View {
        labeled(title) {
            HStack {
                TextField("model-name", text: binding)
                    .textFieldStyle(.roundedBorder)
                Menu("Presets") {
                    ForEach(LLMConfig.modelChoices, id: \.self) { model in
                        Button(model) { binding.wrappedValue = model }
                    }
                }
                .fixedSize()
            }
        }
    }

    private func load() {
        apiUrl = bridge.llmConfig.apiUrl
        apiKey = bridge.llmConfig.apiKey
        planningModel = bridge.llmConfig.planningModel
        codingModel = bridge.llmConfig.codingModel
        baseModel = bridge.llmConfig.model
        tavilyApiKey = bridge.llmConfig.tavilyApiKey
    }

    private func save() {
        saving = true
        Task {
            let ok = await bridge.saveLlmConfig(LLMConfig(
                apiUrl: apiUrl,
                apiKey: apiKey,
                model: baseModel,
                planningModel: planningModel,
                codingModel: codingModel,
                tavilyApiKey: tavilyApiKey
            ))
            saving = false
            if ok { dismiss() }
        }
    }
}