// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **What state every component of a library is in** — the one read a navigator draws its rows from.
//!
//! A component row has two facts worth a mark, and they come from different places:
//!
//! - **how much of the contract exists.** A spec somebody wrote and a decision that ratifies it is
//!   [`Fill::Filled`]; a directory nothing has been said about yet is [`Fill::Implied`] — the least
//!   its own `CMakeLists.txt` states, which is what `project/componentSpec` implies for it; and a
//!   component whose manifest states no kind at all is [`Fill::NoKind`], because there is nothing to
//!   imply and no door can read it back.
//! - **what the last write's gate decided**, from the `EditRecord` `idf_component_edit` keeps. A
//!   component that compiled yesterday and was refused today reads as refused, which is the fact a
//!   reader wants; the code may be exactly as it was and the request still did not land.
//!
//! # A read, and only a read
//!
//! This walks the tree for its component list and asks the graph for each spec, so it answers for a
//! component the graph has never seen — and it **writes nothing**, not even the backfill a first edit
//! performs. Drawing a row is not a leg about to write one, and a navigator that filled in every
//! component it displayed would decide contracts nobody asked for.
//!
//! # Why the list is the tree's
//!
//! The graph knows the components somebody has written; the tree knows the ones that exist. A
//! navigator has to show both, so the membership rule here is the analyzer's — a directory directly
//! under `components/` carrying a `CMakeLists.txt` — and the graph is asked only *about* those names.
//! Sorted, so two reads of one tree agree and a failure can be named by position.

use std::path::Path;

use serde::Serialize;
use spire_core::subsystems::graph::memory_graph::MemoryGraphMessage;
use tokio::sync::mpsc;

use super::component_persist;
use super::component_spec::{self, ComponentSpec};
use super::decision::{freshness_json, DesignFreshness};
use crate::build::edit_history;
use crate::build::idf_projects::{component_kind, ComponentKind};

/// How much of a component's contract exists, in the terms a row can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    /// A **stored** spec that says more than the component's own `CMakeLists.txt` — prose, a device,
    /// a role, an invariant or a fact — so somebody has described it and the record is asked whether
    /// it still stands.
    Filled,
    /// No stored spec, or one that still only repeats the tree: nothing has been said about this
    /// component yet, and its first edit is what decides it.
    Implied,
    /// The manifest states no kind, so [`component_persist::implied_component_spec`] refuses it by
    /// name: there is no contract to show and nothing can be filled until the component says what it
    /// is. An answer, not an error.
    NoKind,
}

/// One component, as a row reads it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComponentStatus {
    /// The component's directory name — the same identity the analyzer gives a component row.
    pub name: String,
    /// What its own manifest says, or the kind its stored spec states for a manifest that is silent.
    /// `None` when neither says anything, which is what [`Fill::NoKind`] reports.
    pub kind: Option<&'static str>,
    /// How much of the contract exists.
    pub fill: Fill,
    /// Whether the record still ratifies the spec beside it — the same shape
    /// `project/componentSpec` answers, so a reader decodes one model. `absent` for a component with
    /// no stored spec, which is the honest "nobody has decided this yet".
    pub freshness: serde_json::Value,
    /// The last `idf_component_edit` run recorded for it, reduced to the verdict a row shows —
    /// `null` when it has never been written, which is not the same as a run that failed.
    pub gate: serde_json::Value,
}

/// Every component a library's tree holds, sorted by name.
///
/// The analyzer's rule, reused: a directory directly under `components/` that carries a
/// `CMakeLists.txt`. A file or a dot-directory is not a component, and a directory with no manifest is
/// not one either — the build could not see it.
pub fn components_of(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join("components")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_string();
            (path.is_dir() && !name.starts_with('.') && path.join("CMakeLists.txt").is_file())
                .then_some(name)
        })
        .collect();
    names.sort();
    names
}

