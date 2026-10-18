// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! BuildManagerActor — neutral entry point to the build system.
//!
//! Maintains a router (config filename → module sender) that it builds by
//! querying each static build module's `DescribeCapabilities`. It is the
//! single point of access for all build/analysis actions:
//!
//! - `AnalyzeProject` routes to a module, produces `BuildMetadata`, and
//!   persists it in the knowledge graph (the actor's state).
//! - `BuildProject`/`TestProject` fetch the stored analysis from the graph
//!   and pass it to the module in the request message (modules stay stateless).
//! - `GetAnalysis` reads the graph without touching any module.

use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::{mpsc, oneshot};

use crate::build::{
    AstParseResult, BuildModuleMessage, BuildOptions, BuildOutput, ModuleCapability, ParseSummary,
    TestOptions,
};
use spire_core::actors::Actor;
use spire_core::models::memory_graph::{
    AttrNode, NodeUpdate, RelationshipInput, RelationshipType, StreamOp, StreamOpResult,
    TransactionRequest,
};
use spire_core::subsystems::graph::memory_graph::MemoryGraphMessage;

/// Build an envelope `AttrNode` for a node stored through a transaction stream
/// (diagnostics, source files, and AST nodes are all open-model node kinds).
fn bm_attr_unknown(
    node_type: &str,
    subtype: Option<String>,
    name: String,
    description: Option<String>,
    properties: HashMap<String, serde_json::Value>,
) -> AttrNode {
    let now = chrono::Utc::now();
    AttrNode {
        id: uuid::Uuid::new_v4().to_string(),
        node_type: node_type.to_string(),
        subtype,
        name,
        description,
        properties,
        embedding_id: None,
        created_at: now,
        updated_at: now,
        version: 1,
    }
}

use spire_core::build_types::BuildMetadata;
use uuid::Uuid;

/// A materialized scaffold's structural contract (returned by
/// `ProjectCreationMessage::ScaffoldProject`). It tells the fill phase (LLM)
/// exactly which paths are locked (structural), which source roots it may
/// write files + create subdirs under, which build-config dependency sections
/// may be edited ONLY through `declare_dependencies`, and the target platforms.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ScaffoldSpec {
    /// Paths (relative to project root) that are structural/locked — the LLM
    /// may NOT write, rename, or delete these.
    pub structural_files: Vec<String>,
    /// Source roots the LLM may add/modify files under (and create
    /// subdirectories beneath them).
    pub fill_roots: Vec<String>,
    /// Build-config dependency sections editable ONLY via the module's
    /// `declare_dependencies` tool.
    pub dependency_sections: Vec<String>,
    /// Registry ids the scaffold targets.
    pub platform_targets: Vec<String>,
    /// The module's build-system label (e.g. "Cargo", "Meson") — used to route
    /// `declare_dependencies` via `CallModuleTool`.
    pub build_system: String,
    /// The emitted scaffold files (build configs + source stubs) for the UI's
    /// structure preview. Ignored by the fill-phase guard (the guard uses
    /// `structural_files`/`fill_roots`).
    #[serde(default)]
    pub files: Vec<crate::build::ScaffoldFile>,
    /// Structural shape the scaffold was emitted for (SingleSource / Native).
    /// The fill guard uses `fill_roots` regardless.
    #[serde(default, skip_serializing_if = "is_native_structure")]
    pub structure: spire_core::build_types::ProjectStructure,
    /// True when the project is embedded (cross-compiled targets only — never
    /// a host build target). Wiring: the wizard sets this for embedded projects.
    #[serde(default)]
    pub embedded: bool,
    /// **A message for the person, when this scaffold did not use the design it was handed.**
    ///
    /// `Some` when the tree already stated its own design in `composition.spire`, so the
    /// decomposition the caller passed in was dropped — the tree's wins, because a design changes
    /// by editing that file (`idf_projects::design_for_scaffold`). The wizard shows this verbatim:
    /// the rule has to be *said*, or a person reviews one composition and is handed a project
    /// built from another. `None` in the ordinary case, and absent from the wire when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_warning: Option<String>,
}

fn is_native_structure(s: &spire_core::build_types::ProjectStructure) -> bool {
    *s == spire_core::build_types::ProjectStructure::Native
}

/// Messages for the BuildManager actor.
pub enum BuildManagerMessage {
    /// Register a module's router entries from its capability.
    AddModule {
        capability: ModuleCapability,
        module_tx: mpsc::Sender<BuildModuleMessage>,
    },
    /// Register a module as the handler for a **platform `os`** (e.g. `"esp-idf"`), in
    /// preference to whichever module owns the config file.
    ///
    /// Separate from `AddModule` on purpose. The config router assumes one config file maps to
    /// exactly one module, and that breaks for a platform-specific build system: an ESP32
    /// project is *also* a `Cargo.toml` project, so a module claiming that file would silently
    /// **replace** the cargo module and send every Rust project in Spire down the ESP32 path.
    /// Declaring the platform routes on what actually differs instead.
    ///
    /// A module registered this way claims **no** config files.
    AddPlatformModule {
        os: String,
        /// What the module declared it can do — carried here because it decides whether a
        /// `flash` request is routed to it or refused (see `PlatformModule`).
        capability: ModuleCapability,
        module_tx: mpsc::Sender<BuildModuleMessage>,
    },
    /// Analyze a project → produce + store BuildMetadata.
    AnalyzeProject {
        path: PathBuf,
        /// The specific config file to analyze (e.g. "Cargo.toml") when the
        /// directory holds several build configs (e.g. a Cargo workspace root
        /// that also carries a Makefile). `None` → the manager detects a single
        /// config in the directory (see `find_config_file`).
        config_file: Option<String>,
        reply_to: oneshot::Sender<Result<BuildMetadata, String>>,
    },
    /// Build a project using stored analysis.
    BuildProject {
        path: PathBuf,
        opts: BuildOptions,
        reply_to: oneshot::Sender<Result<BuildOutput, String>>,
    },
    /// Test a project using stored analysis.
    TestProject {
        path: PathBuf,
        opts: TestOptions,
        reply_to: oneshot::Sender<Result<BuildOutput, String>>,
    },
    /// Get stored analysis without touching a module.
    GetAnalysis {
        path: String,
        reply_to: oneshot::Sender<Option<BuildMetadata>>,
    },
    /// List all registered modules and their capabilities.
    ListModules {
        reply_to: oneshot::Sender<Vec<ModuleCapability>>,
    },
    /// LLM tool invocation: routes build/* tools to the appropriate handler.
    CallTool {
        tool_name: String,
        args: serde_json::Value,
        reply_to: oneshot::Sender<serde_json::Value>,
    },
    /// Generic module tool invocation by build-system label (e.g. "Cargo").
    /// Used by the legacy `project_*` actors so ALL dispatch flows through
    /// the registered build modules — no hardcoded string branches.
    CallModuleTool {
        build_system: String,
        tool_name: String,
        args: serde_json::Value,
        reply_to: oneshot::Sender<Result<serde_json::Value, String>>,
    },
    /// List this manager's unified build tools (actor-based discovery).
    ListTools {
        reply_to: oneshot::Sender<Vec<spire_core::actors::ToolInfo>>,
    },
    /// Parse a source file (routed by extension) and persist the AST into the
    /// graph via a transaction stream. Single-writer rule: only this manager
    /// writes AST nodes/edges.
    ParseAndStoreSourceFile {
        file_path: PathBuf,
        reply_to: oneshot::Sender<Result<ParseSummary, String>>,
    },
    ScaffoldBuildConfig {
        project_name: String,
        goal: String,
        build_file: String,
        /// Optional cross-platform targets (registry ids, e.g. `["rpi5"]`).
        /// Passed through to the build module's scaffold.
        platforms: Vec<String>,
        /// `"native" | "single_source" | "hal"` — forwarded to the build
        /// module's `scaffold_layout`. Defaults to Native for legacy callers.
        structure: Option<spire_core::build_types::ProjectStructure>,
        /// True for embedded projects (cross-compiled targets only — no host).
        embedded: bool,
        /// The **component library** an ESP-IDF application is built against, forwarded to the
        /// build module's scaffold. `None` for every other structure.
        library: Option<String>,
        /// The **decomposition** an ESP-IDF application was designed from, forwarded to the build
        /// module's scaffold, which states its framework in the application's `CMakeLists.txt` and
        /// writes the spec itself as `SPIRE.application.json`. `None` for every other structure, and
        /// for an application whose design has not run.
        application: Option<crate::build::application_spec::ApplicationSpec>,
        reply_to: oneshot::Sender<Result<crate::build::ScaffoldOutput, String>>,
    },
    /// Attach the UI broadcast sender for streaming build events.
    SetEventTx {
        event_tx: tokio::sync::broadcast::Sender<String>,
    },
    /// Attach the actor-owned build-event log (drain-safe while a streaming
    /// build/lint/fix occupies this actor's mailbox).
    SetEventLog {
        event_log_tx: mpsc::Sender<BuildEventLogMessage>,
    },
}

/// The BuildManager actor — routes build/analysis requests to static modules.
pub struct BuildManagerActor {
    /// Router: config filename → module sender.
    router: HashMap<String, mpsc::Sender<BuildModuleMessage>>,
    /// Router: source file extension → module sender (for AST parsing).
    extension_router: HashMap<String, mpsc::Sender<BuildModuleMessage>>,
    /// Router: platform `os` → module sender, for builds whose selected platform names one.
    ///
    /// Consulted *before* [`Self::router`]: a platform-specific module wins over the module
    /// that owns the config file, because the platform is what the invocation actually
    /// depends on (an ESP32 build is still a `Cargo.toml` project).
    ///
    /// The module's capability is stored with its sender: it is what gates the one operation
    /// only a platform module can perform (`flash`), and a missing capability would have to be
    /// re-asked for on every request.
    platform_router: HashMap<String, PlatformModule>,
    /// Capabilities of all registered modules.
    capabilities: Vec<ModuleCapability>,
    /// Sender to the MemoryGraph actor for analysis state.
    memory_graph_tx: mpsc::Sender<MemoryGraphMessage>,
    /// Optional broadcast sender for pushing BuildEvents to the UI event stream.
    event_tx: Option<tokio::sync::broadcast::Sender<String>>,
    /// Actor-owned build-event log sender (set via `SetEventLog`). Streaming
    /// forwarders push incremental lines to this log actor instead of a shared
    /// `Arc<Mutex<Vec>>`, so the FFI can drain while builds/lints/fixes occupy
    /// this actor's mailbox.
    event_log_tx: Option<mpsc::Sender<BuildEventLogMessage>>,
}

/// Which registered module should handle a build.
///
/// The decision is its own type so it can be asserted in a test and named in a log, rather
/// than being an implicit `.or_else()` a reader has to infer.
#[derive(Debug, PartialEq, Eq)]
enum BuildRoute {
    /// A module that declared itself the handler for this platform `os`.
    Platform(String),
    /// The module that owns the config file — the pre-existing behaviour.
    Config(String),
}

/// A module registered by the platform `os` it serves, with the capability that gates it.
///
/// One value rather than two maps: a platform module is only usable together with what it
/// declared it can do, so they are registered and looked up as a unit.
struct PlatformModule {
    capability: ModuleCapability,
    module_tx: mpsc::Sender<BuildModuleMessage>,
}

/// The `os` of a registered platform, or `None` when the id is unknown.
///
/// A free function so the routing decision does not depend on the actor, which is what makes
/// `route_for` testable against a temporary platform directory.
fn platform_os_of(platform_id: &str) -> Option<String> {
    crate::platform::Platform::from_registry(platform_id).map(|p| p.os)
}

impl BuildManagerActor {
    pub fn new(memory_graph_tx: mpsc::Sender<MemoryGraphMessage>) -> Self {
        Self {
            router: HashMap::new(),
            extension_router: HashMap::new(),
            platform_router: HashMap::new(),
            capabilities: Vec::new(),
            memory_graph_tx,
            event_tx: None,
            event_log_tx: None,
        }
    }

    /// Attach the actor-owned build-event log.
    pub fn set_event_log(&mut self, event_log_tx: mpsc::Sender<BuildEventLogMessage>) {
        self.event_log_tx = Some(event_log_tx);
    }

    /// Attach the UI event broadcast sender so streaming build events can be pushed.
    pub fn set_event_tx(&mut self, event_tx: tokio::sync::broadcast::Sender<String>) {
        self.event_tx = Some(event_tx);
    }

    /// Register a module. Each config file maps to the module's sender, and
    /// each source extension maps to the module's sender for AST parsing.
    fn add_module(
        &mut self,
        capability: ModuleCapability,
        module_tx: mpsc::Sender<BuildModuleMessage>,
    ) {
        tracing::info!(
            "BuildManager: registering module '{}' with configs {:?}, extensions {:?}",
            capability.name,
            capability.config_files,
            capability.source_extensions
        );
        for config in &capability.config_files {
            self.router.insert(config.clone(), module_tx.clone());
        }
        for ext in &capability.source_extensions {
            self.extension_router.insert(ext.clone(), module_tx.clone());
        }
        self.capabilities.push(capability);
        tracing::info!(
            "BuildManager: router now has {} config files: {:?}",
            self.router.len(),
            self.router.keys().collect::<Vec<_>>()
        );
    }

    /// Register a platform-specific module (see `AddPlatformModule`).
    fn add_platform_module(
        &mut self,
        os: String,
        capability: ModuleCapability,
        module_tx: mpsc::Sender<BuildModuleMessage>,
    ) {
        tracing::info!(
            "BuildManager: module '{}' registered for platform os '{}'",
            capability.name,
            os
        );
        // A platform module can also own a config file, and for one operation it *must*: a scaffold
        // request is routed by config file (`scaffold_build_config` → `module_tx_for(&config,
        // None)`), with no platform to route by. Without this, a module can be registered, describe
        // itself, answer `Build` and `Flash` — and be unreachable for the one thing that creates a
        // project of its kind.
        //
        // A file that already has an owner keeps it: the config modules register first (or not, in
        // `ffi.rs`, where this one is registered before `node` and `cmake`), and whoever owns a file
        // is the module that *analyses* it. A platform module shadowing that would take analysis
        // away from a module that implements it, to hand it to one declared not to.
        for config in &capability.config_files {
            match self.router.get(config) {
                None => {
                    self.router.insert(config.clone(), module_tx.clone());
                }
                Some(_) => tracing::warn!(
                    "BuildManager: module '{}' also claims '{}', which another module already \
                     owns; the file keeps its owner, and '{}' is reachable by its platform only",
                    capability.name,
                    config,
                    capability.name
                ),
            }
        }
        self.platform_router.insert(
            os,
            PlatformModule {
                capability,
                module_tx,
            },
        );
    }

    /// Which registered module should handle a build of `config` under `platform`.
    ///
    /// The decision is its own type so it can be asserted in a test and named in a log,
    /// rather than being an implicit `.or_else()` someone has to infer.
    fn route_for(&self, config: &str, platform: Option<&str>) -> BuildRoute {
        if let Some(os) = platform.and_then(platform_os_of) {
            if self.platform_router.contains_key(&os) {
                return BuildRoute::Platform(os);
            }
        }
        BuildRoute::Config(config.to_string())
    }

    fn sender_for(&self, route: &BuildRoute) -> Option<&mpsc::Sender<BuildModuleMessage>> {
        match route {
            BuildRoute::Platform(os) => self.platform_router.get(os).map(|m| &m.module_tx),
            BuildRoute::Config(config) => self.router.get(config),
        }
    }

    /// The module a build-like request for `config` should go to, honouring platform routing.
    ///
    /// **The single decision point.** Nine call sites used to repeat this lookup and its error
    /// message by hand, which is how a routing change ends up applied to eight of them — and
    /// the shadowing bug this replaced was exactly a site that had not been updated.
    ///
    /// `platform` is `None` on the paths that have not been threaded yet: they keep the config
    /// router's behaviour exactly as it was, and upgrading one is now a one-line change here
    /// rather than an edit at every call site.
    fn module_tx_for(
        &self,
        config: &str,
        platform: Option<&str>,
    ) -> Result<&mpsc::Sender<BuildModuleMessage>, String> {
        let route = self.route_for(config, platform);
        self.sender_for(&route).ok_or_else(|| match &route {
            BuildRoute::Platform(os) => format!("No module registered for platform '{}'", os),
            BuildRoute::Config(config) => format!("No module registered for {}", config),
        })
    }

