// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **Regenerate one component from its own stored spec** — through the app's own door, against the
//! real model, with the real gate.
//!
//! This is the harness the retrospective specs exist for. `idf_component_edit` is called with an
//! **empty instruction**, so every word the model is asked comes from somewhere else: the component's
//! description and invariants from its `SPIRE.component.yaml` (through the graph), its header and
//! `SPIRE.md` from the tree, and its protocol from `device-facts` — retrieved by the part the spec
//! names. The answer is then written and checked by the same `cmake`/`ctest` gate a person's edit
//! goes through, and a passing run re-ratifies the spec.
//!
//! Nothing here reimplements that path: the coordinator, the RAG actor, the LLM actor and the gate are
//! the app's. Only the wiring is this file's, and it is the wiring `tests/idf_component_edit_tests.rs`
//! establishes as the app's own.
//!
//! # Running it
//!
//! The app must be quit first (it owns `idf/.spire/data`), and the LLM key has to be the one the app
//! uses — `~/.spire/spire-esp32/llm-config.json`, or `SPIRE_APP_NAME` pointing at another scope.
//!
//! ```text
//! cargo run -p spire-esp32 --example regenerate_component -- sht20   # one component
//! cargo run -p spire-esp32 --example regenerate_component -- all     # every editable one
//! ```
//!
//! `SPIRE_COMPONENT` names the component when no argument is given (default `sht20`);
//! `SPIRE_INSTRUCTION` overrides the spec's description with words of your own, which is what an
//! ordinary edit does — for **one** component (`all` refuses it, because the audit asks each
//! component its own question). `SPIRE_IDF_ROOT` overrides the project root. A run **rewrites the
//! component's files** and, when it passes, its spec — take a copy first if that matters.
//!
//! # `all`: the spec audit
//!
//! `all` walks every component the door will write — each directory under `components/` that states a
//! `SPIRE.component.yaml` and is not one of the framework's own (`toolkit`, `ramen`, `actors`) — in
//! sorted order, one run at a time, with the same empty instruction. Ten runs share one graph and one
//! ingested corpus, because re-initialising either per component would measure the harness rather
//! than the specs. `components/` is copied to a `tar` in the temp directory first, on top of the
//! door's own per-run rollback.
//!
//! A component that fails its gate is rolled back by the door itself and the batch **carries on**;
//! the exit code is non-zero iff any failed. That is the audit: a pass says the component's spec —
//! description, invariants, facts, header and `SPIRE.md` — carried everything needed to rewrite it,
//! and a failure names a spec that does not yet. The two libraries (`lvgl_display`,
//! `rolling_average`) have no `device-facts` document, so their prompts stand on the spec alone: the
//! strictest runs in the set.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use spire_actor::registry::ServiceRegistry;
use spire_actor::ActorSystem;
use spire_core::actors::rag::{EmbedderService, RagActor, RagMessage};
use spire_core::actors::MemoryGraphMessage as MgMsg;
use spire_core::models::embedding::Embedder;
use spire_core::subsystems::graph::memory_graph::MemoryGraphActor;
use spire_esp32::actors::{
    rag_bundle, ChatActor, CoordinatorActor, CoordinatorMessage, FfiSharedState, LlmActor,
    McpClientActor, SystemActor, ToolsActor,
};
use tokio::sync::{mpsc, oneshot};

/// The library being regenerated: `SPIRE_IDF_ROOT`, else `<repo>/idf`.
fn idf_root() -> Result<PathBuf> {
    match std::env::var("SPIRE_IDF_ROOT") {
        Ok(root) if !root.trim().is_empty() => Ok(PathBuf::from(root)),
        _ => {
            let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let root = here
                .parent()
                .and_then(Path::parent)
                .map(|repo| repo.join("idf"))
                .context("cannot locate the repository's idf/ directory")?;
            if !root.is_dir() {
                bail!(
                    "{} is not a directory — set SPIRE_IDF_ROOT to the component library",
                    root.display()
                );
            }
            Ok(root)
        }
    }
}