/// **Every component of `root`, with the state its row shows** — one read for the whole list.
///
/// The library name is [`edit_history::library_name`]'s, the same one the spec store and the edit
/// history are scoped by, so a navigator cannot ask about a component under a name nothing was
/// written with.
///
/// `Err` is the one failure that stops the walk: the graph could not be read. A component whose
/// implied spec is refused is not a failure — it is [`Fill::NoKind`], or the kind its stored spec
/// states — because a list that refused to draw because one component states nothing would hide the
/// ones that do.
pub async fn component_statuses(
    mg_tx: &mpsc::Sender<MemoryGraphMessage>,
    root: &Path,
) -> Result<Vec<ComponentStatus>, String> {
    let library = edit_history::library_name(&root.to_string_lossy());
    let mut statuses = Vec::new();
    for name in components_of(root) {
        statuses.push(component_status(mg_tx, root, &library, &name).await?);
    }
    Ok(statuses)
}

/// One component's status — the walk [`component_statuses`] runs, one name at a time.
pub async fn component_status(
    mg_tx: &mpsc::Sender<MemoryGraphMessage>,
    root: &Path,
    library: &str,
    name: &str,
) -> Result<ComponentStatus, String> {
    // The manifest is authority on the kind, and a spec that disagrees describes a different
    // component — but this read does not police that (`project/componentSpec` refuses it): a status
    // read should still say what the tree holds rather than fail the whole list.
    let stated: Option<ComponentKind> = component_kind(root, name);
    let stored: Option<ComponentSpec> =
        component_persist::load_component_spec(mg_tx, library, name)
            .await
            .map_err(|e| format!("components/{name}: the spec could not be read: {e}"))?;
    let record = match &stored {
        None => None,
        Some(_) => component_persist::load_component_record(mg_tx, library, name)
            .await
            .map_err(|e| format!("components/{name}: the record could not be read: {e}"))?,
    };

    let kind = stated.or_else(|| stored.as_ref().map(|spec| spec.kind));
    let fill = match &stored {
        Some(spec) if spec.has_contract() => Fill::Filled,
        _ if kind.is_some() => Fill::Implied,
        _ => Fill::NoKind,
    };
    // A spec nobody stored has no decision to be stale, and a stored one with no record is `absent`
    // — the same answer `project/componentSpec` gives, from the same function.
    let freshness = match &stored {
        Some(spec) => component_spec::freshness(record.as_ref(), spec),
        None => DesignFreshness::Absent,
    };

    // One record is enough: what a row shows is the last run; the history pane is where the rest of
    // the log lives.
    let gate = edit_history::component_edit_history(mg_tx, &root.to_string_lossy(), name, 1)
        .await
        .into_iter()
        .next()
        .map(gate_json)
        .unwrap_or(serde_json::Value::Null);

    Ok(ComponentStatus {
        name: name.to_string(),
        kind: kind.map(ComponentKind::as_str),
        fill,
        freshness: freshness_json(&freshness),
        gate,
    })
}