    /// The options a build should actually use, with the chip read back from the tree when the
    /// caller named no platform.
    ///
    /// The analysis already records the chip as a **platform target**
    /// ([`CmakeBuildModule`](crate::build::cmake::CmakeBuildModule) via
    /// [`declared_idf_target`](crate::build::idf_projects::declared_idf_target)), and the dashboard
    /// threads it from there — so this is the **backstop**, for a request that reached `build_build`
    /// without one (a tool call made outside the UI). Without it, routing by config file alone
    /// sends such a request to the cmake module — `cmake --build` on an IDF tree, which is the
    /// wrong invocation — while the tree itself says which chip it is for (`sdkconfig`'s
    /// `CONFIG_IDF_TARGET`, or the composition's `board.chip`).
    ///
    /// Only an id the registry can resolve **and** a registered module answers for is adopted:
    /// an `IDF_TARGET` no module serves must leave the route exactly as it was, rather than turn
    /// a working cmake build into "No module registered for platform".
    fn opts_with_declared_platform(&self, path: &Path, opts: &BuildOptions) -> BuildOptions {
        let names_platform = opts
            .platform
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty());
        if names_platform {
            return opts.clone();
        }
        let Some(chip) = crate::build::idf_projects::declared_idf_target(path) else {
            return opts.clone();
        };
        let answered =
            platform_os_of(&chip).is_some_and(|os| self.platform_router.contains_key(&os));
        if !answered {
            return opts.clone();
        }
        BuildOptions {
            platform: Some(chip),
            ..opts.clone()
        }
    }

    /// The route a `flash` request takes, refused unless the module that would handle it
    /// declares `supports_flash`.
    ///
    /// Flash is the one operation only a *platform* module can perform — the chip and the USB
    /// tool are platform facts, and no config file implies them. So a project whose platform
    /// routes to its config file's owner is refused here by name (a Raspberry Pi is still
    /// Cargo), rather than handed to a module whose answer would arrive as a lost channel.
    fn route_for_flash(&self, config: &str, platform: &str) -> Result<BuildRoute, String> {
        let route = self.route_for(config, Some(platform));
        match &route {
            BuildRoute::Platform(os) => {
                let module = self
                    .platform_router
                    .get(os)
                    .ok_or_else(|| format!("No module registered for platform '{}'", os))?;
                if !module.capability.supports_flash {
                    return Err(format!(
                        "flash is not supported for build system {}",
                        module.capability.build_system
                    ));
                }
                Ok(route)
            }
            // No platform module claimed this platform's `os`, so nothing here flashes. Naming
            // the `os` is what tells the user whether the id was wrong or the board simply has
            // no flash step.
            BuildRoute::Config(_) => Err(match platform_os_of(platform) {
                Some(os) => format!(
                    "flash is not supported for platform '{}' (no module flashes os '{}')",
                    platform, os
                ),
                None => format!("unknown platform '{}'", platform),
            }),
        }
    }

    /// Find which registered config file applies to a path.
    /// For a file path, use its basename. For a directory, look for a known
    /// config file inside it.
    /// Deterministic config-file priority so a directory holding several
    /// build configs (e.g. a Cargo workspace root that also carries a
    /// Makefile wrapper) resolves to the same primary system every time.
    const CONFIG_PRIORITY: &[&str] = &[
        "Cargo.toml",
        "Package.swift",
        "meson.build",
        "package.json",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
        "CMakeLists.txt",
        "Makefile",
        "go.mod",
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
    ];

    fn find_config_file(&self, path: &Path) -> Option<String> {
        if path.is_file() {
            let name = path.file_name()?.to_str()?.to_string();
            return self.router.contains_key(&name).then_some(name);
        }
        // Deterministic priority first…
        for config in Self::CONFIG_PRIORITY {
            if self.router.contains_key(*config) && path.join(config).exists() {
                return Some(config.to_string());
            }
        }
        // …then any remaining registered config in stable sorted order.
        let mut keys: Vec<&String> = self.router.keys().collect();
        keys.sort();
        for config in keys {
            if path.join(config).exists() {
                return Some(config.clone());
            }
        }
        None
    }

    /// Persist analysis metadata in the graph keyed by the path.
    async fn store_analysis(&self, path: &str, metadata: &BuildMetadata) -> Result<(), String> {
        let key = format!("build.analysis.{}", path);
        let value = serde_json::to_value(metadata)
            .map_err(|e| format!("Failed to serialize BuildMetadata: {e}"))?;
        let (tx, rx) = oneshot::channel();
        self.memory_graph_tx
            .send(MemoryGraphMessage::SetConfig {
                key,
                value,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("MemoryGraph channel closed: {e}"))?;
        rx.await
            .map_err(|e| format!("MemoryGraph response lost: {e}"))?
            .map_err(|e| format!("MemoryGraph store failed: {e}"))?;
        Ok(())
    }

    /// Fetch stored analysis metadata from the graph by path key.
    async fn get_analysis(&self, path: &str) -> Option<BuildMetadata> {
        let key = format!("build.analysis.{}", path);
        let (tx, rx) = oneshot::channel();
        let _ = self
            .memory_graph_tx
            .send(MemoryGraphMessage::GetConfig { key, reply_to: tx })
            .await;
        match rx.await {
            Ok(Ok(Some(value))) => serde_json::from_value(value).ok(),
            _ => None,
        }
    }

    /// The analysis a build needs: the stored one, else a fresh one.
    ///
    /// The store is **best-effort** — `store_analysis` only warns on a persistence failure, because
    /// losing the analyser's work would be worse than losing the cache — so "I analysed" is not
    /// something a caller can rely on: an `analyze_project` can succeed and the lookup still miss.
    /// Every build path went through `get_analysis(..).ok_or_else("run AnalyzeProject first")`, which
    /// turned that into the most confusing failure available: the analyse call reports success, and
    /// the build immediately asks the caller to analyse first.
    ///
    /// So the metadata `analyze_project` *returns* is used directly when the store is empty, rather
    /// than being discarded and then looked for. The only case that still fails is a project nothing
    /// can analyse, and then the message says that.
    async fn analysis_for(&self, path: &Path) -> Result<BuildMetadata, String> {
        let path_str = path.to_string_lossy().to_string();
        if let Some(metadata) = self.get_analysis(&path_str).await {
            return Ok(metadata);
        }
        self.analyze_project(path, None).await.map_err(|e| {
            format!("no stored analysis for {path_str}, and it could not be analysed: {e}")
        })
    }

    /// Send a single operation through a transaction stream and await its result.
    async fn send_stream_op(
        stream: &mpsc::Sender<TransactionRequest>,
        op: StreamOp,
    ) -> Result<StreamOpResult, String> {
        let (tx, rx) = oneshot::channel();
        stream
            .send(TransactionRequest {
                operation: op,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Transaction stream closed: {e}"))?;
        rx.await
            .map_err(|e| format!("Transaction stream response lost: {e}"))?
            .map_err(|e| format!("Stream op failed: {e}"))
    }

    /// Find an existing SourceFile node by its file path.
    async fn find_source_file(
        &self,
        path: &str,
    ) -> Option<spire_core::models::memory_graph::AttrNode> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .memory_graph_tx
            .send(MemoryGraphMessage::QueryAttrNodes {
                node_type: Some("SourceFile".to_string()),
                subtype: None,
                name: None,
                limit: Some(100),
                reply_to: tx,
            })
            .await;
        match rx.await {
            Ok(Ok(nodes)) => nodes
                .into_iter()
                .find(|n| n.name() == path || n.get("path").and_then(|v| v.as_str()) == Some(path)),
            _ => None,
        }
    }

    /// The scope root diagnostics are recorded under: the nearest ancestor of
    /// `path` that looks like a project (`.git` / `.spire`), else `path` itself.
    ///
    /// The UI passes a SUBPROJECT path to Build/Lint/Verify, while diagnostics are
    /// stored with absolute file paths under the project root — so matching the
    /// raw argument would leave the previous run's diagnostics behind and the
    /// Build tab would keep showing errors that no longer reproduce.
    fn diagnostics_scope_root(path: &Path) -> PathBuf {
        let mut dir = path.to_path_buf();
        loop {
            if dir.join(".git").exists() || dir.join(".spire").exists() {
                return dir;
            }
            if !dir.pop() {
                return path.to_path_buf();
            }
        }
    }

    /// Delete the previous run's Diagnostic nodes of `build_type` for
    /// `project_root`, inside the caller's open transaction stream.
    ///
    /// Diagnostics are merged by a stable `file:line:message` name, so a finding
    /// that no longer reproduces would otherwise stay in the graph forever and
    /// keep showing up in the Build tab. That is exactly how the bogus
    /// cross-header errors from pre-fix lint runs outlived the fix (the panel
    /// reads `project/diagnostics`, which has no "latest run" notion of its own).
    ///
    /// Membership is decided by `diagnostic_in_project`, which accepts the
    /// build-dir-relative paths compilers actually print — see its note.
    async fn clear_previous_diagnostics(
        &self,
        stream: &mpsc::Sender<TransactionRequest>,
        build_type: &str,
        project_root: &Path,
    ) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();
        self.memory_graph_tx
            .send(MemoryGraphMessage::QueryAttrNodes {
                node_type: Some("Diagnostic".to_string()),
                subtype: None,
                name: None,
                limit: Some(5000),
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("MemoryGraph channel closed: {e}"))?;
        let nodes = rx
            .await
            .map_err(|e| format!("MemoryGraph response lost: {e}"))?
            .map_err(|e| format!("Diagnostic query failed: {e}"))?;

        let root = Self::diagnostics_scope_root(project_root)
            .to_string_lossy()
            .trim_end_matches('/')
            .to_string();
        for node in nodes {
            // Only this project's diagnostics of this kind — never another
            // project's, and never another run kind (build vs lint vs fix).
            let same_kind = node.get("build_type").and_then(|v| v.as_str()) == Some(build_type);
            let in_project = node
                .get("file")
                .and_then(|v| v.as_str())
                .map(|file| Self::diagnostic_in_project(file, &root))
                .unwrap_or(false);
            if same_kind && in_project {
                Self::send_stream_op(stream, StreamOp::DeleteNode(node.id().to_string())).await?;
            }
        }
        Ok(())
    }

    /// Persist a run's BuildEvents as Diagnostic graph nodes linked to their
    /// SourceFile nodes via HasDiagnostic edges. Each run is tagged with a
    /// build_run_id.
    ///
    /// A run SUPERSEDES the previous run of the same kind for this project (see
    /// [`Self::clear_previous_diagnostics`]), so the Build tab always reflects
    /// the latest run rather than an accumulation of every past one.
    async fn ingest_diagnostics(
        &self,
        events: &[serde_json::Value],
        build_type: &str,
        project_root: &Path,
    ) -> Result<(), String> {
        // Collect events that carry a file path — these become Diagnostic nodes.
        let diag_events: Vec<&serde_json::Value> = events
            .iter()
            .filter(|ev| {
                ev.get("file").and_then(|f| f.as_str()).is_some()
                    && ev.get("level").and_then(|l| l.as_str()).is_some()
            })
            .collect();

        let run_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.memory_graph_tx
            .send(MemoryGraphMessage::OpenTransactionStream { reply_to: tx })
            .await
            .map_err(|e| format!("MemoryGraph channel closed: {e}"))?;
        let stream = rx
            .await
            .map_err(|e| format!("MemoryGraph response lost: {e}"))?;

        // Supersede the previous run of this kind — deliberately BEFORE the
        // "nothing to ingest" return, so a run that is now clean clears the
        // findings it used to report instead of leaving them on screen.
        self.clear_previous_diagnostics(&stream, build_type, project_root)
            .await?;
        if diag_events.is_empty() {
            Self::send_stream_op(&stream, StreamOp::Commit).await?;
            return Ok(());
        }

        for ev in diag_events {
            let file = ev["file"].as_str().unwrap_or("").to_string();
            let level = ev["level"].as_str().unwrap_or("info").to_string();
            let message = ev["message"]
                .as_str()
                .unwrap_or_else(|| ev["line"].as_str().unwrap_or(""))
                .to_string();
            let line = ev["line_number"].as_u64().map(|v| v as u32);
            let mut props = HashMap::new();
            props.insert(
                "message".to_string(),
                serde_json::Value::String(message.clone()),
            );
            props.insert("file".to_string(), serde_json::Value::String(file.clone()));
            if let Some(line) = line {
                props.insert("line".to_string(), serde_json::json!(line));
            }
            props.insert("severity".to_string(), serde_json::Value::String(level));
            props.insert(
                "build_type".to_string(),
                serde_json::Value::String(build_type.to_string()),
            );
            props.insert(
                "build_run_id".to_string(),
                serde_json::Value::String(run_id.clone()),
            );

            // Stable name so MergeNode upserts by (Diagnostic, name).
            let name = format!(
                "{}:{}:{}",
                file,
                line.map(|l| l.to_string()).unwrap_or_default(),
                message
            );
            let merge_result = Self::send_stream_op(
                &stream,
                StreamOp::MergeNode(bm_attr_unknown(
                    "Diagnostic",
                    Some(build_type.to_string()),
                    name,
                    Some(message.clone()),
                    props.clone(),
                )),
            )
            .await?;

            // Link SourceFile --HasDiagnostic--> Diagnostic using the merged
            // node's stable id (kept across upserts).
            let diag_id = match merge_result {
                StreamOpResult::NodeStored(n) | StreamOpResult::NodeUpdated(n) => {
                    n.id().to_string()
                }
                _ => continue,
            };
            if let Some(source_node) = self.find_source_file(&file).await {
                let _ = Self::send_stream_op(
                    &stream,
                    StreamOp::MergeRelationship(RelationshipInput {
                        edge_type: RelationshipType::HasDiagnostic,
                        from_id: source_node.id().to_string(),
                        to_id: diag_id,
                        properties: None,
                        weight: None,
                    }),
                )
                .await;
            }
        }

        Self::send_stream_op(&stream, StreamOp::Commit).await?;
        Ok(())
    }

    /// Parse a source file via the extension router and persist the AST to the
    /// graph in a single transaction stream (UPSERT semantics via MergeNode /
    /// MergeRelationship). Skips the write when the content hash is unchanged.
    async fn parse_and_store_source_file(&self, path: &Path) -> Result<ParseSummary, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| format!("No file extension for {}", path.display()))?
            .to_string();
        let module_tx = self
            .extension_router
            .get(&ext)
            .cloned()
            .ok_or_else(|| format!("No parser registered for .{}", ext))?;

        // 1. Module parses the file (stateless — never touches the graph).
        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::ParseSourceFile {
                file_path: path.to_path_buf(),
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {e}"))?;
        let result: AstParseResult = rx
            .await
            .map_err(|e| format!("Module response lost: {e}"))??;

        let path_str = path.to_string_lossy().to_string();

        // 2. Incremental re-parse: skip if the content hash is unchanged.
        if let Some(existing) = self.find_source_file(&path_str).await {
            if existing.get("content_hash").and_then(|v| v.as_str())
                == Some(result.content_hash.as_str())
            {
                return Ok(ParseSummary {
                    nodes_written: 0,
                    edges_written: 0,
                    skipped: true,
                });
            }
        }

        // 3. Open a transaction stream and persist the AST.
        let (tx, rx) = oneshot::channel();
        self.memory_graph_tx
            .send(MemoryGraphMessage::OpenTransactionStream { reply_to: tx })
            .await
            .map_err(|e| format!("MemoryGraph channel closed: {e}"))?;
        let stream = rx
            .await
            .map_err(|e| format!("MemoryGraph response lost: {e}"))?;

        // SourceFile node — reuses the node created by ProjectSyncActor during
        // the bootstrap scan (unified file representation), or creates one if
        // no file-tree node exists yet. The AST `ast_child` edges below link
        // this SourceFile node (the file-tree entity) to its parsed children.
        let existing_source = self.find_source_file(&path_str).await;
        let source_file_id = if let Some(existing) = &existing_source {
            // Update metadata (content_hash, last_parsed, has_errors) in place.
            let mut source_props = HashMap::new();
            source_props.insert(
                "content_hash".to_string(),
                serde_json::Value::String(result.content_hash.clone()),
            );
            source_props.insert(
                "last_parsed".to_string(),
                serde_json::Value::String(Utc::now().to_rfc3339()),
            );
            source_props.insert(
                "has_errors".to_string(),
                serde_json::Value::Bool(result.has_errors),
            );
            Self::send_stream_op(
                &stream,
                StreamOp::UpdateNode {
                    id: existing.id().to_string(),
                    updates: NodeUpdate {
                        node_type: None,
                        subtype: None,
                        name: None,
                        description: None,
                        properties: Some(source_props),
                        embedding_id: None,
                    },
                },
            )
            .await?;
            existing.id().to_string()
        } else {
            // No file-tree node yet — create a fresh SourceFile (UPSERT).
            let mut source_props = HashMap::new();
            source_props.insert(
                "path".to_string(),
                serde_json::Value::String(path_str.clone()),
            );
            source_props.insert(
                "language".to_string(),
                serde_json::Value::String(result.language.clone()),
            );
            source_props.insert(
                "content_hash".to_string(),
                serde_json::Value::String(result.content_hash.clone()),
            );
            source_props.insert(
                "last_parsed".to_string(),
                serde_json::Value::String(Utc::now().to_rfc3339()),
            );
            source_props.insert(
                "has_errors".to_string(),
                serde_json::Value::Bool(result.has_errors),
            );
            let op_result = Self::send_stream_op(
                &stream,
                StreamOp::MergeNode(bm_attr_unknown(
                    "SourceFile",
                    None,
                    path_str.clone(),
                    Some(format!("Source file: {}", path_str)),
                    source_props,
                )),
            )
            .await?;
            match op_result {
                StreamOpResult::NodeStored(n) | StreamOpResult::NodeUpdated(n) => {
                    n.id().to_string()
                }
                _ => return Err("MergeNode did not return a node".to_string()),
            }
        };
        let mut nodes_written = 1usize;
        let mut edges_written = 0usize;

        // AST nodes (UPSERT by (type, name)).
        // Map each index in result.nodes to its stored node id (None → skipped).
        let mut stored_ids: Vec<Option<String>> = Vec::with_capacity(result.nodes.len());
        let mut top_level_ids: Vec<String> = Vec::new();
        for node in &result.nodes {
            let node_type = match node.node_type.as_str() {
                "function" | "method" => "AstFunction",
                "class" => "AstClass",
                "import" => "AstImport",
                "variable" => "AstVariable",
                _ => {
                    // Blocks and other structural nodes are not stored as
                    // first-class graph nodes; their edges are skipped too.
                    stored_ids.push(None);
                    continue;
                }
            };

            let mut props = HashMap::new();
            props.insert(
                "kind".to_string(),
                serde_json::Value::String(node.node_type.clone()),
            );
            props.insert(
                "text".to_string(),
                serde_json::Value::String(node.text.clone()),
            );
            props.insert(
                "start_line".to_string(),
                serde_json::Value::Number(node.start_line.into()),
            );
            props.insert(
                "start_col".to_string(),
                serde_json::Value::Number(node.start_col.into()),
            );
            props.insert(
                "end_line".to_string(),
                serde_json::Value::Number(node.end_line.into()),
            );
            props.insert(
                "end_col".to_string(),
                serde_json::Value::Number(node.end_col.into()),
            );
            props.insert(
                "depth".to_string(),
                serde_json::Value::Number(node.depth.into()),
            );
            props.insert(
                "file_path".to_string(),
                serde_json::Value::String(path_str.clone()),
            );
            props.insert(
                "language".to_string(),
                serde_json::Value::String(result.language.clone()),
            );
            props.insert(
                "is_public".to_string(),
                serde_json::Value::Bool(node.is_public),
            );
            props.insert(
                "is_async".to_string(),
                serde_json::Value::Bool(node.is_async),
            );
            if let Some(sig) = &node.signature {
                props.insert(
                    "signature".to_string(),
                    serde_json::Value::String(sig.clone()),
                );
            }
            if let Some(rt) = &node.return_type {
                props.insert(
                    "return_type".to_string(),
                    serde_json::Value::String(rt.clone()),
                );
            }

            let name = node.name.clone().unwrap_or_else(|| {
                format!("{}@{}:{}", node.node_type, node.start_line, node.start_col)
            });
            let description = if node.text.is_empty() {
                None
            } else {
                Some(node.text.chars().take(200).collect())
            };

            let op_result = Self::send_stream_op(
                &stream,
                StreamOp::MergeNode(bm_attr_unknown(
                    node_type,
                    None,
                    name.clone(),
                    description,
                    props,
                )),
            )
            .await?;
            let id = match op_result {
                StreamOpResult::NodeStored(n) | StreamOpResult::NodeUpdated(n) => {
                    n.id().to_string()
                }
                _ => return Err("MergeNode did not return a node".to_string()),
            };
            if node.depth == 0 {
                top_level_ids.push(id.clone());
            }
            stored_ids.push(Some(id));
            nodes_written += 1;
        }

        // Link the owning SourceFile node to its top-level AST nodes
        // (the file tree and AST graph are unified — one node per file).
        for (order, child_id) in top_level_ids.iter().enumerate() {
            let mut props = HashMap::new();
            props.insert(
                "order".to_string(),
                serde_json::Value::Number((order as u32).into()),
            );
            Self::send_stream_op(
                &stream,
                StreamOp::MergeRelationship(RelationshipInput {
                    edge_type: RelationshipType::Custom("ast_child".to_string()),
                    from_id: source_file_id.clone(),
                    to_id: child_id.clone(),
                    properties: Some(props),
                    weight: None,
                }),
            )
            .await?;
            edges_written += 1;
        }

        // AST edges (UPSERT by (edge label, from_id, to_id)).
        for edge in &result.edges {
            let from_id = match stored_ids.get(edge.from_index) {
                Some(Some(id)) => id.clone(),
                _ => continue,
            };
            let to_id = match stored_ids.get(edge.to_index) {
                Some(Some(id)) => id.clone(),
                _ => continue,
            };
            let edge_type = match edge.edge_type.as_str() {
                "child" => "ast_child",
                "calls" => "ast_calls",
                "imports" => "ast_imports",
                "references" => "ast_references",
                _ => continue,
            };

            let mut props = HashMap::new();
            if let Some(order) = edge.order {
                props.insert("order".to_string(), serde_json::Value::Number(order.into()));
            }
            if let Some(field) = &edge.field {
                props.insert(
                    "field".to_string(),
                    serde_json::Value::String(field.clone()),
                );
            }

            Self::send_stream_op(
                &stream,
                StreamOp::MergeRelationship(RelationshipInput {
                    edge_type: RelationshipType::Custom(edge_type.to_string()),
                    from_id,
                    to_id,
                    properties: Some(props),
                    weight: None,
                }),
            )
            .await?;
            edges_written += 1;
        }

        // 4. Commit the transaction.
        Self::send_stream_op(&stream, StreamOp::Commit).await?;

        Ok(ParseSummary {
            nodes_written,
            edges_written,
            skipped: false,
        })
    }

    /// Analyze: route to the matching module, then store result in graph.
    async fn analyze_project(
        &self,
        path: &Path,
        config_file: Option<&str>,
    ) -> Result<BuildMetadata, String> {
        // Prefer the explicitly requested config file (the discovery already
        // knows which one it found); fall back to directory detection.
        let config = match config_file {
            Some(cf) if self.router.contains_key(cf) => cf.to_string(),
            _ => self
                .find_config_file(path)
                .ok_or_else(|| format!("No known build config found for {}", path.display()))?,
        };
        // Through the same single decision point as `build_project`. Platform threading for
        // this path is a one-line change here once its `opts` is plumbed.
        let module_tx = self.module_tx_for(&config, None)?;

        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::Analyze {
                path: path.to_path_buf(),
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        let metadata = rx
            .await
            .map_err(|e| format!("Module response lost: {}", e))??;

        // Persist as the manager's state (single writer via MemoryGraphActor).
        // This is best-effort: a persistence failure must not discard a valid
        // analysis result. The metadata is already correct; it just won't be
        // available via GetAnalysis until a later successful persist.
        let path_str = path.to_string_lossy().to_string();
        if let Err(e) = self.store_analysis(&path_str, &metadata).await {
            tracing::warn!(
                "BuildManager: failed to persist analysis for {}: {}",
                path_str,
                e
            );
        }

        Ok(metadata)
    }

    /// Build: fetch stored analysis, route to module with the analysis in the
    /// message (the module stays stateless). Batch — returns only BuildOutput.
    async fn build_project(&self, path: &Path, opts: &BuildOptions) -> Result<BuildOutput, String> {
        let metadata = self.analysis_for(path).await?;

        let config = metadata
            .config_files
            .first()
            .cloned()
            .ok_or_else(|| "Stored analysis has no config file".to_string())?;
        // A platform-specific module wins over the config file's owner: the platform is what
        // the invocation actually depends on, and an ESP32 build is still a Cargo.toml project.
        // A request that names none gets the chip the tree declares, when it declares one.
        let opts = self.opts_with_declared_platform(path, opts);
        let module_tx = self.module_tx_for(&config, opts.platform.as_deref())?;

        let build_spec = Self::resolve_build_spec(&metadata, &opts);
        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::Build {
                path: path.to_path_buf(),
                metadata,
                opts,
                build_spec,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        rx.await
            .map_err(|e| format!("Module response lost: {}", e))?
    }

    /// Streaming build that collects every per-line BuildEvent and returns them
    /// alongside the final BuildOutput. The UI receives ALL lines reliably (no
    /// broadcast-channel overflow).
    async fn build_project_with_events(
        &self,
        path: &Path,
        opts: &BuildOptions,
    ) -> Result<(BuildOutput, Vec<serde_json::Value>), String> {
        let metadata = self.analysis_for(path).await?;

        let config = metadata
            .config_files
            .first()
            .cloned()
            .ok_or_else(|| "Stored analysis has no config file".to_string())?;
        // The platform decides the module here too — `build_build` is the path a UI Build click
        // and a `build_build` tool call share, so leaving `platform` out of the lookup sent an
        // ESP32 build to the cargo module while the batch `build_project` above sent it to the
        // platform module. Two paths, two answers, for the same request. A request that names no
        // platform gets the chip the tree declares, which is what the dashboard's Build sends.
        let opts = self.opts_with_declared_platform(path, opts);
        let module_tx = self.module_tx_for(&config, opts.platform.as_deref())?;

        let (tx, rx) = oneshot::channel();
        let (build_event_tx, mut build_event_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::build::BuildEvent>();
        let events_buf =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let event_log = self.event_log_tx.clone();
        {
            let events_buf = events_buf.clone();
            tokio::spawn(async move {
                while let Some(ev) = build_event_rx.recv().await {
                    let json = serde_json::json!({
                        "line": ev.line,
                        "level": ev.level,
                        "target": ev.target,
                        "file": ev.file,
                        "line_number": ev.line_number,
                        "message": ev.message,
                        "detail": ev.detail,
                    });
                    events_buf.lock().unwrap().push(json.clone());
                    // Incremental: forward to the actor-owned build-event log
                    // (the FFI drains it while this actor's mailbox is busy).
                    if let Some(event_log_tx) = event_log.as_ref() {
                        let _ = event_log_tx.send(BuildEventLogMessage::Record(json)).await;
                    }
                }
            });
        }
        let build_spec = Self::resolve_build_spec(&metadata, &opts);
        module_tx
            .send(BuildModuleMessage::BuildStreaming {
                path: path.to_path_buf(),
                metadata,
                opts: opts.clone(),
                build_spec,
                event_tx: build_event_tx,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        let output = rx
            .await
            .map_err(|e| format!("Module response lost: {}", e))?;
        let events = events_buf.lock().unwrap().clone();
        output.map(|o| (o, events))
    }

    /// Reject an operation the registered module declares unsupported.
    ///
    /// Modules set `supports_*` in their `ModuleCapability`; the modules without
    /// a clean/lint/format/fix implementation leave them false, so the manager
    /// surfaces a clear error instead of forwarding to a module that would reply
    /// "not implemented".
    fn check_capability(
        &self,
        config: &str,
        op: &str,
        supported: impl Fn(&ModuleCapability) -> bool,
    ) -> Result<(), String> {
        if let Some(cap) = self
            .capabilities
            .iter()
            .find(|c| c.config_files.iter().any(|f| f == config))
        {
            if !supported(cap) {
                return Err(format!(
                    "{} is not supported for build system {}",
                    op, cap.build_system
                ));
            }
        }
        Ok(())
    }

    /// Streaming lint that pushes every line into the shared pollable buffer.
    async fn lint_project_streaming(
        &self,
        path: &Path,
        platform: Option<String>,
    ) -> Result<(BuildOutput, Vec<serde_json::Value>), String> {
        let metadata = self.analysis_for(path).await?;

        let config = metadata
            .config_files
            .first()
            .cloned()
            .ok_or_else(|| "Stored analysis has no config file".to_string())?;
        self.check_capability(&config, "lint", |c| c.supports_lint)?;
        let module_tx = self.module_tx_for(&config, None)?;

        let (tx, rx) = oneshot::channel();
        let (build_event_tx, mut build_event_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::build::BuildEvent>();
        let events_buf =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let event_log = self.event_log_tx.clone();
        {
            let events_buf = events_buf.clone();
            tokio::spawn(async move {
                while let Some(ev) = build_event_rx.recv().await {
                    let json = serde_json::json!({
                        "line": ev.line,
                        "level": ev.level,
                        "target": ev.target,
                        "file": ev.file,
                        "line_number": ev.line_number,
                        "message": ev.message,
                        "detail": ev.detail,
                    });
                    events_buf.lock().unwrap().push(json.clone());
                    // Incremental: forward to the actor-owned build-event log
                    // (the FFI drains it while this actor's mailbox is busy).
                    if let Some(event_log_tx) = event_log.as_ref() {
                        let _ = event_log_tx.send(BuildEventLogMessage::Record(json)).await;
                    }
                }
            });
        }
        module_tx
            .send(BuildModuleMessage::LintStreaming {
                path: path.to_path_buf(),
                metadata,
                platform: platform.clone(),
                event_tx: build_event_tx,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        let output = rx
            .await
            .map_err(|e| format!("Module response lost: {}", e))?;
        let events = events_buf.lock().unwrap().clone();
        output.map(|o| (o, events))
    }

    /// Streaming fix that pushes every line into the shared pollable buffer.
    async fn fix_project_streaming(
        &self,
        path: &Path,
    ) -> Result<(BuildOutput, Vec<serde_json::Value>), String> {
        let metadata = self.analysis_for(path).await?;

        let config = metadata
            .config_files
            .first()
            .cloned()
            .ok_or_else(|| "Stored analysis has no config file".to_string())?;
        self.check_capability(&config, "fix", |c| c.supports_fix)?;
        let module_tx = self.module_tx_for(&config, None)?;

        let (tx, rx) = oneshot::channel();
        let (build_event_tx, mut build_event_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::build::BuildEvent>();
        let events_buf =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let event_log = self.event_log_tx.clone();
        {
            let events_buf = events_buf.clone();
            tokio::spawn(async move {
                while let Some(ev) = build_event_rx.recv().await {
                    let json = serde_json::json!({
                        "line": ev.line,
                        "level": ev.level,
                        "target": ev.target,
                        "file": ev.file,
                        "line_number": ev.line_number,
                        "message": ev.message,
                        "detail": ev.detail,
                    });
                    events_buf.lock().unwrap().push(json.clone());
                    // Incremental: forward to the actor-owned build-event log
                    // (the FFI drains it while this actor's mailbox is busy).
                    if let Some(event_log_tx) = event_log.as_ref() {
                        let _ = event_log_tx.send(BuildEventLogMessage::Record(json)).await;
                    }
                }
            });
        }
        module_tx
            .send(BuildModuleMessage::FixStreaming {
                path: path.to_path_buf(),
                metadata,
                event_tx: build_event_tx,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        let output = rx
            .await
            .map_err(|e| format!("Module response lost: {}", e))?;
        let events = events_buf.lock().unwrap().clone();
        output.map(|o| (o, events))
    }

    /// Test: same pattern as build.
    async fn test_project(
        &self,
        path: &Path,
        opts: &TestOptions,
        platform: Option<String>,
    ) -> Result<BuildOutput, String> {
        let metadata = self.analysis_for(path).await?;

        let config = metadata
            .config_files
            .first()
            .cloned()
            .ok_or_else(|| "Stored analysis has no config file".to_string())?;
        let module_tx = self.module_tx_for(&config, None)?;

        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::Test {
                path: path.to_path_buf(),
                metadata,
                opts: opts.clone(),
                platform,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        rx.await
            .map_err(|e| format!("Module response lost: {}", e))?
    }

    /// Unified LLM tool entry point: routes build/* tools to handlers.
    /// Clean: same pattern as build — route to module via a proper message.
    async fn clean_project(
        &self,
        path: &Path,
        platform: Option<String>,
    ) -> Result<BuildOutput, String> {
        let metadata = self.analysis_for(path).await?;

        let config = metadata
            .config_files
            .first()
            .cloned()
            .ok_or_else(|| "Stored analysis has no config file".to_string())?;
        self.check_capability(&config, "clean", |c| c.supports_clean)?;
        let module_tx = self.module_tx_for(&config, None)?;

        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::Clean {
                path: path.to_path_buf(),
                metadata,
                platform,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        rx.await
            .map_err(|e| format!("Module response lost: {}", e))?
    }

    /// The last `n` lines of build output.
    ///
    /// Flash the analyzed project's artifact to the named platform's device.
    async fn flash_project(
        &self,
        path: &Path,
        opts: &BuildOptions,
        artifact: Option<PathBuf>,
        port: Option<PathBuf>,
    ) -> Result<BuildOutput, String> {
        let platform_id = opts
            .platform
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| "flashing needs a platform, e.g. \"esp32c6\"".to_string())?;

        let metadata = self.analysis_for(path).await?;
        let config = metadata
            .config_files
            .first()
            .cloned()
            .ok_or_else(|| "Stored analysis has no config file".to_string())?;

        self.route_for_flash(&config, platform_id)?;
        let module_tx = self.module_tx_for(&config, Some(platform_id))?;

        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::Flash {
                path: path.to_path_buf(),
                metadata,
                opts: opts.clone(),
                artifact,
                port,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        rx.await
            .map_err(|e| format!("Module response lost: {}", e))?
    }

    /// Parse clang/clang++ analyzer output into structured lint events:
    /// `file:line:col: error|warning: message` (and `file:line:col: fatal error: …`).
    fn parse_clang_output(output: &str) -> Vec<serde_json::Value> {
        let mut events = Vec::new();
        // Clang emits diagnostics as:  /path/file.cpp:12:5: error: message text
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Split at the first ": " that precedes error/warning/fatal.
            let lower = line.to_lowercase();
            // Longest marker first: "fatal error:" CONTAINS "error:", so matching
            // "error:" first would leave the "fatal " prefix in the path part and
            // swallow the line number — turning `../x.cpp:41:10: fatal error: …` into
            // the unresolvable file `../x.cpp:41`, which is why missing-header errors
            // could never be located (and so were silently skipped by the fix loop).
            let markers = ["fatal error:", "error:", "warning:"];
            let Some(marker) = markers.iter().find(|m| lower.contains(**m)) else {
                continue;
            };
            // Find the position of the message.
            let Some(msg_idx) = lower.find(marker) else {
                continue;
            };
            let path_part = &line[..msg_idx];
            let msg = line[msg_idx + marker.len()..].trim().to_string();
            // path_part is "<file>:<line>:<col>: " (column optional). Drop the
            // trailing separator FIRST, then read the numbers off the right — splitting
            // straight away glued the line number onto the file name, so every build
            // diagnostic pointed at a path that cannot exist (`../x.cpp:41`) and the
            // fix loop skipped all of them.
            let mut loc = path_part.trim_end();
            if let Some(stripped) = loc.strip_suffix(':') {
                loc = stripped.trim_end();
            }
            let (head, tail) = loc.rsplit_once(':').unwrap_or(("", loc));
            let (file, line_no, col) = match head.rsplit_once(':') {
                // "<file>:<line>:<col>" — two numbers, so the pair is line:column.
                Some((path, line)) if line.trim().parse::<u64>().is_ok() => (
                    path.trim().to_string(),
                    line.trim().parse::<u64>().ok(),
                    tail.trim().parse::<u64>().ok(),
                ),
                // "<file>:<line>" — a single number is the line.
                _ => (
                    head.trim().to_string(),
                    tail.trim().parse::<u64>().ok(),
                    None,
                ),
            };
            // "fatal error:" is an error, not a warning.
            let severity = if marker.contains("error") {
                "error"
            } else {
                "warning"
            };
            events.push(serde_json::json!({
                "file": file,
                "level": severity,
                "line": msg.clone(),
                "message": msg,
                "line_number": line_no,
                "column": col,
            }));
        }
        events
    }

    /// True when a diagnostic's recorded `file` belongs to `project_root`.
    ///
    /// Compilers print paths the way they were invoked, and Meson/ninja invoke them
    /// RELATIVE to the build directory (`../app/main.cpp`), so a relative path is
    /// this project's by definition — the memory graph is per-project. An absolute
    /// path must still sit under the project root, so a shared graph can never have
    /// another project's diagnostics deleted by mistake.
    ///
    /// Getting this wrong is what let ~137 stale BUILD errors (recorded with relative
    /// paths by an earlier platform-less build) survive every supersede and be
    /// re-reported by "Fix & Verify" as if the current rpi5 build produced them.
    fn diagnostic_in_project(file: &str, project_root: &str) -> bool {
        let file = file.trim();
        if file.is_empty() {
            return false;
        }
        let path = std::path::Path::new(file);
        if !path.is_absolute() {
            return true;
        }
        // A compiler may spell the same directory differently than the caller did:
        // `/var` versus `/private/var` on macOS, a symlinked workspace, or a `..`
        // segment. A lexical prefix check then says "another project" and the stale
        // errors are never cleared — which is exactly how Fix & Verify re-reported an
        // error it had already fixed. Compare canonical paths when both resolve.
        let root = std::path::Path::new(project_root.trim_end_matches('/'));
        if let (Ok(file), Ok(root)) = (path.canonicalize(), root.canonicalize()) {
            return file.starts_with(root);
        }
        // Fall back to a lexical comparison with a path boundary: `/p/app2` is not
        // under `/p/app`, which a bare `starts_with` would have claimed it was.
        let file = path.to_string_lossy();
        let root = project_root.trim_end_matches('/');
        file == root || file.starts_with(&format!("{root}/"))
    }

    /// Format: same pattern as build — route to module via a proper message.
    async fn format_project(&self, path: &Path) -> Result<BuildOutput, String> {
        let metadata = self.analysis_for(path).await?;

        let config = metadata
            .config_files
            .first()
            .cloned()
            .ok_or_else(|| "Stored analysis has no config file".to_string())?;
        self.check_capability(&config, "format", |c| c.supports_format)?;
        let module_tx = self.module_tx_for(&config, None)?;

        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::Format {
                path: path.to_path_buf(),
                metadata,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        rx.await
            .map_err(|e| format!("Module response lost: {}", e))?
    }

    /// Scaffold a new project by routing to the build module that owns the
    /// requested build system (e.g. "Cargo" → Cargo.toml, "Python" → pyproject.toml,
    /// "Meson" → meson.build). Templates live 1:1 with each build module.
    async fn scaffold_build_config(
        &self,
        project_name: &str,
        build_system: &str,
        goal: &str,
        platforms: &[String],
        structure: spire_core::build_types::ProjectStructure,
        embedded: bool,
    ) -> Result<crate::build::ScaffoldOutput, String> {
        // Map the build-system label to the config file the module owns.
        let config = self
            .capabilities
            .iter()
            .find(|cap| {
                cap.build_system.eq_ignore_ascii_case(build_system)
                    || cap.language.eq_ignore_ascii_case(build_system)
            })
            .and_then(|cap| cap.config_files.first().cloned())
            .ok_or_else(|| {
                format!(
                    "No build module registered for build system '{}'",
                    build_system
                )
            })?;

        let module_tx = self.module_tx_for(&config, None)?;

        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::ScaffoldBuildConfig {
                project_name: project_name.to_string(),
                goal: goal.to_string(),
                platforms: platforms.to_vec(),
                structure,
                embedded,
                // This path has no library to name: it is reached by a caller that asked for a
                // *build system*, not for a project type, and the two ESP-IDF types are reached
                // through `ScaffoldBuildConfig` proper.
                library: None,
                // For the same reason, no decomposition: it is a fact about an application's design,
                // and this path does not know of one.
                application: None,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        rx.await
            .map_err(|e| format!("Module response lost: {}", e))?
    }

    /// Find the registered module capability for a build-system label.
    /// Matches `build_system` or `language` case-insensitively (e.g. "Cargo",
    /// "Rust", "npm", "JavaScript", "Meson").
    fn module_for_build_system(&self, bt: &str) -> Option<&ModuleCapability> {
        self.capabilities.iter().find(|cap| {
            cap.build_system.eq_ignore_ascii_case(bt) || cap.language.eq_ignore_ascii_case(bt)
        })
    }

    /// Dispatch a generic tool to the module that owns `build_system` by
    /// wrapping `BuildModuleMessage::CallTool`. Unwraps the module's
    /// MCP-style reply envelope (`{ result: { content, isError } }`) into a
    /// plain JSON value so callers (project_* actors) get the same shape
    /// they previously received from MCP servers.
    async fn call_module_tool(
        &self,
        build_system: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let config = self
            .module_for_build_system(build_system)
            .and_then(|cap| cap.config_files.first().cloned())
            .ok_or_else(|| {
                format!(
                    "No build module registered for build system '{}'",
                    build_system
                )
            })?;
        let module_tx = self.module_tx_for(&config, None)?;

        let (tx, rx) = oneshot::channel();
        module_tx
            .send(BuildModuleMessage::CallTool {
                tool_name: tool_name.to_string(),
                args,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("Module channel closed: {}", e))?;
        let reply = rx
            .await
            .map_err(|e| format!("Module response lost: {}", e))?;

        // Modules reply with an MCP-shaped envelope; some (node/meson stubs)
        // reply with a bare `{ "error": "…" }` instead.
        if let Some(err_text) = reply.get("error").and_then(|v| v.as_str()).or_else(|| {
            reply
                .get("result")
                .and_then(|r| r.get("isError"))
                .and_then(|ie| ie.as_bool())
                .filter(|b| *b)
                .and_then(|_| {
                    reply["result"]["content"][0]
                        .get("text")
                        .and_then(|t| t.as_str())
                })
        }) {
            return Err(err_text.to_string());
        }

        let text = reply["result"]["content"][0]
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("{}");
        serde_json::from_str(text)
            .map_err(|e| format!("Failed to parse module tool response: {}", e))
    }

    async fn call_tool(&self, tool_name: &str, args: serde_json::Value) -> serde_json::Value {
        match tool_name {
            "build_analyze" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                match self.analyze_project(Path::new(path), None).await {
                    Ok(md) => serde_json::to_value(md)
                        .unwrap_or(serde_json::json!({"error": "serialize"})),
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_build" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("");
                let opts = BuildOptions {
                    mode: mode.to_string(),
                    package: args
                        .get("package")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    platform: args
                        .get("platform")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    target: args
                        .get("target")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };
                match self.build_project_with_events(Path::new(path), &opts).await {
                    Ok((o, mut events)) => {
                        // Persist warnings/errors as Diagnostic graph nodes so the
                        // Build tab can show them after the run. Meson/ninja put
                        // compiler warnings in o.output as plain text (the streamed
                        // events only carry module "finished" markers), so parse
                        // them into diagnostic events when none were streamed.
                        let has_file_diags = events
                            .iter()
                            .any(|e| e.get("file").and_then(|f| f.as_str()).is_some());
                        if !has_file_diags {
                            events.append(&mut Self::parse_clang_output(&o.output));
                        }
                        let _ = self
                            .ingest_diagnostics(&events, "build", Path::new(path))
                            .await;
                        // Persist the build status PER TARGET (success/duration +
                        // raw output) so building rock3c and rpi5 store separate
                        // results under the same key the platform list reads back.
                        let status_target = self
                            .resolve_status_target(
                                path,
                                opts.target.as_deref(),
                                opts.platform.as_deref(),
                            )
                            .await;
                        let _ = self
                            .store_action_status_with_output(
                                "build",
                                path,
                                status_target.as_deref(),
                                o.success,
                                o.duration_secs,
                                &o.output,
                            )
                            .await;
                        let mut val = serde_json::to_value(o)
                            .unwrap_or(serde_json::json!({"error": "serialize"}));
                        if let serde_json::Value::Object(ref mut m) = val {
                            m.insert("buildEvents".to_string(), serde_json::json!(events));
                        }
                        val
                    }
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_test" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let filter = args
                    .get("filter")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let platform = args
                    .get("platform")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let opts = TestOptions { filter };
                let status_target = self
                    .resolve_status_target(
                        path,
                        args.get("target").and_then(|v| v.as_str()),
                        platform.as_deref(),
                    )
                    .await;
                match self.test_project(Path::new(path), &opts, platform).await {
                    Ok(o) => {
                        // Record the test result PER TARGET as well, so the
                        // platform list shows the last test — not just builds.
                        let _ = self
                            .store_action_status(
                                "test",
                                path,
                                status_target.as_deref(),
                                o.success,
                                o.duration_secs,
                            )
                            .await;
                        serde_json::to_value(o).unwrap_or(serde_json::json!({"error": "serialize"}))
                    }
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_flash" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let opts = BuildOptions {
                    mode: args
                        .get("mode")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    package: args
                        .get("package")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    platform: args
                        .get("platform")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    target: None,
                };
                let artifact = args
                    .get("artifact")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(PathBuf::from);
                let port = args
                    .get("port")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from);
                let status_target = self
                    .resolve_status_target(path, None, opts.platform.as_deref())
                    .await;
                match self
                    .flash_project(Path::new(path), &opts, artifact, port)
                    .await
                {
                    Ok(o) => {
                        // Recorded per target like build/test, so the platform list can show
                        // "last flashed" alongside "last built" — the two facts that decide
                        // whether the chip is running what is on disk.
                        let _ = self
                            .store_action_status_with_output(
                                "flash",
                                path,
                                status_target.as_deref(),
                                o.success,
                                o.duration_secs,
                                &o.output,
                            )
                            .await;
                        serde_json::to_value(o).unwrap_or(serde_json::json!({"error": "serialize"}))
                    }
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_clean" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let platform = args
                    .get("platform")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let status_target = self
                    .resolve_status_target(
                        path,
                        args.get("target").and_then(|v| v.as_str()),
                        platform.as_deref(),
                    )
                    .await;
                match self.clean_project(Path::new(path), platform).await {
                    Ok(o) => {
                        // Clean is a per-platform action too (`meson compile
                        // --clean -C build-<platform>`), so record it per target.
                        let _ = self
                            .store_action_status(
                                "clean",
                                path,
                                status_target.as_deref(),
                                o.success,
                                o.duration_secs,
                            )
                            .await;
                        serde_json::to_value(o).unwrap_or(serde_json::json!({"error": "serialize"}))
                    }
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_lint" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                tracing::info!(
                    "build_lint: path={:?} exists={}",
                    path,
                    std::path::Path::new(path).exists()
                );
                let platform = args
                    .get("platform")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let status_target = self
                    .resolve_status_target(
                        path,
                        args.get("target").and_then(|v| v.as_str()),
                        platform.as_deref(),
                    )
                    .await;
                match self.lint_project_streaming(Path::new(path), platform).await {
                    Ok((o, mut events)) => {
                        tracing::info!(
                            "build_lint OK: success={} out_len={} events={}",
                            o.success,
                            o.output.len(),
                            events.len()
                        );
                        // Meson lint returns clang analyzer output as plain text in
                        // `o.output` (the streaming events are usually empty), so
                        // parse the clang diagnostic lines into structured events.
                        // That lets ingest_diagnostics persist graph nodes and the
                        // UI's lint/diagnostics panel show real findings.
                        if events.is_empty() {
                            events.append(&mut Self::parse_clang_output(&o.output));
                            tracing::info!(
                                "build_lint: parsed {} diagnostics from output",
                                events.len()
                            );
                        }
                        // Persist lint findings as Diagnostic graph nodes so the
                        // Build tab shows them after the lint run.
                        let _ = self
                            .ingest_diagnostics(&events, "lint", Path::new(path))
                            .await;
                        let _ = self
                            .store_action_status(
                                "lint",
                                path,
                                status_target.as_deref(),
                                o.success,
                                o.duration_secs,
                            )
                            .await;
                        let mut val = serde_json::to_value(&o)
                            .unwrap_or(serde_json::json!({"error": "serialize"}));
                        if let serde_json::Value::Object(ref mut m) = val {
                            m.insert("buildEvents".to_string(), serde_json::json!(events));
                        }
                        val
                    }
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_verify" => {
                // Build, then (only if the build compiled) lint — the two
                // checks a developer runs together. Both results are persisted
                // per target under their own kind so the platform list shows
                // them independently.
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let opts = BuildOptions {
                    mode: args
                        .get("mode")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    package: args
                        .get("package")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    platform: args
                        .get("platform")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    target: args
                        .get("target")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };
                let status_target = self
                    .resolve_status_target(path, opts.target.as_deref(), opts.platform.as_deref())
                    .await;

                // ── 1. Build ─────────────────────────────────────────────────
                let (build_out, mut build_events) =
                    match self.build_project_with_events(Path::new(path), &opts).await {
                        Ok(v) => v,
                        Err(e) => return serde_json::json!({ "error": e }),
                    };
                let has_file_diags = build_events
                    .iter()
                    .any(|e| e.get("file").and_then(|f| f.as_str()).is_some());
                if !has_file_diags {
                    build_events.append(&mut Self::parse_clang_output(&build_out.output));
                }
                let _ = self
                    .ingest_diagnostics(&build_events, "build", Path::new(path))
                    .await;
                let _ = self
                    .store_action_status_with_output(
                        "build",
                        path,
                        status_target.as_deref(),
                        build_out.success,
                        build_out.duration_secs,
                        &build_out.output,
                    )
                    .await;

                // ── 2. Lint (only meaningful once the code compiles) ─────────
                let mut lint_out: Option<BuildOutput> = None;
                let mut lint_events: Vec<serde_json::Value> = Vec::new();
                if build_out.success {
                    if let Ok((lo, mut le)) = self
                        .lint_project_streaming(Path::new(path), opts.platform.clone())
                        .await
                    {
                        if le.is_empty() {
                            le.append(&mut Self::parse_clang_output(&lo.output));
                        }
                        let _ = self.ingest_diagnostics(&le, "lint", Path::new(path)).await;
                        let _ = self
                            .store_action_status(
                                "lint",
                                path,
                                status_target.as_deref(),
                                lo.success,
                                lo.duration_secs,
                            )
                            .await;
                        lint_events = le;
                        lint_out = Some(lo);
                    }
                }

                let success =
                    build_out.success && lint_out.as_ref().map(|l| l.success).unwrap_or(true);
                let mut sections = vec![format!(
                    "=== Build ({}) ===\n{}",
                    if build_out.success { "ok" } else { "FAILED" },
                    build_out.output
                )];
                match &lint_out {
                    Some(lo) => sections.push(format!(
                        "=== Lint ({}) ===\n{}",
                        if lo.success { "ok" } else { "FAILED" },
                        lo.output
                    )),
                    None => sections.push(
                        "=== Lint ===\nskipped — the build failed, so there is no compile \
                         database to analyse"
                            .to_string(),
                    ),
                }
                let mut events = build_events;
                events.extend(lint_events);
                serde_json::json!({
                    "success": success,
                    "output": sections.join("\n\n"),
                    "command": "meson compile + lint",
                    "duration_secs": build_out.duration_secs
                        + lint_out.as_ref().map(|l| l.duration_secs).unwrap_or(0.0),
                    "exit_code": if success { 0 } else { 1 },
                    "buildEvents": events,
                    "steps": [
                        { "name": "build", "success": build_out.success },
                        { "name": "lint", "success": lint_out.as_ref().map(|l| l.success) },
                    ],
                })
            }
            "build_format" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                match self.format_project(Path::new(path)).await {
                    Ok(o) => {
                        serde_json::to_value(o).unwrap_or(serde_json::json!({"error": "serialize"}))
                    }
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_fix" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                match self.fix_project_streaming(Path::new(path)).await {
                    Ok((o, events)) => {
                        let mut val = serde_json::to_value(&o)
                            .unwrap_or(serde_json::json!({"error": "serialize"}));
                        if let serde_json::Value::Object(ref mut m) = val {
                            m.insert("buildEvents".to_string(), serde_json::json!(events.clone()));
                        }
                        // Persist post-fix diagnostics (usually no remaining events).
                        let _ = self
                            .ingest_diagnostics(&events, "fix", Path::new(path))
                            .await;
                        val
                    }
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_scaffold" => {
                let project_name = args
                    .get("project_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let build_system = args
                    .get("build_system")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let goal = args
                    .get("goal")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                // Empty platforms => single-target ("host") scaffold.
                let platforms: Vec<String> = args
                    .get("platforms")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| p.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["host".to_string()]);
                match self
                    .scaffold_build_config(
                        project_name,
                        build_system,
                        goal,
                        &platforms,
                        spire_core::build_types::ProjectStructure::Native,
                        false,
                    )
                    .await
                {
                    Ok(out) => serde_json::to_value(out).unwrap_or_else(
                        |_| serde_json::json!({ "error": "scaffold serialization" }),
                    ),
                    Err(e) => serde_json::json!({ "error": e }),
                }
            }
            "build_list_modules" => {
                serde_json::to_value(&self.capabilities).unwrap_or(serde_json::json!([]))
            }
            "build_dependency_docs" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let version = args.get("version").and_then(|v| v.as_str()).unwrap_or("");
                // Route to the module that owns the requested language, falling
                // back to Cargo (Rust) which is the primary supported runtime.
                let lang = args
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Rust");
                let module_tx = self
                    .capabilities
                    .iter()
                    .find(|c| {
                        c.language.eq_ignore_ascii_case(lang)
                            || c.build_system.eq_ignore_ascii_case(lang)
                    })
                    .and_then(|c| {
                        self.router
                            .get(c.config_files.first().map(|s| s.as_str()).unwrap_or(""))
                    })
                    .cloned()
                    .or_else(|| self.router.get("Cargo.toml").cloned());
                match module_tx {
                    Some(tx) => {
                        let (t, r) = oneshot::channel();
                        let _ = tx
                            .send(crate::build::BuildModuleMessage::CallTool {
                                tool_name: "get_dependency_docs".to_string(),
                                args: serde_json::json!({ "name": name, "version": version }),
                                reply_to: t,
                            })
                            .await;
                        match r.await {
                            Ok(v) => v,
                            Err(e) => {
                                serde_json::json!({ "error": format!("dependency docs response lost: {}", e) })
                            }
                        }
                    }
                    None => {
                        serde_json::json!({ "error": "no build module available for dependency docs" })
                    }
                }
            }
            // Applying a **design** to a library: the components it names, written as stubs.
            //
            // The decomposition is checked here rather than trusted — the same rules a composition is held
            // to — because the wizard may hand back an edited one, and a stub written from a design
            // that does not hold together is a component nobody asked for. What the library already has
            // is read once, so the plan is a snapshot of one moment rather than a view that shifts as
            // the plan is applied.
            "idf_apply_design" => {
                let root = args
                    .get("root")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let Some(application) = args.get("application").filter(|value| !value.is_null())
                else {
                    return serde_json::json!({
                        "error": "idf_apply_design: 'application' (the reviewed decomposition) is \
                                  required, and 'root' (the component library) with it"
                    });
                };
                if root.is_empty() {
                    return serde_json::json!({
                        "error": "idf_apply_design: 'root' has to be the component library the \
                                  design's components belong to"
                    });
                }
                let spec: crate::build::application_spec::ApplicationSpec =
                    match serde_json::from_value(application.clone()) {
                        Ok(spec) => spec,
                        Err(e) => {
                            return serde_json::json!({
                                "error": format!("idf_apply_design: 'application' is not an \
                                                  application spec: {e}")
                            })
                        }
                    };
                if let Err(problems) = crate::build::application_spec::validate(&spec) {
                    return serde_json::json!({
                        "error": format!(
                            "idf_apply_design: the decomposition does not hold together, so no \
                             component was written:\n  - {}",
                            problems.join("\n  - ")
                        )
                    });
                }
                let root = Path::new(&root);
                let present = crate::build::idf_projects::component_names(root);
                let plan = crate::build::application_spec::component_plan(&spec, |name| {
                    present.iter().any(|known| known == name)
                });
                crate::build::idf_projects::apply_component_plan(root, &plan)
            }

            // Adding a **component** to an ESP-IDF component library: its skeleton, as a stub whose
            // code the model fills in. The **kind** is stated by the caller and decides the shape —
            // a `driver` is a device on a bus and gets a seam, a `library` is pure code and gets a
            // plain unit test. A `bus` is required only for a driver, and means nothing for a
            // library: a bus passed for a library would be a contract that lies.
            "idf_add_component" => {
                let root = args
                    .get("root")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let kind_arg = args
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let bus = args.get("bus").and_then(|v| v.as_str()).unwrap_or_default();
                if root.is_empty() || name.is_empty() {
                    serde_json::json!({
                        "error": "idf_add_component: 'root' (the component library directory) and \
                                  'name' (the component) are required"
                    })
                } else {
                    match crate::build::idf_projects::ComponentKind::from_str(kind_arg) {
                        None => serde_json::json!({
                            "error": format!(
                                "idf_add_component: 'kind' has to be 'driver' (a device on a bus) or \
                                 'library' (pure code) — got '{}'. Nothing here guesses which: the \
                                 kind decides the skeleton, and it is not recoverable later.",
                                kind_arg.trim()
                            )
                        }),
                        Some(kind) => {
                            match crate::build::idf_projects::add_component(
                                std::path::Path::new(root),
                                name,
                                kind,
                                bus,
                            ) {
                                Ok(result) => result,
                                Err(e) => serde_json::json!({ "error": e }),
                            }
                        }
                    }
                }
            }

            // Removing one: the directory, and nothing else. Refuses when another component is
            // built on it, rather than editing that component's `REQUIRES` on the caller's behalf.
            "idf_remove_component" => {
                let root = args
                    .get("root")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if root.is_empty() || name.is_empty() {
                    serde_json::json!({
                        "error": "idf_remove_component: 'root' (the component library directory) and \
                                  'name' (the component) are required"
                    })
                } else {
                    match crate::build::idf_projects::remove_component(
                        std::path::Path::new(root),
                        name,
                    ) {
                        Ok(result) => result,
                        Err(e) => serde_json::json!({ "error": e }),
                    }
                }
            }

            // The ESP-IDF environment, tested and fixed. No model in the loop: what the doctor
            // reports is a fact about this machine, and the fix is `idf_tools.py`'s own installer.
            "idf_env_check" => {
                serde_json::to_value(crate::build::idf_env::doctor_idf_environment().await)
                    .unwrap_or(serde_json::json!({ "error": "idf_env_check serialization" }))
            }
            "idf_env_fix" => {
                serde_json::to_value(crate::build::idf_env::repair_idf_environment().await)
                    .unwrap_or(serde_json::json!({ "error": "idf_env_fix serialization" }))
            }

            "cpp_syntax_check" => {
                let path = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                match std::fs::read_to_string(path) {
                    Ok(content) => serde_json::to_value(
                        crate::build::generic_helpers::cpp_syntax_check(&content),
                    )
                    .unwrap_or(serde_json::json!({"error": "cpp_syntax_check serialization"})),
                    Err(e) => serde_json::json!({"error": format!("read {}: {e}", path)}),
                }
            }
            other => serde_json::json!({ "error": format!("Unknown build tool: {other}") }),
        }
    }

    /// Persist the latest per-platform action status under a graph config key
    /// `<kind>.last.<path>.<target>` (or `<kind>.last.<path>` when no target was
    /// selected) so the platform list / Build tab can show "last <kind>
    /// succeeded/failed" + duration PER PLATFORM. `kind` is one of
    /// "build" | "lint" | "test" | "clean"; each action runs per platform
    /// (rpi5/rock3c), so results MUST NOT overwrite each other.
    async fn store_action_status(
        &self,
        kind: &str,
        path: &str,
        target: Option<&str>,
        success: bool,
        duration_secs: f64,
    ) -> Result<(), String> {
        self.store_action_status_with_output(kind, path, target, success, duration_secs, "")
            .await
    }

    async fn store_action_status_with_output(
        &self,
        kind: &str,
        path: &str,
        target: Option<&str>,
        success: bool,
        duration_secs: f64,
        output: &str,
    ) -> Result<(), String> {
        let key = match target {
            Some(t) if !t.trim().is_empty() => format!("{kind}.last.{path}.{t}"),
            _ => format!("{kind}.last.{path}"),
        };
        let value = serde_json::json!({
            "path": path,
            "kind": kind,
            "success": success,
            "duration_secs": duration_secs,
            "timestamp": Utc::now().to_rfc3339(),
            "output": output,
        });
        let (tx, rx) = oneshot::channel();
        self.memory_graph_tx
            .send(MemoryGraphMessage::SetConfig {
                key,
                value,
                reply_to: tx,
            })
            .await
            .map_err(|e| format!("MemoryGraph channel closed: {e}"))?;
        rx.await
            .map_err(|e| format!("MemoryGraph response lost: {e}"))?
            .map_err(|e| format!("MemoryGraph store failed: {e}"))?;
        Ok(())
    }

    /// Resolve the target name an action actually ran for, so per-platform
    /// status lands under the SAME per-target key the UI reads back:
    ///   • the explicit `target` argument, when the caller selected a target;
    ///   • otherwise the target matching the requested `platform` (a platform /
    ///     domain selection still builds one specific executable).
    async fn resolve_status_target(
        &self,
        path: &str,
        target: Option<&str>,
        platform: Option<&str>,
    ) -> Option<String> {
        if let Some(t) = target.filter(|t| !t.trim().is_empty()) {
            return Some(t.to_string());
        }
        let plat = platform.filter(|p| !p.trim().is_empty())?;
        let md = self.get_analysis(path).await?;
        md.targets
            .iter()
            .find(|t| t.platform == plat)
            .map(|t| t.name.clone())
    }

    /// Resolve the normalized `build_spec` for the selected build target from
    /// stored analysis. When the caller selected a concrete target (name or
    /// platform), the matching `BuildTarget.build_spec` is returned so the
    /// module executes it directly; otherwise `None` (existing per-tool logic).
    fn resolve_build_spec(
        metadata: &BuildMetadata,
        opts: &BuildOptions,
    ) -> Option<spire_core::build_types::BuildSpec> {
        let target_matches = |t: &spire_core::build_types::BuildTarget| {
            if let Some(tname) = &opts.target {
                if !tname.is_empty() && t.name == *tname {
                    return true;
                }
            }
            if let Some(plat) = &opts.platform {
                if !plat.is_empty() && t.platform == *plat {
                    return true;
                }
            }
            false
        };
        metadata
            .targets
            .iter()
            .find(|t| target_matches(t))
            .and_then(|t| t.build_spec.clone())
    }

    /// The unified build tools exposed to the LLM.
    fn list_tools() -> Vec<spire_core::actors::ToolInfo> {
        let mut tools = vec![
            spire_core::actors::ToolInfo {
                name: "build_analyze".to_string(),
                description: "Analyze a project directory using its detected build system (Cargo, npm, Maven, CMake, etc.) and return structured metadata.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string", "description": "Project directory path" } },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_build".to_string(),
                description: "Build a project directory using its detected build system. Requires prior build_analyze.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project directory path" },
                        "mode": { "type": "string", "description": "Build mode: debug (default) or release" },
                        "package": { "type": "string", "description": "Optional workspace member/package name (Cargo: --package)" },
                        "platform": { "type": "string", "description": "Optional cross-platform target (e.g. host/rpi5) selecting build-<platform> Meson dir" },
                        "target": { "type": "string", "description": "Optional specific build target (e.g. Meson executable name like 'myapp-rpi') passed to meson compile -C <dir> <target>" }
                    },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_test".to_string(),
                description: "Run tests for a project directory using its detected build system. Requires prior build_analyze.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project directory path" },
                        "filter": { "type": "string", "description": "Optional test name filter" },
                        "platform": { "type": "string", "description": "Optional cross-platform target (e.g. host/rpi5) selecting which build-<platform> Meson dir to test in" }
                    },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_verify".to_string(),
                description: "Build a project and, when it compiles, lint it — the combined check a developer runs together. Requires prior build_analyze.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project directory path" },
                        "platform": { "type": "string", "description": "Optional cross-platform target (e.g. host/rpi5) selecting the build-<platform> Meson dir" },
                        "target": { "type": "string", "description": "Optional specific build target (e.g. Meson executable name)" }
                    },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_flash".to_string(),
                description: "Flash a built artifact onto an embedded board over USB (esp-idf: espflash) and report the result. Requires prior build_analyze and a successful build for that platform.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project root directory" },
                        "platform": { "type": "string", "description": "Platform registry id of the board to flash (e.g. esp32c6) — required, because it names the chip the tool talks to" },
                        "mode": { "type": "string", "description": "Build profile whose artifact is flashed: debug (default) or release" },
                        "package": { "type": "string", "description": "Optional package/workspace member whose binary is flashed" },
                        "artifact": { "type": "string", "description": "Optional path to the binary to flash; defaults to the one the build wrote for that platform" },
                        "port": { "type": "string", "description": "Optional serial port of the board (e.g. /dev/cu.usbserial-1234); defaults to the single USB serial device" }
                    },
                    "required": ["path", "platform"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_clean".to_string(),
                description: "Clean a project directory using its detected build system (removes build artifacts, keeps the configured build dir). Requires prior build_analyze.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project directory path" },
                        "platform": { "type": "string", "description": "Optional cross-platform target (e.g. host/rpi5) pinning the build-<platform> Meson dir to clean" }
                    },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_lint".to_string(),
                description: "Lint/analyze a project directory using its detected build system and return structured diagnostics. Requires prior build_analyze.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project directory path" },
                        "platform": { "type": "string", "description": "Optional cross-platform target (e.g. host/rpi5) selecting which build-<platform> Meson dir's compile database to analyze" }
                    },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_scaffold".to_string(),
                description: "Generate a minimal build-config + source stub for a new project via the registered build module (templates live with each language module). Returns build_file, build_content, source_file, source_content; caller writes the files.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "project_name": { "type": "string", "description": "Project/package name" },
                        "build_system": { "type": "string", "description": "Build system label or language, e.g. Cargo, Python, SwiftPM, Meson" },
                        "goal": { "type": "string", "description": "Natural-language goal for the scaffold" }
                    },
                    "required": ["project_name", "build_system"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_list_modules".to_string(),
                description: "List the registered build modules and their capabilities (config files, languages).".to_string(),
                input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            },
            spire_core::actors::ToolInfo {
                name: "build_format".to_string(),
                description: "Run the project's formatter (e.g. clang-format) via the language module.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project directory path" },
                        "language": { "type": "string", "description": "Language/module to route to" }
                    },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_fix".to_string(),
                description: "Apply auto-fixes for warnings/errors (cargo fix --allow-dirty) via the language module.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project directory path" },
                        "language": { "type": "string", "description": "Language/module to route to" }
                    },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "modify_code".to_string(),
                description: "Change existing code from a prompt: the model picks the files in scope, rewrites them, and a change is kept only if the project still builds and the tests that can run still pass — otherwise it is rolled back byte-for-byte. Target tests run only when a board is connected, and the result says which layer verification reached.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project or subproject directory path" },
                        "prompt": { "type": "string", "description": "What to change, in the user's own words" },
                        "platform": { "type": "string", "description": "Cross-platform target (e.g. rpi5) the change is verified against" },
                        "scope": { "type": "array", "items": { "type": "string" }, "description": "Files the model may consider; defaults to the project's sources" }
                    },
                    "required": ["path", "prompt"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "idf_component_edit".to_string(),
                description: "Write or change one component of an ESP-IDF component library, or one **unit** of an ESP-IDF application. What the model is given follows from the component's kind, which the component states in its own CMakeLists.txt: a `driver` gets the bus seam and the invariants a protocol keeps, a `library` gets the invariants pure code keeps, and an `actor` or a `stage` — a unit of an application — gets the **reviewed design** the application carries (its message and the peers it sends to, or the value it pulls and pushes, plus the wiring and the board facts) and the framework's own rules. A library's component also gets the library's SPIRE.md and the component's own header. Both get the same gate: the change is kept only if the component's host test still compiles and passes (no board, no chip, no IDF) — otherwise every file is rolled back byte-for-byte. Naming a chip adds the chip build as a second gate; without one the result says the chip build did not run.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "The component library directory, or — for a unit — the application directory that holds `components/<name>/` and states its design in `SPIRE.application.json` (or `composition.spire`)" },
                        "name": { "type": "string", "description": "The component or unit to write or change" },
                        "instruction": { "type": "string", "description": "What the user knows: for a driver, the datasheet material the protocol is written from; for a library, what the code should do and with what inputs and outputs; for a unit (an actor or a stage), what it should do with the message it receives, or the value it pulls" },
                        "platform": { "type": "string", "description": "A chip to build for as a second gate (e.g. esp32s3); omitted means the host test is the whole check" }
                    },
                    "required": ["root", "name"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "idf_apply_design".to_string(),
                description: "Apply a reviewed decomposition to an ESP-IDF component library: write the components the design names that the library does not have, as typed stubs whose code the model fills in — a `driver` gets a bus seam, a fake bus and a host-test harness, a `library` gets a plain unit test — and report what was written, what was already there, and where the design and the library *disagree* (a component the design says the library has and it does not, which is a person's call to resolve). Nothing of the design's composition is written here: the units, the wiring and the board facts belong to the application's `main/`.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "The component library directory (the project whose CMakeLists.txt says `set(SPIRE_PROJECT_STRUCTURE idf_library)`)" },
                        "application": { "type": "object", "description": "The reviewed application spec — the composition `createProject/ParseComposition` read, checked again here rather than trusted" }
                    },
                    "required": ["root", "application"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "idf_env_check".to_string(),
                description: "Test this machine's ESP-IDF environment and say what is wrong with it: where the install is, whether `idf.py` runs, whether `export.sh` activates, and which tools `idf_tools.py check` cannot find. Read-only — nothing is installed. Run it before blaming a chip build: it is the difference between \"the build failed\" and \"this machine has no working ESP-IDF\".".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            spire_core::actors::ToolInfo {
                name: "idf_env_fix".to_string(),
                description: "Repair the ESP-IDF environment by installing exactly the tools `idf_env_check` found missing, then testing again. Nothing else is touched — the same `idf_tools.py install` a person would run by hand. Needs network access (the tools come from dl.espressif.com). A build and a flash do not use the debug tools it installs; `export.sh` does, which is why their absence breaks a shell.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            spire_core::actors::ToolInfo {
                name: "idf_component_history".to_string(),
                description: "The recorded edits of one component of an ESP-IDF component library — or of one unit (an actor or a stage) of an ESP-IDF application — newest first: what was asked, whether the gate kept the change, how far verification reached, and which files moved. Read-only — the same records `idf_component_edit` echoes in its response, but fetched on demand (and beyond the inline window) for a history pane.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "The component library root" },
                        "name": { "type": "string", "description": "The component whose history is read" },
                        "limit": { "type": "integer", "description": "How many records to return, newest first (default 50, max 200)" }
                    },
                    "required": ["root", "name"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_autofix".to_string(),
                description: "Fix & Verify pipeline: compile, fix compile errors with LLM rewrites, lint, fix safely-fixable warnings (dead stores/unused code), and re-verify — keeping only edits that measurably help and rolling back any that do not, until the project builds cleanly or the round cap is reached. Every write is compile-verified; warnings that need judgement are reported, never rewritten.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Project or subproject directory path" },
                        "platform": { "type": "string", "description": "Cross-platform target (e.g. rpi5) whose build directory the fixes are verified against" },
                        "target": { "type": "string", "description": "Specific build target within the project" }
                    },
                    "required": ["path"]
                }),
            },
            spire_core::actors::ToolInfo {
                name: "build_dependency_docs".to_string(),
                description: "Fetch documentation (Markdown) for a dependency package via the language module (crates.io for Rust, npm registry, PyPI, etc.).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Dependency package name" },
                        "version": { "type": "string", "description": "Optional package version" },
                        "language": { "type": "string", "description": "Language/module to route to (e.g. Rust, JavaScript, Swift)" }
                    },
                    "required": ["name"]
                }),
            },
        ];
        tools.extend(Self::component_tool_definitions());
        tools
    }

    /// ESP-IDF component-library tools dispatched by [`Self::call_tool`].
    ///
    /// These MUST be advertised here: `build_default_registry` registers exactly
    /// the tools `ListTools` returns, so any tool handled by `call_tool` but
    /// omitted from this list is unreachable through `tools/call` — it falls
    /// through to the MCP catch-all and fails with "no tool found <name>".
    fn component_tool_definitions() -> Vec<spire_core::actors::ToolInfo> {
        fn t(
            name: &str,
            description: &str,
            properties: serde_json::Value,
            required: &[&str],
        ) -> spire_core::actors::ToolInfo {
            spire_core::actors::ToolInfo {
                name: name.to_string(),
                description: description.to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": properties,
                    "required": required,
                }),
            }
        }

        vec![
            // ESP-IDF component library: the two operations that grow and shrink it.
            t("idf_add_component", "Add a component to an ESP-IDF component library: its skeleton, as a typed stub whose code the model fills in. The kind is stated and decides the shape — a `driver` (a device on a bus) gets a bus seam, a fake bus and a host-test harness, and its protocol is written from the device's datasheet; a `library` (pure code: an algorithm, a filter, a codec) gets a plain unit test and no bus at all.",
              serde_json::json!({
                  "root": { "type": "string", "description": "The component library directory (the project whose CMakeLists.txt says `set(SPIRE_PROJECT_STRUCTURE idf_library)`)" },
                  "name": { "type": "string", "description": "The component, e.g. sps30 or rolling_average — becomes the component directory, its namespace and its class; for a driver, the device it speaks to" },
                  "kind": { "type": "string", "enum": ["driver", "library"], "description": "What this component is, stated rather than guessed. `driver`: one device on one bus — a protocol, which needs a seam so a host test can stand in for the device. `library`: pure code with no device, no bus and no board. It decides the whole skeleton, and it is written into the component's CMakeLists.txt." },
                  "bus": { "type": "string", "enum": ["i2c", "spi", "uart"], "description": "Required for `kind: driver` and meaningless for a `library`: which bus the device is on, a device fact the stub's REQUIRES follows from" }
              }),
              &["root", "name", "kind"]),
            t("idf_remove_component", "Remove a component from an ESP-IDF component library: its directory, and nothing else. Refuses when another component REQUIRES it, so a shared component is never deleted out from under its users.",
              serde_json::json!({
                  "root": { "type": "string", "description": "The component library directory" },
                  "name": { "type": "string", "description": "The component to remove, by directory name" }
              }),
              &["root", "name"]),
            // Helpers.
            t("cpp_syntax_check", "Run a lightweight C++ syntax check on a file.",
              serde_json::json!({ "path": { "type": "string" } }), &["path"]),
        ]
    }
}