/// A copy of `idf/components` before a batch run.
///
/// The door already rolls a component back when its gate refuses, so this is not the safety net that
/// makes a run safe — it is the one that survives a bug *in this harness* (a panic between two
/// components leaves the tree as the last run left it, not as it started). One `tar`, in the temp
/// directory, named by the epoch so two runs cannot overwrite each other.
fn snapshot_components(idf: &Path) -> Result<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("idf-components-backup-{secs}.tgz"));
    let status = std::process::Command::new("tar")
        .arg("czf")
        .arg(&path)
        .arg("-C")
        .arg(idf)
        .arg("components")
        .status()
        .context("cannot run `tar` for the pre-run backup")?;
    if !status.success() {
        bail!("the pre-run backup did not complete (tar exited {status})");
    }
    Ok(path)
}

/// A sender whose receiver is drained forever, for the coordinator's channels this run never asks.
fn idle<T: Send + 'static>() -> mpsc::Sender<T> {
    let (tx, mut rx) = mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    tx
}

/// The two calls the app makes at startup: point a graph actor at a store. The reply is awaited so
/// the first read cannot race the load.
async fn init_graph(tx: &mpsc::Sender<MgMsg>, dir: &Path) -> Result<()> {
    let (reply, rx) = oneshot::channel();
    tx.send(MgMsg::Initialize {
        data_dir: dir.to_path_buf(),
        reply_to: reply,
    })
    .await
    .context("send Initialize to the graph")?;
    rx.await
        .context("Initialize reply")?
        .context("the graph did not initialize")?;
    Ok(())
}

/// What the graph already holds for a component — printed before its run, so the transcript shows
/// what the spec said beside what the gate accepted.
async fn print_stored_spec(memory_graph_tx: &mpsc::Sender<MgMsg>, library: &str, component: &str) {
    match spire_esp32::subsystems::project::component_persist::load_component_spec(
        memory_graph_tx,
        library,
        component,
    )
    .await
    {
        Ok(Some(spec)) => {
            println!("spec       {}  ({:?})", spec.name, spec.kind);
            println!("  description  {}", spec.description);
            println!("  invariants   {}", spec.invariants.len());
            for invariant in &spec.invariants {
                println!("    · {invariant}");
            }
            println!("  facts        {}", spec.facts.len());
            for fact in &spec.facts {
                println!("    · {}  ({})", fact.source, fact.corpus);
            }
        }
        Ok(None) => println!(
            "spec       none stored yet — the coordinator backfills what the tree states and \
             carries on"
        ),
        Err(e) => println!("spec       unreadable ({e}) — the coordinator falls back to the tree"),
    }
}

/// One `idf_component_edit` through the coordinator, with the record synced to the project graph.
///
/// The single-component mode and the batch mode share this verbatim: the batch is ten of these in a
/// row, not a second path through the door.
async fn regenerate_one(
    coord_tx: &mpsc::Sender<CoordinatorMessage>,
    memory_graph_tx: &mpsc::Sender<MgMsg>,
    idf: &Path,
    component: &str,
    instruction: &str,
) -> Result<serde_json::Value> {
    let started = std::time::Instant::now();
    let (reply, rx) = oneshot::channel();
    coord_tx
        .send(CoordinatorMessage::HandleRequest {
            method: "tools/call".to_string(),
            params: serde_json::json!({
                "tool": "idf_component_edit",
                "args": {
                    "root": idf.to_string_lossy(),
                    "name": component,
                    "instruction": instruction,
                },
            }),
            response_tx: reply,
        })
        .await
        .context("send HandleRequest")?;
    let result = rx.await.context("the coordinator dropped the reply")?;

    // The record this run wrote lives in the project graph, like every other edit.
    let (reply, rx) = oneshot::channel();
    memory_graph_tx
        .send(MgMsg::Sync { reply_to: reply })
        .await
        .context("send Sync to the graph")?;
    let _ = rx.await;

    println!("elapsed    {:.1}s", started.elapsed().as_secs_f32());
    Ok(result)
}