/// The last run, reduced to what a row can say: did it survive its gate, what was checked, and when.
///
/// The rest of the record — the instruction, the files that moved or were reverted, the caveats,
/// every earlier run — stays in `idf-component/history`, which is what the pane opens for. A row that
/// carried all of it would be a second history in miniature, read at a glance it does not survive.
fn gate_json(record: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "success": record["success"],
        "up_to_date": record["up_to_date"],
        "verified": record["verified"],
        "chip_build": record["chip_build"],
        "at": record["at"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::idf_projects::component_kind_line;
    use crate::subsystems::project::component_persist::{
        decide_component_spec, load_component_spec, store_component_spec,
    };
    use crate::subsystems::project::component_spec::{ComponentDevice, ComponentSource, FactRef};
    use spire_core::actors::Actor;
    use spire_core::subsystems::graph::memory_graph::MemoryGraphActor;
    use tokio::sync::oneshot;

    /// A library tree holding `components/<name>/CMakeLists.txt` for each entry, stating its kind.
    fn library_with(components: &[(&str, ComponentKind)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("a temp dir");
        for (name, kind) in components {
            let dir = root.path().join("components").join(name);
            std::fs::create_dir_all(&dir).expect("the component's directory");
            std::fs::write(
                dir.join("CMakeLists.txt"),
                format!("{}\n", component_kind_line(*kind)),
            )
            .expect("the manifest");
        }
        root
    }

    /// A real `MemoryGraphActor` over a fresh in-memory store — the actor the app runs.
    async fn fresh_graph(dir: &Path) -> mpsc::Sender<MemoryGraphMessage> {
        let (tx, rx) = mpsc::channel(64);
        let _join = MemoryGraphActor::new().spawn(rx);
        let (reply, ready) = oneshot::channel();
        tx.send(MemoryGraphMessage::InitializeInMemory {
            data_dir: dir.to_path_buf(),
            reply_to: reply,
        })
        .await
        .expect("send InitializeInMemory");
        ready.await.expect("init reply").expect("init");
        tx
    }

    /// A spec that says something: prose, an invariant, a device, a role and a document.
    fn sps30() -> ComponentSpec {
        ComponentSpec {
            name: "sps30".to_string(),
            kind: ComponentKind::Driver,
            description: "Read PM2.5 from the SPS30 over I2C.".to_string(),
            invariants: vec!["a failed checksum is a reading, not an exception".to_string()],
            device: Some(ComponentDevice {
                part: "sps30".to_string(),
                bus: "i2c".to_string(),
                address: Some("0x69".to_string()),
            }),
            provides: vec!["sensing.particulate".to_string()],
            facts: vec![FactRef {
                corpus: "device-facts".to_string(),
                source: "sps30.md".to_string(),
            }],
        }
    }

    fn status_of<'a>(statuses: &'a [ComponentStatus], name: &str) -> &'a ComponentStatus {
        statuses
            .iter()
            .find(|status| status.name == name)
            .unwrap_or_else(|| panic!("{name} is in the list: {statuses:?}"))
    }

    /// **A component the graph has never seen is implied, and reading it stores nothing.**
    ///
    /// The read is what the navigator does on every appearance, so it is the one place a backfill
    /// would be invisible and every component of every library would acquire a spec nobody decided.
    #[tokio::test]
    async fn an_unwritten_component_reads_as_implied_and_the_read_backfills_nothing() {
        let tree = library_with(&[("sps30", ComponentKind::Driver)]);
        let graph = fresh_graph(&tree.path().join("graph")).await;
        let library = edit_history::library_name(&tree.path().to_string_lossy());

        let statuses = component_statuses(&graph, tree.path())
            .await
            .expect("a library with one component reads");
        assert_eq!(statuses.len(), 1);
        let status = status_of(&statuses, "sps30");
        assert_eq!(status.kind, Some("driver"), "the manifest's kind");
        assert_eq!(status.fill, Fill::Implied, "nothing has been said about it");
        assert_eq!(status.freshness, serde_json::json!({ "state": "absent" }));
        assert_eq!(status.gate, serde_json::Value::Null, "never written");

        assert_eq!(
            load_component_spec(&graph, &library, "sps30").await,
            Ok(None),
            "looking at a component is not a leg that writes one"
        );
    }

    /// **A decided spec reads as filled, and the row follows the record**: current until the spec is
    /// edited, stale after — the freshness `project/componentSpec` reports, from the same function.
    #[tokio::test]
    async fn a_decided_spec_reads_as_filled_and_goes_stale_when_the_spec_moves() {
        let tree = library_with(&[("sps30", ComponentKind::Driver)]);
        let graph = fresh_graph(&tree.path().join("graph")).await;
        let library = edit_history::library_name(&tree.path().to_string_lossy());

        store_component_spec(&graph, &library, &sps30())
            .await
            .expect("stored");
        decide_component_spec(&graph, &library, &sps30(), ComponentSource::Filled)
            .await
            .expect("decided");

        let statuses = component_statuses(&graph, tree.path()).await.expect("read");
        let status = status_of(&statuses, "sps30");
        assert_eq!(status.fill, Fill::Filled);
        assert_eq!(status.kind, Some("driver"));
        assert_eq!(status.freshness["state"], "current");

        let mut edited = sps30();
        edited.description = "Read PM2.5 and PM10.".to_string();
        store_component_spec(&graph, &library, &edited)
            .await
            .expect("re-stored");

        let statuses = component_statuses(&graph, tree.path()).await.expect("read");
        assert_eq!(
            status_of(&statuses, "sps30").freshness["state"],
            "stale",
            "a spec edited since its decision must not read as decided"
        );
    }

    /// A stored spec that only repeats the tree is **not** filled: a backfilled stub is stored and its
    /// record ratifies it, so "is it stored" cannot be the question a row asks.
    #[tokio::test]
    async fn a_stored_spec_with_nothing_in_it_still_reads_as_implied() {
        let tree = library_with(&[("blank", ComponentKind::Library)]);
        let graph = fresh_graph(&tree.path().join("graph")).await;
        let library = edit_history::library_name(&tree.path().to_string_lossy());

        let blank = ComponentSpec::new("blank", ComponentKind::Library);
        store_component_spec(&graph, &library, &blank)
            .await
            .expect("stored");
        decide_component_spec(&graph, &library, &blank, ComponentSource::Stub)
            .await
            .expect("decided");

        let statuses = component_statuses(&graph, tree.path()).await.expect("read");
        let status = status_of(&statuses, "blank");
        assert_eq!(
            status.fill,
            Fill::Implied,
            "a decision that says nothing has not filled anything in"
        );
        assert_eq!(status.freshness["state"], "current");
    }

    /// A manifest that states no kind has no contract to imply — the answer, not a failure, and one
    /// component stating nothing does not stop the ones that do from being read.
    #[tokio::test]
    async fn a_component_that_states_no_kind_reads_as_no_kind_beside_the_others() {
        let tree = library_with(&[("sps30", ComponentKind::Driver)]);
        let vendored = tree.path().join("components").join("lvgl");
        std::fs::create_dir_all(&vendored).expect("the vendored directory");
        std::fs::write(
            vendored.join("CMakeLists.txt"),
            "idf_component_register()\n",
        )
        .expect("a manifest with no kind");
        // Neither a file nor a dotted directory is a component, so neither is in the list.
        std::fs::write(
            tree.path().join("components").join("README.md"),
            "not a component\n",
        )
        .expect("a file under components/");
        std::fs::create_dir_all(tree.path().join("components").join(".cache"))
            .expect("a dot-directory");

        let graph = fresh_graph(&tree.path().join("graph")).await;
        let statuses = component_statuses(&graph, tree.path()).await.expect("read");
        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["lvgl", "sps30"],
            "the tree's components, sorted"
        );

        let lvgl = status_of(&statuses, "lvgl");
        assert_eq!(lvgl.kind, None);
        assert_eq!(lvgl.fill, Fill::NoKind);
        assert_eq!(lvgl.gate, serde_json::Value::Null);
        assert_eq!(status_of(&statuses, "sps30").fill, Fill::Implied);
    }

    /// **The row's second fact is the newest run's verdict**, not the code's state: a refusal is what
    /// happened, even though the files were put back exactly as they were.
    #[tokio::test]
    async fn the_gate_is_the_newest_runs_verdict() {
        let tree = library_with(&[("sps30", ComponentKind::Driver)]);
        let graph = fresh_graph(&tree.path().join("graph")).await;
        let root = tree.path().to_string_lossy().to_string();
        let no_files: Vec<String> = Vec::new();
        let no_caveats: Vec<String> = Vec::new();

        edit_history::record_component_edit(
            &graph,
            &edit_history::EditFacts {
                instruction: "Add a PM10 reading.",
                component: "sps30",
                root: &root,
                success: true,
                verified: "HostOnly",
                up_to_date: false,
                files_changed: &["components/sps30/src/sps30.cpp".to_string()],
                files_reverted: &no_files,
                files_skipped: &no_files,
                chip_build: "not run: no chip named",
                caveats: &no_caveats,
            },
        )
        .await
        .expect("the kept run is recorded");
        // A later run the gate refused: newer, and the one the row must report.
        edit_history::record_component_edit(
            &graph,
            &edit_history::EditFacts {
                instruction: "Return the humidity too.",
                component: "sps30",
                root: &root,
                success: false,
                verified: "HostOnly",
                up_to_date: false,
                files_changed: &no_files,
                files_reverted: &["components/sps30/src/sps30.cpp".to_string()],
                files_skipped: &no_files,
                chip_build: "not run: no chip named",
                caveats: &no_caveats,
            },
        )
        .await
        .expect("the refused run is recorded");

        let statuses = component_statuses(&graph, tree.path()).await.expect("read");
        let gate = &status_of(&statuses, "sps30").gate;
        assert_eq!(gate["success"], false, "the refusal is the newest fact");
        assert_eq!(gate["verified"], "HostOnly");
        assert_eq!(gate["chip_build"], "not run: no chip named");
        assert!(
            gate["at"].as_str().is_some_and(|at| at.contains('T')),
            "the run is stamped with when it happened: {gate}"
        );
    }
}