#[async_trait]
impl Actor for BuildManagerActor {
    type Message = BuildManagerMessage;

    async fn handle(&mut self, msg: Self::Message) {
        match msg {
            BuildManagerMessage::AddModule {
                capability,
                module_tx,
            } => {
                self.add_module(capability, module_tx);
            }

            BuildManagerMessage::AddPlatformModule {
                os,
                capability,
                module_tx,
            } => {
                self.add_platform_module(os, capability, module_tx);
            }

            BuildManagerMessage::AnalyzeProject {
                path,
                config_file,
                reply_to,
            } => {
                let result = self.analyze_project(&path, config_file.as_deref()).await;
                let _ = reply_to.send(result);
            }

            BuildManagerMessage::BuildProject {
                path,
                opts,
                reply_to,
            } => {
                let result = self.build_project(&path, &opts).await;
                let _ = reply_to.send(result);
            }

            BuildManagerMessage::TestProject {
                path,
                opts,
                reply_to,
            } => {
                // Legacy manager-level path (no platform context); the
                // platform-aware entry point is the `build_test` tool.
                let result = self.test_project(&path, &opts, None).await;
                let _ = reply_to.send(result);
            }

            BuildManagerMessage::GetAnalysis { path, reply_to } => {
                let result = self.get_analysis(&path).await;
                let _ = reply_to.send(result);
            }

            BuildManagerMessage::ListModules { reply_to } => {
                let _ = reply_to.send(self.capabilities.clone());
            }

            BuildManagerMessage::CallTool {
                tool_name,
                args,
                reply_to,
            } => {
                // Isolate tool panics: a panic would otherwise unwind this actor
                // task and drop `reply_to`, surfacing to the caller as
                // "backend response error: channel closed" AND killing the
                // BuildManager for the rest of the session (every later build
                // call then fails too). Catch it, log it, and answer with an
                // error instead.
                let result = match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                    self.call_tool(&tool_name, args),
                ))
                .await
                {
                    Ok(v) => v,
                    Err(payload) => {
                        let msg = payload
                            .downcast_ref::<&str>()
                            .map(|s| (*s).to_string())
                            .or_else(|| payload.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown panic".to_string());
                        tracing::error!("BuildManager: tool '{tool_name}' panicked: {msg}");
                        serde_json::json!({ "error": format!("tool '{tool_name}' panicked: {msg}") })
                    }
                };
                let _ = reply_to.send(result);
            }

            BuildManagerMessage::CallModuleTool {
                build_system,
                tool_name,
                args,
                reply_to,
            } => {
                let result = self.call_module_tool(&build_system, &tool_name, args).await;
                let _ = reply_to.send(result);
            }

            BuildManagerMessage::SetEventTx { event_tx } => {
                self.event_tx = Some(event_tx);
            }

            BuildManagerMessage::SetEventLog { event_log_tx } => {
                self.event_log_tx = Some(event_log_tx);
            }

            BuildManagerMessage::ScaffoldBuildConfig {
                project_name,
                goal,
                build_file,
                platforms,
                structure,
                embedded,
                library,
                application,
                reply_to,
            } => {
                if let Some(tx) = self.router.get(&build_file).cloned() {
                    let (t, r) = oneshot::channel();
                    let _ = tx
                        .send(crate::build::BuildModuleMessage::ScaffoldBuildConfig {
                            project_name,
                            goal,
                            platforms,
                            structure: structure
                                .unwrap_or(spire_core::build_types::ProjectStructure::Native),
                            embedded,
                            library,
                            application,
                            reply_to: t,
                        })
                        .await;
                    match r.await {
                        Ok(result) => {
                            let _ = reply_to.send(result);
                        }
                        Err(e) => {
                            let _ = reply_to.send(Err(format!("scaffold response lost: {}", e)));
                        }
                    }
                } else {
                    let _ = reply_to.send(Err(format!(
                        "no build module owns config file '{}'",
                        build_file
                    )));
                }
            }

            BuildManagerMessage::ListTools { reply_to } => {
                let _ = reply_to.send(Self::list_tools());
            }

            BuildManagerMessage::ParseAndStoreSourceFile {
                file_path,
                reply_to,
            } => {
                let result = self.parse_and_store_source_file(&file_path).await;
                let _ = reply_to.send(result);
            }
        }
    }
}