#[tokio::main]
async fn main() -> Result<()> {
    // The app names its own scope; a test binary does not, and this example is not named
    // `spire-esp32` either. `SPIRE_APP_NAME` still wins when it is set.
    spire_core::config::set_app_name("spire-esp32");

    let idf = idf_root()?;
    let component = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("SPIRE_COMPONENT").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "sht20".to_string());
    let instruction = std::env::var("SPIRE_INSTRUCTION").unwrap_or_default();
    let library = spire_esp32::build::edit_history::library_name(&idf.to_string_lossy());
    let data_dir = idf.join(".spire").join("data");

    let config = spire_core::config::load_global_llm_config();
    if config.api_key.is_empty() {
        bail!(
            "no LLM key in ~/.spire/spire-esp32/llm-config.json (or the scope SPIRE_APP_NAME names) \
             — this harness exists to spend a real request, so it has nothing to do without one"
        );
    }

    println!("library    {library}  ({})", idf.display());
    println!("component  {component}");
    println!("graph      {}", data_dir.display());
    println!("model      {}  ({})", config.model, config.api_url);
    println!(
        "asked      {}",
        if instruction.trim().is_empty() {
            "(the spec's own description)"
        } else {
            instruction.trim()
        }
    );
    println!("(the Spire app must not be running against this store)");
    println!();

    // ── The store, and the spec this run starts from ──
    let system = ActorSystem::new();
    let (memory_graph_tx, _) = system.spawn(MemoryGraphActor::new());
    init_graph(&memory_graph_tx, &data_dir).await?;

    // A single component's spec, before anything is installed or asked. The batch prints each one as
    // it reaches it — and `all` is not a component, so asking the graph for its spec would report a
    // missing spec that was never meant to exist.
    if component != "all" {
        print_stored_spec(&memory_graph_tx, &library, &component).await;
        println!();
    }

    // ── The corpus: the facts the prompt can carry ──
    //
    // Installed where the manifest's relative `path` resolves, then ingested: exactly what the
    // app's installer does. A run without it would still write a component, but the prompt would
    // say the facts are unknown — which is the failure this harness is meant to notice.
    let store_tmp = tempfile::tempdir().context("temp store")?;
    let store = store_tmp.path().to_path_buf();
    rag_bundle::install_into(&store).map_err(anyhow::Error::msg)?;

    let (knowledge_tx, _) = system.spawn(MemoryGraphActor::new());
    init_graph(&knowledge_tx, &store).await?;
    let embedder: Arc<dyn Embedder> =
        Arc::new(spire_core::embedder::CandleEmbedder::new().context("load the embedder")?);
    {
        let (reply, rx) = oneshot::channel();
        knowledge_tx
            .send(MgMsg::InitializeEmbedder {
                model_path: None,
                embedder: Some(embedder.clone()),
                reply_to: reply,
            })
            .await
            .context("send InitializeEmbedder")?;
        let _ = rx.await;
    }
    let registry = Arc::new(ServiceRegistry::new());
    let _ = registry.register_service("embedder", Arc::new(EmbedderService(embedder)));
    let _ = registry.register::<MgMsg>("knowledge_graph", knowledge_tx.clone());
    let _ = registry.register::<MgMsg>("memory_graph", memory_graph_tx.clone());
    let (rag_tx, _) = system.spawn(RagActor::from_registry(
        knowledge_tx,
        memory_graph_tx.clone(),
        registry.clone(),
    ));
    let _ = registry.register::<RagMessage>("rag", rag_tx.clone());
    let (reply, rx) = oneshot::channel();
    rag_tx
        .send(RagMessage::IngestGraphConfig {
            manifest_path: store
                .join(rag_bundle::DEVICE_FACTS_CORPUS)
                .join("ingest.yaml"),
            project_root: None,
            reply_to: reply,
        })
        .await
        .context("send IngestGraphConfig")?;
    let report = rx
        .await
        .context("ingest reply")?
        .context("the device-facts corpus did not ingest")?;
    println!(
        "corpus     {} chunks from {}\n",
        report.chunks,
        rag_bundle::DEVICE_FACTS_CORPUS
    );

    // ── The actors, wired as the app wires them ──
    let (chat_tx, _) = system.spawn(ChatActor::new());
    let (tools_tx, _) = system.spawn(ToolsActor::new(idle()));
    let (mcp_tx, _) = system.spawn(McpClientActor::new());
    let (llm_tx, _) = system.spawn(LlmActor::new(config));
    let (system_tx, _) = system.spawn(SystemActor::new());
    let (coord_tx, _) = system.spawn(CoordinatorActor::new(
        chat_tx,
        tools_tx,
        mcp_tx,
        llm_tx,
        system_tx,
        memory_graph_tx.clone(),
        idle(),
        idle(),
        idle(),
        idle(),
        idle(),
    ));

    let (watcher_tx, _watcher_rx) = mpsc::channel::<spire_esp32::actors::FileChangeNotification>(8);
    coord_tx
        .send(CoordinatorMessage::SetFfiDeps {
            registry,
            state: Arc::new(FfiSharedState {
                project_root: Mutex::new(None),
                analysis: Mutex::new(None),
                watcher_out_tx: watcher_tx,
            }),
        })
        .await
        .context("send SetFfiDeps")?;

    // ── The run, or the audit ──
    if component == "all" {
        return run_all(&coord_tx, &memory_graph_tx, &idf, &library, &instruction).await;
    }

    println!("regenerating through `idf_component_edit` …");
    let result =
        regenerate_one(&coord_tx, &memory_graph_tx, &idf, &component, &instruction).await?;

    println!("\n{}", serde_json::to_string_pretty(&result)?);

    // A non-zero exit when the gate refused, so a script can tell the two apart.
    let success = result
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !success {
        bail!("the regeneration did not pass its gate (see the result above)");
    }
    println!(
        "\nthe spec was re-ratified; `git -C {} diff -- components/{}` shows what changed",
        idf.display(),
        component
    );
    Ok(())
}