// ============================================================================
// BuildEventLogActor — owns the incremental build-event log the FFI drains.
// ============================================================================

/// Messages for the actor-owned build-event log.
pub enum BuildEventLogMessage {
    /// Append one serialized build event.
    Record(serde_json::Value),
    /// Drain (remove + return) every accumulated event.
    Drain {
        reply_to: tokio::sync::oneshot::Sender<Vec<serde_json::Value>>,
    },
}

/// Owns the accumulated build events that the Swift UI streams incrementally.
///
/// Replaces the previous shared `Arc<Mutex<Vec<Value>>>` + `Notify` that the
/// streaming forwarders and the FFI both reached into directly. The
/// BuildManagerActor is occupied for the whole duration of a streaming
/// build/lint/fix, so event delivery cannot go through its mailbox; this small
/// dedicated actor serializes appends and drains without a `Mutex` shared with
/// the FFI (the FFI only holds this actor's sender + a `Notify`).
pub struct BuildEventLogActor {
    pending: Vec<serde_json::Value>,
    notify: std::sync::Arc<tokio::sync::Notify>,
}

impl BuildEventLogActor {
    pub fn new(notify: std::sync::Arc<tokio::sync::Notify>) -> Self {
        Self {
            pending: Vec::new(),
            notify,
        }
    }
}