/// The batch half of the harness: every editable component, one run at a time.
///
/// Sequential on purpose. The coordinator is one actor and each run spends a model request, so
/// running ten at once would interleave the transcript the audit exists to read; per-component
/// `test/build` directories mean nothing is shared between them anyway. A component the gate refuses
/// is already rolled back by the door before its result comes back, so the batch records it and
/// moves on — the refusal is a finding about that spec, not a reason to stop measuring the rest.
async fn run_all(
    coord_tx: &mpsc::Sender<CoordinatorMessage>,
    memory_graph_tx: &mpsc::Sender<MgMsg>,
    idf: &Path,
    library: &str,
    instruction: &str,
) -> Result<()> {
    let components = spire_esp32::build::idf_projects::editable_components(idf);
    if components.is_empty() {
        bail!("{} has no editable components", idf.display());
    }
    // The audit's question is what each component's **own** spec carries. One instruction applied to
    // ten components is a different question, and a batch is the wrong shape for it: name the
    // component and run it alone.
    if !instruction.trim().is_empty() {
        bail!(
            "`all` regenerates each component from its own spec, so SPIRE_INSTRUCTION cannot be used \
             with it — name the component and re-run with that instruction"
        );
    }
    let backup = snapshot_components(idf)?;
    println!(
        "audit      {} components: {}\n           backup {}\n",
        components.len(),
        components.join(", "),
        backup.display()
    );

    let mut failures = Vec::new();
    for (position, name) in components.iter().enumerate() {
        println!(
            "── [{}/{}] {name} ─────────────────────────────────────────",
            position + 1,
            components.len()
        );
        print_stored_spec(memory_graph_tx, library, name).await;
        let result = regenerate_one(coord_tx, memory_graph_tx, idf, name, instruction).await?;
        let success = result
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let verified = result
            .get("verified")
            .and_then(|v| v.as_str())
            .unwrap_or("—");
        let up_to_date = result
            .get("up_to_date")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if success {
            println!(
                "✓ {name}  verified {verified}{}\n",
                if up_to_date { "  (up to date)" } else { "" }
            );
        } else {
            // The whole result, because a refusal that names no file (a link error, a missing
            // `REQUIRES`) is only actionable in the compiler's own words.
            println!(
                "✗ {name}  rejected\n{}",
                serde_json::to_string_pretty(&result)?
            );
            println!();
            failures.push(name.clone());
        }
    }

    // The table is the point of the batch: one line per component, in the order the runs took.
    println!("──── audit summary ────");
    for name in &components {
        let mark = if failures.contains(name) {
            "✗"
        } else {
            "✓"
        };
        println!("{mark} {name}");
    }
    if failures.is_empty() {
        println!(
            "\nall {} components regenerated through the door and passed their gate",
            components.len()
        );
        Ok(())
    } else {
        bail!(
            "{} of {} components did not regenerate: {} — each was rolled back by the door; \
             the specs that could not carry their component are the finding",
            failures.len(),
            components.len(),
            failures.join(", ")
        )
    }
}