#[async_trait]
impl Actor for BuildEventLogActor {
    type Message = BuildEventLogMessage;

    async fn handle(&mut self, msg: Self::Message) {
        match msg {
            BuildEventLogMessage::Record(json) => {
                self.pending.push(json);
                self.notify.notify_one();
            }
            BuildEventLogMessage::Drain { reply_to } => {
                let _ = reply_to.send(std::mem::take(&mut self.pending));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::CmakeBuildModule;
    use crate::Actor;

    use spire_actor::ServiceRegistry;
    use std::sync::Arc;

    /// **The key the wizard's banner decodes must not drift.**
    ///
    /// `ScaffoldSpec.design_warning` is read by the Swift model *by name*
    /// (`Models/ScaffoldSpec.swift`) with `decodeIfPresent`, so a renamed or dropped key is not an
    /// error there — it is a banner that never appears, which is exactly the silence this field exists
    /// to end. So the wire shape is pinned here instead: snake_case, and absent when there is nothing
    /// to say.
    #[test]
    fn a_scaffold_report_crosses_the_wire_under_the_key_the_wizard_reads() {
        let quiet = serde_json::to_value(ScaffoldSpec::default()).unwrap();
        assert!(
            quiet.get("design_warning").is_none(),
            "nothing to say is said by absence, not by null: {quiet}"
        );

        let told = ScaffoldSpec {
            design_warning: Some("the tree's design decides".to_string()),
            ..ScaffoldSpec::default()
        };
        let json = serde_json::to_value(&told).unwrap();
        assert_eq!(
            json["design_warning"].as_str(),
            Some("the tree's design decides"),
            "the wizard reads this key by name: {json}"
        );
        assert_eq!(
            serde_json::from_value::<ScaffoldSpec>(json)
                .unwrap()
                .design_warning,
            told.design_warning,
            "and it round-trips, so the spec the fill hands back still carries it"
        );

        // A core older than the field — no key at all — still decodes, as `None`.
        let mut without = serde_json::to_value(&told).unwrap();
        without.as_object_mut().unwrap().remove("design_warning");
        assert_eq!(
            serde_json::from_value::<ScaffoldSpec>(without)
                .unwrap()
                .design_warning,
            None,
            "the field is optional on the wire as well as in the type"
        );
    }

    /// Verify is invoked with a SUBPROJECT path, but diagnostics are stored under
    /// the project root — the scope root must be resolved by walking up to the
    /// project marker, or the previous run's diagnostics are never superseded and
    /// the Build tab keeps showing errors that no longer reproduce.
    #[test]
    fn diagnostics_scope_root_walks_up_to_the_project_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let sub = root.join("app");
        std::fs::create_dir_all(&sub).unwrap();

        assert_eq!(
            BuildManagerActor::diagnostics_scope_root(&sub),
            root,
            "a subproject path must resolve to the project root"
        );
        assert_eq!(
            BuildManagerActor::diagnostics_scope_root(&root),
            root,
            "the root resolves to itself"
        );
        // No marker anywhere → fall back to the path we were given (so a match
        // can still be attempted rather than silently scoping to nothing).
        let loose = tmp.path().join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        assert_eq!(BuildManagerActor::diagnostics_scope_root(&loose), loose);
    }

    /// Diagnostics recorded by a BUILD carry the compiler's verbatim path, and
    /// ninja makes it relative to the build dir (`../app/main.cpp`). Those MUST be
    /// treated as this project's: otherwise the supersede never clears them and
    /// stale build errors get re-reported by "Fix & Verify" run after run.
    #[test]
    fn diagnostic_in_project_accepts_build_dir_relative_paths() {
        let root = "/Users/me/ai-traps";
        // Relative (ninja/clang): this project's by construction.
        assert!(BuildManagerActor::diagnostic_in_project(
            "../app/main.cpp",
            root
        ));
        assert!(BuildManagerActor::diagnostic_in_project(
            "../hal/implementations/rock3c/camera_hal_rk.cpp",
            root
        ));
        // Absolute under the project: ours.
        assert!(BuildManagerActor::diagnostic_in_project(
            "/Users/me/ai-traps/build-rpi5/../app/main.cpp",
            root
        ));
        assert!(BuildManagerActor::diagnostic_in_project(
            "/Users/me/ai-traps/app/main.cpp",
            root
        ));
        // Absolute elsewhere: never deleted from a (possibly shared) graph.
        assert!(!BuildManagerActor::diagnostic_in_project(
            "/Users/me/other-project/main.cpp",
            root
        ));
        assert!(!BuildManagerActor::diagnostic_in_project(
            "/usr/include/stdio.h",
            root
        ));
        // A shared prefix is not a parent: this was a bare `starts_with` and would
        // have deleted a sibling project's diagnostics.
        assert!(!BuildManagerActor::diagnostic_in_project(
            "/Users/me/ai-traps-2/main.cpp",
            root
        ));
        // A trailing slash on the root must not matter.
        assert!(BuildManagerActor::diagnostic_in_project(
            "/Users/me/ai-traps/x.cpp",
            "/Users/me/ai-traps/"
        ));
        // Empty paths are never a match.
        assert!(!BuildManagerActor::diagnostic_in_project("", root));
        assert!(!BuildManagerActor::diagnostic_in_project("   ", root));
    }

    /// The same directory can be spelled two ways: macOS reports a compiler path
    /// through `/private/var` for a root created under `/var`, and a symlinked
    /// workspace does the same. If the stale-diagnostic sweep does not see through
    /// that, the errors it was meant to clear survive and Fix & Verify re-reports a
    /// defect it has just fixed.
    #[cfg(unix)]
    #[test]
    fn diagnostic_in_project_resolves_the_same_directory_through_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let file = real.join("main.cpp");
        std::fs::write(&file, "int main() { return 0; }\n").unwrap();

        assert!(
            BuildManagerActor::diagnostic_in_project(
                file.to_str().unwrap(),
                link.to_str().unwrap()
            ),
            "the compiler's path and the caller's path name the same directory"
        );
    }

    /// A `fatal error:` line must yield a CLEAN path: a missing header is the most
    /// common cross-build failure, and a file recorded as `../x.cpp:41` (line
    /// number glued on) can never be located — so the fix loop silently skipped
    /// exactly the errors it was built to repair.
    #[test]
    fn fatal_error_lines_parse_to_a_resolvable_relative_path() {
        let output = "ninja: Entering directory `/proj/build-rpi5'\n\
                      [1/3] Compiling C++ object rock3c/libfoo.a.p/x.cpp.o\n\
                      ../hal/implementations/rock3c/camera_hal_rk.cpp:41:10: fatal error: 'rockchip/rk_mpi.h' file not found\n\
                      1 error generated.\n";
        let events = BuildManagerActor::parse_clang_output(output);
        assert_eq!(events.len(), 1, "one diagnostic: {events:?}");

        let event = &events[0];
        assert_eq!(
            event["file"], "../hal/implementations/rock3c/camera_hal_rk.cpp",
            "the path must not carry the line number: {event:?}"
        );
        assert_eq!(event["line_number"], 41);
        assert_eq!(event["column"], 10);
        assert_eq!(
            event["level"], "error",
            "a fatal error is an error, never a warning"
        );
        assert!(
            BuildManagerActor::diagnostic_in_project(
                event["file"].as_str().unwrap_or_default(),
                "/proj"
            ),
            "the recorded path must count as this project's, or the supersede skips it"
        );

        // The plain "error:" form must parse identically.
        let plain =
            BuildManagerActor::parse_clang_output("../app/main.cpp:27:5: error: no member\n");
        assert_eq!(plain.len(), 1, "{plain:?}");
        assert_eq!(plain[0]["file"], "../app/main.cpp");
        assert_eq!(plain[0]["line_number"], 27);
        assert_eq!(plain[0]["column"], 5);
        assert_eq!(plain[0]["level"], "error");
    }

    // Serializes tests that mutate the PROCESS-GLOBAL `SPIRE_PLATFORM_DIR`
    // env var (the platform registry seed). Cargo runs tests in parallel, so
    // two registry-dependent tests must never set it concurrently — the last
    // writer would break the other's `Platform::from_registry` lookup.
    // Shared across the crate via `crate::PLATFORM_DIR_TEST_LOCK` so the
    // cargo/meson scaffold tests (readers) serialize against these writers.

    /// Sets `SPIRE_PLATFORM_DIR` for a fixture and restores the ambient value
    /// on drop — a stale var pointing at a deleted fixture dir would break any
    /// registry-reading test that runs later in this process.
    struct SpirePlatformDirGuard {
        previous: Option<String>,
    }
    impl SpirePlatformDirGuard {
        fn set(dir: impl AsRef<std::path::Path>) -> Self {
            let previous = std::env::var("SPIRE_PLATFORM_DIR").ok();
            std::env::set_var("SPIRE_PLATFORM_DIR", dir.as_ref());
            Self { previous }
        }
    }
    impl Drop for SpirePlatformDirGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(prev) => std::env::set_var("SPIRE_PLATFORM_DIR", prev),
                None => std::env::remove_var("SPIRE_PLATFORM_DIR"),
            }
        }
    }

    /// The capability the ESP32 module advertises: it claims **no** config file (routing is by
    /// platform) and it is the one module that flashes.
    fn esp_module_capability() -> ModuleCapability {
        ModuleCapability {
            name: "esp-idf".to_string(),
            config_files: Vec::new(),
            build_system: "Cargo (esp-idf)".to_string(),
            language: "Rust".to_string(),
            source_extensions: vec!["rs".to_string()],
            mcp_servers: Vec::new(),
            supports_flash: true,
            supports_clean: false,
            supports_lint: false,
            supports_format: false,
            supports_fix: false,
        }
    }

    /// The capability a platform module advertises: it claims **no** config file (routing is by
    /// platform) and it is one of the modules that flashes. The rp2040 module's, as it registers
    /// itself — the two platform modules must not shadow each other.
    fn rp2040_module_capability() -> ModuleCapability {
        ModuleCapability {
            name: "rp2040".to_string(),
            config_files: Vec::new(),
            build_system: "Cargo (rp2040)".to_string(),
            language: "Rust".to_string(),
            source_extensions: vec!["rs".to_string()],
            mcp_servers: Vec::new(),
            supports_flash: true,
            supports_clean: false,
            supports_lint: false,
            supports_format: false,
            supports_fix: false,
        }
    }

    /// The capability of a config-file module: owns `Cargo.toml`, flashes nothing.
    fn cargo_capability() -> ModuleCapability {
        ModuleCapability {
            name: "cargo".to_string(),
            config_files: vec!["Cargo.toml".to_string()],
            build_system: "Cargo".to_string(),
            language: "Rust".to_string(),
            source_extensions: vec!["rs".to_string()],
            mcp_servers: Vec::new(),
            supports_clean: true,
            supports_lint: true,
            supports_format: true,
            supports_fix: true,
            supports_flash: false,
        }
    }

    /// The point of platform routing: an ESP32 build must reach the platform module, and
    /// **every other project must keep reaching its config module**.
    ///
    /// The second half is the one that matters. A routing change that quietly claimed
    /// ordinary Rust projects would be far worse than having no routing at all — that is the
    /// failure this whole mechanism exists to avoid, so it is asserted, not assumed.
    #[test]
    fn a_build_routes_by_platform_only_when_a_platform_module_exists() {
        let _guard = crate::PLATFORM_DIR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("platform dir");
        std::fs::write(
            dir.path().join("esp32c6.yaml"),
            "id: esp32c6\nname: ESP32-C6\nos: esp-idf\narchitecture:\n  \
             cpu_family: riscv\n  cpu: esp32c6\n  endian: little\n  \
             target_triple: riscv32imac-esp-espidf\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("rp2040.yaml"),
            "id: rp2040\nname: Raspberry Pi Pico\nos: rp2040\nfamily: rp2040\narchitecture:\n  \
             cpu_family: arm\n  cpu: rp2040\n  endian: little\n  \
             target_triple: thumbv6m-none-eabi\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("rpi5.yaml"),
            "id: rpi5\nname: Raspberry Pi 5\nos: linux\narchitecture:\n  \
             cpu_family: aarch64\n  cpu: armv8-a\n  endian: little\n  \
             target_triple: aarch64-linux-gnu\n",
        )
        .unwrap();
        let _env = SpirePlatformDirGuard::set(dir.path());

        let mut manager = BuildManagerActor::new(mpsc::channel(1).0);
        // The platform modules (as the ESP32 and rp2040 ones register themselves) — and note
        // they claim no config file, which is what makes the next registration possible.
        manager.add_platform_module(
            "esp-idf".to_string(),
            esp_module_capability(),
            mpsc::channel(1).0,
        );
        manager.add_platform_module(
            "rp2040".to_string(),
            rp2040_module_capability(),
            mpsc::channel(1).0,
        );
        manager.add_module(
            ModuleCapability {
                name: "cargo".to_string(),
                config_files: vec!["Cargo.toml".to_string()],
                build_system: "Cargo".to_string(),
                language: "Rust".to_string(),
                source_extensions: vec!["rs".to_string()],
                mcp_servers: Vec::new(),
                supports_clean: true,
                supports_lint: true,
                supports_format: true,
                supports_fix: true,
                supports_flash: false,
            },
            mpsc::channel(1).0,
        );

        // The esp-idf case: the platform module wins even though cargo owns the config file.
        assert_eq!(
            manager.route_for("Cargo.toml", Some("esp32c6")),
            BuildRoute::Platform("esp-idf".to_string())
        );

        // And the second platform module is reached just as directly: routing is by `os`, so
        // neither platform can shadow the other however many there are.
        assert_eq!(
            manager.route_for("Cargo.toml", Some("rp2040")),
            BuildRoute::Platform("rp2040".to_string())
        );

        // Everywhere else, nothing changes — including for a Linux *cross* target, which is
        // also a Cargo project with a platform set.
        assert_eq!(
            manager.route_for("Cargo.toml", Some("rpi5")),
            BuildRoute::Config("Cargo.toml".to_string())
        );
        assert_eq!(
            manager.route_for("Cargo.toml", None),
            BuildRoute::Config("Cargo.toml".to_string())
        );
        // An unknown platform id must not swallow the route either.
        assert_eq!(
            manager.route_for("Cargo.toml", Some("no-such-board")),
            BuildRoute::Config("Cargo.toml".to_string())
        );
    }

    /// **A dashboard build of an ESP-IDF tree names no platform** — the wizard that threaded the
    /// chip through a form is gone — so the chip is read back from the tree itself, and the
    /// request reaches the module that can actually build it.
    ///
    /// This is the gap the creation flow hid: it passes the wizard's chip as `platform`, so the
    /// same build works there and, without this, falls to the cmake module when clicked from the
    /// dashboard. Nothing else may change: a caller that names a platform keeps it, and a chip no
    /// registered module answers for leaves the options exactly as they were.
    #[test]
    fn a_builds_chip_is_read_back_from_the_tree_when_no_platform_is_named() {
        let _guard = crate::PLATFORM_DIR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("platform dir");
        std::fs::write(
            dir.path().join("esp32s3.yaml"),
            "id: esp32s3\nname: ESP32-S3\nos: esp-idf\narchitecture:\n  \
             cpu_family: xtensa\n  cpu: esp32s3\n  endian: little\n  \
             target_triple: xtensa-esp32s3-elf\n",
        )
        .unwrap();
        let _env = SpirePlatformDirGuard::set(dir.path());

        let mut manager = BuildManagerActor::new(mpsc::channel(1).0);
        manager.add_platform_module(
            "esp-idf".to_string(),
            esp_module_capability(),
            mpsc::channel(1).0,
        );

        let tree = tempfile::tempdir().expect("project tree");
        std::fs::write(
            tree.path().join("sdkconfig"),
            "CONFIG_IDF_TARGET=\"esp32s3\"\n",
        )
        .unwrap();

        let derived = manager.opts_with_declared_platform(tree.path(), &BuildOptions::default());
        assert_eq!(
            derived.platform.as_deref(),
            Some("esp32s3"),
            "the chip the tree was configured for is the platform the build runs with"
        );

        // A caller that named one is not second-guessed.
        let named = BuildOptions {
            platform: Some("esp32s3".to_string()),
            ..BuildOptions::default()
        };
        assert_eq!(
            manager
                .opts_with_declared_platform(tree.path(), &named)
                .platform
                .as_deref(),
            Some("esp32s3")
        );

        // A chip no registered module answers for changes nothing — the route stays whatever it
        // was, rather than becoming "No module registered for platform".
        std::fs::write(
            tree.path().join("sdkconfig"),
            "CONFIG_IDF_TARGET=\"esp32c6\"\n",
        )
        .unwrap();
        assert_eq!(
            manager
                .opts_with_declared_platform(tree.path(), &BuildOptions::default())
                .platform,
            None
        );
    }

    /// `flash` is gated by capability *and* by platform, and each refusal names a different
    /// gap — wrong id, no flash step for that os, or a module that does not flash.
    ///
    /// It matters that these are refusals and not errors from the module: a module that cannot
    /// flash has no reply to send, so routing to it would surface as a lost channel, which
    /// reads as a crash rather than as "this board has no USB flash step".
    #[test]
    fn flash_routes_only_to_a_module_that_declares_it() {
        let _guard = crate::PLATFORM_DIR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("platform dir");
        std::fs::write(
            dir.path().join("esp32c6.yaml"),
            "id: esp32c6\nname: ESP32-C6\nos: esp-idf\narchitecture:\n  \
             cpu_family: riscv\n  cpu: esp32c6\n  endian: little\n  \
             target_triple: riscv32imac-esp-espidf\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("rp2040.yaml"),
            "id: rp2040\nname: Raspberry Pi Pico\nos: rp2040\nfamily: rp2040\narchitecture:\n  \
             cpu_family: arm\n  cpu: rp2040\n  endian: little\n  \
             target_triple: thumbv6m-none-eabi\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("rpi5.yaml"),
            "id: rpi5\nname: Raspberry Pi 5\nos: linux\narchitecture:\n  \
             cpu_family: aarch64\n  cpu: armv8-a\n  endian: little\n  \
             target_triple: aarch64-linux-gnu\n",
        )
        .unwrap();
        let _env = SpirePlatformDirGuard::set(dir.path());

        let mut manager = BuildManagerActor::new(mpsc::channel(1).0);
        manager.add_platform_module(
            "esp-idf".to_string(),
            esp_module_capability(),
            mpsc::channel(1).0,
        );
        manager.add_platform_module(
            "rp2040".to_string(),
            rp2040_module_capability(),
            mpsc::channel(1).0,
        );
        manager.add_module(cargo_capability(), mpsc::channel(1).0);

        // A flash-capable platform module is routed exactly as a build would be.
        assert_eq!(
            manager.route_for_flash("Cargo.toml", "esp32c6"),
            Ok(BuildRoute::Platform("esp-idf".to_string()))
        );
        assert_eq!(
            manager.route_for_flash("Cargo.toml", "rp2040"),
            Ok(BuildRoute::Platform("rp2040".to_string()))
        );

        // A board with no flash step: the message names both the platform and its os, so a
        // reader can tell a typo'd id from "this board is not flashed over USB".
        let err = manager
            .route_for_flash("Cargo.toml", "rpi5")
            .expect_err("a linux platform has no flash step");
        assert!(err.contains("rpi5") && err.contains("linux"), "{err}");

        // An id that is not in the registry: the same wording `run_idf_flash` uses, so the
        // message does not depend on how far the request got.
        let err = manager
            .route_for_flash("Cargo.toml", "no-such-board")
            .expect_err("an unknown platform cannot be flashed");
        assert!(err.contains("unknown platform"), "{err}");

        // Same platform, but the module says it does not flash: refused before routing.
        let mut no_flash = BuildManagerActor::new(mpsc::channel(1).0);
        let mut cap = esp_module_capability();
        cap.supports_flash = false;
        no_flash.add_platform_module("esp-idf".to_string(), cap, mpsc::channel(1).0);
        no_flash.add_module(cargo_capability(), mpsc::channel(1).0);
        let err = no_flash
            .route_for_flash("Cargo.toml", "esp32c6")
            .expect_err("a module without flash must be refused up front");
        assert!(err.contains("not supported for build system"), "{err}");
    }

    /// `build_build` — the **streaming** path a UI Build click and a `build_build` tool call
    /// share — must honour the platform when routing, exactly as the batch path does.
    ///
    /// Regression test for a bug found on hardware: the streaming path looked the module up
    /// with `platform: None`, so an ESP32 build went to the **cargo** module while the batch
    /// `build_project` sent the same request to the esp module. Two paths, two answers — and
    /// the wrong one is a host `cargo build` that "succeeds" without producing firmware.
    ///
    /// Both modules are fakes that stamp a distinct `command`, so the assertion is about *which
    /// module ran*, not about a build. The stored analysis is faked at the channel boundary
    /// (`GetConfig`), which is what keeps this a unit test: no MemoryGraph, no toolchain, no
    /// board.
    #[tokio::test]
    async fn build_build_routes_by_platform_not_to_the_config_owners_module() {
        let _guard = crate::PLATFORM_DIR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let reg = tempfile::tempdir().unwrap();
        std::fs::write(
            reg.path().join("esp32c6.yaml"),
            "id: esp32c6\nname: ESP32-C6\nos: esp-idf\narchitecture:\n  \
             cpu_family: riscv\n  cpu: esp32c6\n  endian: little\n  \
             target_triple: riscv32imac-esp-espidf\n",
        )
        .unwrap();
        let _env = SpirePlatformDirGuard::set(reg.path());

        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_string_lossy().to_string();

        // `get_analysis` asks the graph for the manager's own key; answer it with the analysis
        // an `AnalyzeProject` would have stored, and swallow anything else.
        let analysis_key = format!("build.analysis.{root}");
        let stored = serde_json::to_value(BuildMetadata {
            config_files: vec!["Cargo.toml".to_string()],
            ..Default::default()
        })
        .unwrap();
        let (mg_tx, mut mg_rx) = mpsc::channel::<MemoryGraphMessage>(8);
        tokio::spawn(async move {
            while let Some(msg) = mg_rx.recv().await {
                match msg {
                    MemoryGraphMessage::GetConfig { key, reply_to } => {
                        let value = (key == analysis_key).then(|| stored.clone());
                        let _ = reply_to.send(Ok(value));
                    }
                    MemoryGraphMessage::SetConfig { reply_to, .. } => {
                        let _ = reply_to.send(Ok(()));
                    }
                    _ => {}
                }
            }
        });

        let mut manager = BuildManagerActor::new(mg_tx);

        // The platform module — and the command it stamps is what proves it was chosen.
        let (esp_tx, mut esp_rx) = mpsc::channel::<BuildModuleMessage>(8);
        tokio::spawn(async move {
            while let Some(msg) = esp_rx.recv().await {
                if let BuildModuleMessage::BuildStreaming { reply_to, .. } = msg {
                    let _ = reply_to.send(Ok(BuildOutput {
                        success: true,
                        command: "esp-build".to_string(),
                        ..Default::default()
                    }));
                }
            }
        });
        manager.add_platform_module("esp-idf".to_string(), esp_module_capability(), esp_tx);

        // The module that owns `Cargo.toml`: registered, and must NOT be the one asked.
        let (cargo_tx, mut cargo_rx) = mpsc::channel::<BuildModuleMessage>(8);
        tokio::spawn(async move {
            while let Some(msg) = cargo_rx.recv().await {
                if let BuildModuleMessage::BuildStreaming { reply_to, .. } = msg {
                    let _ = reply_to.send(Ok(BuildOutput {
                        success: true,
                        command: "cargo-build".to_string(),
                        ..Default::default()
                    }));
                }
            }
        });
        manager.add_module(cargo_capability(), cargo_tx);

        let opts = BuildOptions {
            platform: Some("esp32c6".to_string()),
            mode: "release".to_string(),
            ..Default::default()
        };
        let (output, _events) = manager
            .build_project_with_events(std::path::Path::new(&root), &opts)
            .await
            .expect("the routed module answers");

        assert_eq!(
            output.command, "esp-build",
            "the esp-idf module must build for the esp platform, not the Cargo.toml owner"
        );
    }

    /// Every action the UI's action row can invoke MUST be advertised by
    /// `list_tools()`: `build_default_registry` registers exactly the tools
    /// `ListTools` returns, so a tool handled by `call_tool` but missing there
    /// is silently unreachable through `tools/call` (it falls through to the
    /// MCP catch-all and fails instantly, on every target).
    ///
    /// Regression guard for build_clean / build_lint / build_format, and for the
    /// ESP-IDF environment pair (`idf_env_check` / `idf_env_fix`) — the app's own
    /// diagnosis of an install that is present but broken, which a person reaches
    /// from the same action row.
    #[test]
    fn ui_build_actions_are_registered_tools() {
        let names: Vec<String> = BuildManagerActor::list_tools()
            .into_iter()
            .map(|t| t.name)
            .collect();
        for name in [
            "build_analyze",
            "build_build",
            "build_verify",
            "build_test",
            "build_flash",
            "build_clean",
            "build_lint",
            "build_format",
            "build_fix",
            "idf_env_check",
            "idf_env_fix",
        ] {
            assert!(
                names.iter().any(|n| n == name),
                "tool '{name}' is handled by call_tool but missing from list_tools(), \
                 so tools/call cannot reach it. Registered: {names:?}"
            );
        }
    }

    /// **Applying a design** is advertised, because a tool a model cannot discover is a tool that does
    /// not exist: `build_default_registry` registers exactly what `ListTools` returns. This pins the
    /// half that can be checked here — the advertisement and the arguments it needs.
    #[test]
    fn the_apply_design_tool_is_advertised_with_its_required_arguments() {
        let tools = BuildManagerActor::list_tools();
        let tool = tools
            .iter()
            .find(|t| t.name == "idf_apply_design")
            .unwrap_or_else(|| {
                panic!(
                    "`idf_apply_design` must be advertised or the model cannot reach it. Registered: {:?}",
                    tools.iter().map(|t| &t.name).collect::<Vec<_>>()
                )
            });
        let required: Vec<&str> = tool.input_schema["required"]
            .as_array()
            .expect("required is an array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        for arg in ["root", "application"] {
            assert!(
                required.contains(&arg),
                "'{arg}' is required by idf_apply_design, so the tool must say so: {required:?}"
            );
        }
    }

    /// **Applying a design** to a library, through the tool: the components it names are written, what
    /// was already there is reported, a disagreement between the design and the library is carried
    /// through rather than resolved, and a decomposition that does not hold together is refused
    /// *before* anything is written.
    #[test]
    fn applying_a_design_writes_the_components_it_names() {
        use crate::build::application_spec::{examples, parse_spec};
        use crate::build::idf_projects::{component_kind, ComponentKind};

        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = BuildManagerActor::new(mpsc::channel(1).0);
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_library(root, "sensors");

        let spec = parse_spec(examples::PM25_METER).expect("the worked example parses");
        let args = serde_json::json!({
            "root": root.to_string_lossy(),
            "application": spec,
        });
        let report = rt.block_on(manager.call_tool("idf_apply_design", args.clone()));

        // The two drivers the design asks to be written are written…
        let added: Vec<&str> = report["added"]
            .as_array()
            .expect("added")
            .iter()
            .filter_map(|entry| entry["component"].as_str())
            .collect();
        assert_eq!(added, vec!["sps30", "sht20"], "{report}");
        // …and their **role** became their own stated kind, with the bus a device fact in the manifest
        // rather than something inferred later.
        let manifest = std::fs::read_to_string(root.join("components/sps30/CMakeLists.txt"))
            .expect("the stub's manifest");
        assert!(
            manifest.contains("SPIRE_COMPONENT_KIND driver") && manifest.contains("esp_driver_i2c"),
            "{manifest}"
        );
        assert_eq!(component_kind(root, "sps30"), Some(ComponentKind::Driver));
        assert!(root.join("components/sps30/src/sps30.cpp").is_file());

        // The design says the library already has `rolling_average` and it does not: a disagreement,
        // stated rather than resolved by writing a component the design never asked to be written.
        let problems: Vec<&str> = report["problems"]
            .as_array()
            .expect("problems")
            .iter()
            .filter_map(|p| p.as_str())
            .collect();
        assert_eq!(problems.len(), 1, "{report}");
        assert!(
            problems[0].contains("already has 'rolling_average' and it does not"),
            "{report}"
        );
        assert!(!root.join("components/rolling_average").exists());

        // Applying the same design again writes nothing: the library now is what the design asked for,
        // which is what makes this safe to run twice.
        let again = rt.block_on(manager.call_tool("idf_apply_design", args));
        assert_eq!(
            again["added"].as_array().expect("added").len(),
            0,
            "{again}"
        );
        let present: Vec<&str> = again["present"]
            .as_array()
            .expect("present")
            .iter()
            .filter_map(|p| p.as_str())
            .collect();
        assert_eq!(present, vec!["sps30", "sht20"], "{again}");

        // A decomposition that does not hold together is refused, and nothing is written for it.
        let holed = serde_json::json!({
            "root": root.to_string_lossy(),
            "application": {
                "framework": "actors",
                "board": { "chip": "esp32s3", "bsp": "m5stack_core_s3" },
                "units": [ { "id": "ticker", "kind": "stage" } ],
            },
        });
        let refused = rt.block_on(manager.call_tool("idf_apply_design", holed));
        let error = refused["error"]
            .as_str()
            .unwrap_or_else(|| panic!("{refused}"));
        assert!(error.contains("does not hold together"), "{error}");
        assert!(
            !root.join("components/ticker").exists(),
            "a design that does not hold together writes nothing:\n{refused}"
        );
    }

    /// A component library at `root`, written from the scaffold — the ordinary starting point.
    fn write_library(root: &std::path::Path, name: &str) {
        let out = crate::build::idf_projects::library_scaffold(name, &[]).expect("scaffolds");
        for file in &out.files {
            let path = root.join(&file.path);
            std::fs::create_dir_all(path.parent().expect("a file has a parent")).unwrap();
            std::fs::write(&path, &file.content).unwrap();
        }
    }

    #[test]
    fn find_config_file_detects_known_names() {
        let mut manager = BuildManagerActor::new(mpsc::channel(1).0);
        manager.add_module(
            ModuleCapability {
                name: "cargo".to_string(),
                config_files: vec!["Cargo.toml".to_string()],
                build_system: "Cargo".to_string(),
                language: "Rust".to_string(),
                source_extensions: vec!["rs".to_string()],
                supports_clean: true,
                supports_lint: true,
                supports_format: true,
                supports_fix: true,
                supports_flash: false,
                mcp_servers: vec![],
            },
            mpsc::channel(1).0,
        );

        // Direct file path
        assert_eq!(
            manager.find_config_file(Path::new("Cargo.toml")),
            Some("Cargo.toml".to_string())
        );
        // Unknown file
        assert!(manager.find_config_file(Path::new("README.md")).is_none());
    }

    #[test]
    fn find_config_file_prefers_cargo_over_makefile_deterministically() {
        let mut manager = BuildManagerActor::new(mpsc::channel(1).0);
        let add = |manager: &mut BuildManagerActor, name: &str, config: &str| {
            manager.add_module(
                ModuleCapability {
                    name: name.to_string(),
                    config_files: vec![config.to_string()],
                    build_system: name.to_string(),
                    language: "x".to_string(),
                    source_extensions: Vec::new(),
                    supports_clean: false,
                    supports_lint: false,
                    supports_format: false,
                    supports_fix: false,
                    supports_flash: false,
                    mcp_servers: vec![],
                },
                mpsc::channel(1).0,
            );
        };
        add(&mut manager, "cargo", "Cargo.toml");
        add(&mut manager, "make", "Makefile");

        let tmp = tempfile::tempdir().unwrap();
        // A Cargo workspace root that also carries a Makefile wrapper.
        std::fs::write(tmp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(tmp.path().join("Makefile"), "build:\n").unwrap();

        // Deterministic: the primary build config (Cargo) wins every time.
        for _ in 0..5 {
            assert_eq!(
                manager.find_config_file(tmp.path()),
                Some("Cargo.toml".to_string())
            );
        }
    }

    #[tokio::test]
    async fn analyze_routes_to_cmake_module() {
        let system = spire_actor::ActorSystem::new();
        let _registry = Arc::new(ServiceRegistry::new());
        let (mg_tx, _mg_rx) = mpsc::channel(1);

        // Spawn the cmake module + build manager.
        let (cmake_tx, _cmake_handle) = {
            let (tx, rx) = mpsc::channel::<BuildModuleMessage>(8);
            CmakeBuildModule::new().spawn(rx);
            (tx, ())
        };
        let (bm_tx, _bm_handle) = system.spawn(BuildManagerActor::new(mg_tx));

        // Register the module (as the FFI bootstrap would).
        let (t, r) = oneshot::channel();
        cmake_tx
            .send(BuildModuleMessage::DescribeCapabilities { reply_to: t })
            .await
            .unwrap();
        let cap = r.await.unwrap();
        bm_tx
            .send(BuildManagerMessage::AddModule {
                capability: cap,
                module_tx: cmake_tx,
            })
            .await
            .unwrap();

        // List modules — should contain the cmake module.
        let (t, r) = oneshot::channel();
        bm_tx
            .send(BuildManagerMessage::ListModules { reply_to: t })
            .await
            .unwrap();
        let modules = r.await.unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "cmake");
    }

    #[tokio::test]
    async fn extension_router_registers_source_extensions() {
        let mut manager = BuildManagerActor::new(mpsc::channel(1).0);
        manager.add_module(
            ModuleCapability {
                name: "cargo".to_string(),
                config_files: vec!["Cargo.toml".to_string()],
                build_system: "Cargo".to_string(),
                language: "Rust".to_string(),
                source_extensions: vec!["rs".to_string()],
                supports_clean: true,
                supports_lint: true,
                supports_format: true,
                supports_fix: true,
                supports_flash: false,
                mcp_servers: vec![],
            },
            mpsc::channel(1).0,
        );

        assert!(manager.extension_router.contains_key("rs"));
        assert!(!manager.extension_router.contains_key("py"));
    }
}
