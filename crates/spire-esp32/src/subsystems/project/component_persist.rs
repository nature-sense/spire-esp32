// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **Component-spec persistence** — how the memory-graph actor layer stores a component's contract.
//!
//! The decomposed spec ([`super::component_spec::decompose`]) is written the same way an
//! application's composition is ([`super::composition_persist`]), because it is the same kind of
//! thing one level down:
//!
//! - one **anchor** node (`node_type = "idf_component"`, `subtype = "anchor"`, name
//!   `{library}::{component}::component`) carrying the kind, the fill prose, the invariants and a
//!   rendered `SPIRE.component.yaml` copy — the file projection, never the source;
//! - every decomposed node becomes an `AttrNode` (`node_type = "idf_component"`, its `node::`
//!   discriminator as `subtype`, the logical name in `logical`, memory name scoped
//!   `{library}::{component}::{logical}`);
//! - every decomposed edge becomes a `Custom(<predicate>)` relationship between the stored ids.
//!
//! # Why a component is scoped twice
//!
//! A component's *name* is only unique within its library — two libraries may both have an `sps30`
//! that are different code — so the scope is `{library}::{component}`, the same pair the edit history
//! uses. The anchor then sits inside that scope as `component`, which keeps each spec's whole
//! decomposition one prefix away and lets [`load_component_spec`] tell this component's nodes from a
//! sibling's without trusting a property.
//!
//! # A store is a replacement, not an append
//!
//! Re-storing a spec **prunes** the children it no longer states. That matters because a spec can
//! shrink: removing a role or a fact is an edit a person can make, and a loader that read back the
//! old `role.2` beside the new ones would report a contract nobody holds any more.
//!
//! # The record
//!
//! Storing a spec is not the same as **deciding** one — the backfill of a component that predates
//! this graph writes a spec from what the tree already says — so the decision is a node of its own:
//! [`store_component_record`] writes a `design_record` child of the anchor via `HAS_DESIGN_RECORD`
//! carrying the shared [`Decision`](super::decision::Decision), and [`load_component_freshness`]
//! compares its fingerprint with the spec in the graph so a reader is *told* whether the record still
//! stands.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use spire_core::models::memory_graph::AttrNode;
use spire_core::subsystems::graph::memory_graph::MemoryGraphMessage;
use tracing::{info, warn};

use super::component_spec::{
    self, edge, node, ComponentRecord, ComponentSource, ComponentSpec, DESIGN_RECORD_NAME, ROOT,
};
use super::decision::DesignFreshness;
use crate::build::idf_projects::{component_kind, ident_of};
use crate::graph_io::{
    create_rel, decode_props, delete_node, encode_props, mem_name, merge_node, query_nodes,
    rels_of_node, un_mem_name, PROP_LOGICAL, QUERY_LIMIT_ALL,
};

/// `node_type` for every decomposed component-spec node (the `node::` subtype carries the kind).
///
/// Distinct from the edit history's `Module`/`idf_component` spine: a history hangs off a node that
/// must survive a spec rewrite, and the store's merge can replace a node, so the two are kept apart
/// deliberately rather than reusing one name for both jobs.
pub const MG_NODE_TYPE: &str = "idf_component";

/// Property carrying the rendered `SPIRE.component.yaml` review copy on the anchor.
pub const PROP_RENDERED: &str = "rendered";
/// Property carrying the stored node's version (store metadata, not part of the spec).
const PROP_VERSION: &str = "version";

/// The file name a component's rendered spec is written to, beside its `CMakeLists.txt`.
pub const COMPONENT_SPEC_FILE: &str = "SPIRE.component.yaml";

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

/// The graph scope of a component: `{library}::{component}`.
fn component_scope(library: &str, name: &str) -> String {
    mem_name(library, &ident_of(name))
}

/// Persist a component's full decomposition, replacing what the previous store wrote.
///
/// The spec is **validated first** ([`ComponentSpec::validate`]): a role the vocabulary does not know
/// and a device on a library would both reach the prompt as facts, so they are refused before they
/// become anything. Children and edges are best-effort once the anchor is stored — the same rule the
/// composition follows — because a partial graph is recoverable while a refused write is not.
pub async fn store_component_spec(
    mg_tx: &tokio::sync::mpsc::Sender<MemoryGraphMessage>,
    library: &str,
    spec: &ComponentSpec,
) -> Result<String, String> {
    spec.validate()?;
    let scope = component_scope(library, &spec.name);
    let g = component_spec::decompose(spec);

    let anchor_node = g
        .nodes
        .iter()
        .find(|n| n.node_type == node::ANCHOR)
        .ok_or_else(|| "decompose must carry an anchor node".to_string())?;

    let mut anchor_props = encode_props(&anchor_node.properties);
    anchor_props.insert(PROP_VERSION.to_string(), serde_json::json!(1));
    anchor_props.insert(
        PROP_RENDERED.to_string(),
        serde_json::json!(component_spec::render_yaml(spec)),
    );
    let anchor = AttrNode {
        id: uuid::Uuid::new_v4().to_string(),
        node_type: MG_NODE_TYPE.to_string(),
        subtype: Some(node::ANCHOR.to_string()),
        name: mem_name(&scope, ROOT),
        description: anchor_node.description.clone(),
        properties: anchor_props,
        embedding_id: None,
        created_at: now(),
        updated_at: now(),
        version: 1,
    };
    let anchor_id = merge_node(mg_tx, anchor)
        .await
        .map_err(|e| format!("the component anchor for '{scope}' was not stored: {e}"))?
        .id;
    info!("[ComponentPersist] stored component anchor '{scope}' id={anchor_id}");

    let mut id_by_logical: HashMap<String, String> = HashMap::new();
    id_by_logical.insert(ROOT.to_string(), anchor_id.clone());
    for n in g.nodes.iter().filter(|n| n.node_type != node::ANCHOR) {
        let mut properties = encode_props(&n.properties);
        properties.insert(PROP_LOGICAL.to_string(), serde_json::json!(n.name));
        let child = AttrNode {
            id: uuid::Uuid::new_v4().to_string(),
            node_type: MG_NODE_TYPE.to_string(),
            subtype: Some(n.node_type.clone()),
            name: mem_name(&scope, &n.name),
            description: n.description.clone(),
            properties,
            embedding_id: None,
            created_at: now(),
            updated_at: now(),
            version: 1,
        };
        match merge_node(mg_tx, child).await {
            Ok(stored) => {
                id_by_logical.insert(n.name.clone(), stored.id);
            }
            Err(e) => warn!(
                "[ComponentPersist] node '{}' ({}) not stored for '{scope}': {e}",
                n.name, n.node_type
            ),
        }
    }

    for e in &g.edges {
        match (
            id_by_logical.get(&e.from_name),
            id_by_logical.get(&e.to_name),
        ) {
            (Some(from), Some(to)) => {
                if let Err(err) = create_rel(mg_tx, &e.predicate, from, to).await {
                    warn!(
                        "[ComponentPersist] edge '{}' {}->{} not stored for '{scope}': {err}",
                        e.predicate, e.from_name, e.to_name
                    );
                }
            }
            _ => warn!(
                "[ComponentPersist] edge '{}' {}->{} references an unstored node; skipped",
                e.predicate, e.from_name, e.to_name
            ),
        }
    }

    prune_children(mg_tx, &scope, &id_by_logical).await;
    Ok(anchor_id)
}

/// Delete this component's stored children that the new decomposition no longer states.
///
/// Best-effort and silent on an unreadable store: the nodes just written are the ones that matter,
/// and a leftover role is a stale answer rather than a lost one. The `design_record` child is left
/// alone — it is the *decision*, not part of the spec, and it is replaced by its own store.
async fn prune_children(
    mg_tx: &tokio::sync::mpsc::Sender<MemoryGraphMessage>,
    scope: &str,
    keep: &HashMap<String, String>,
) {
    let Ok(nodes) = query_nodes(mg_tx, Some(MG_NODE_TYPE), None, None, QUERY_LIMIT_ALL).await
    else {
        return;
    };
    let keep_ids: HashSet<&str> = keep.values().map(String::as_str).collect();
    for n in nodes {
        if n.subtype.as_deref() == Some(node::DESIGN_RECORD) || keep_ids.contains(n.id.as_str()) {
            continue;
        }
        let Some(logical) = un_mem_name(scope, &n.name) else {
            continue;
        };
        if logical == DESIGN_RECORD_NAME {
            continue;
        }
        if let Err(e) = delete_node(mg_tx, &n.id).await {
            warn!("[ComponentPersist] stale node '{logical}' not pruned from '{scope}': {e}");
        }
    }
}

/// **Read a component's stored spec**, when it has one.
///
/// `Ok(None)` means *no spec was ever stored* — the ordinary state of a component that predates this
/// graph — and never "the spec is broken": an anchor that is there but whose decomposition does not
/// rebuild is an `Err`, because a caller that confused the two would backfill over a contract that
/// already exists.
pub async fn load_component_spec(
    mg_tx: &tokio::sync::mpsc::Sender<MemoryGraphMessage>,
    library: &str,
    name: &str,
) -> Result<Option<ComponentSpec>, String> {
    let scope = component_scope(library, name);
    let nodes = query_nodes(mg_tx, Some(MG_NODE_TYPE), None, None, QUERY_LIMIT_ALL).await?;
    let anchor_name = mem_name(&scope, ROOT);
    let Some(anchor) = nodes
        .iter()
        .find(|n| n.subtype.as_deref() == Some(node::ANCHOR) && n.name == anchor_name)
    else {
        return Ok(None);
    };

    let mut g = component_spec::ComponentGraph::default();
    let mut id_to_logical: HashMap<String, String> = HashMap::new();
    g.nodes.push(component_spec::ComponentNode {
        node_type: node::ANCHOR.to_string(),
        name: ROOT.to_string(),
        description: anchor.description.clone(),
        properties: decode_props(&anchor.properties),
    });
    id_to_logical.insert(anchor.id.clone(), ROOT.to_string());

    for n in &nodes {
        if n.id == anchor.id
            || n.subtype.as_deref() == Some(node::ANCHOR)
            || n.subtype.as_deref() == Some(node::DESIGN_RECORD)
        {
            continue;
        }
        let Some(logical) = un_mem_name(&scope, &n.name) else {
            continue; // some other component's decomposition
        };
        let logical = logical.to_string();
        g.nodes.push(component_spec::ComponentNode {
            node_type: n.subtype.clone().unwrap_or_default(),
            name: logical.clone(),
            description: n.description.clone(),
            properties: decode_props(&n.properties),
        });
        id_to_logical.insert(n.id.clone(), logical);
    }

    // Edges: every stored node reports its relationships in both directions, so walking each node
    // covers every edge. Dedupe by (predicate, from, to) — a re-store appends fresh relationships and
    // the logical edge set must not double up.
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    for id in id_to_logical.keys() {
        for rel in rels_of_node(mg_tx, id).await? {
            let spire_core::models::memory_graph::RelationshipType::Custom(predicate) =
                &rel.edge_type
            else {
                continue;
            };
            let (Some(from), Some(to)) = (
                id_to_logical.get(&rel.from_id),
                id_to_logical.get(&rel.to_id),
            ) else {
                continue; // the design record, or some other component's node
            };
            if !seen.insert((predicate.clone(), from.clone(), to.clone())) {
                continue;
            }
            g.edges.push(component_spec::ComponentEdge {
                predicate: predicate.clone(),
                from_name: from.clone(),
                to_name: to.clone(),
            });
        }
    }

    component_spec::reconstruct(&g).map(Some)
}

/// Store the **decision** behind a component spec, linked to its anchor.
///
/// Best-effort once the anchor is found, like the composition's record store: a lost note is not a
/// lost spec. A component with no stored spec has nothing for a record to ratify, which is an `Err`
/// rather than a silent no-op — a caller that recorded nothing would go on reporting the component as
/// decided.
pub async fn store_component_record(
    mg_tx: &tokio::sync::mpsc::Sender<MemoryGraphMessage>,
    library: &str,
    name: &str,
    record: &ComponentRecord,
) -> Result<String, String> {
    let scope = component_scope(library, name);
    let anchor_name = mem_name(&scope, ROOT);
    let anchor = query_nodes(
        mg_tx,
        Some(MG_NODE_TYPE),
        Some(node::ANCHOR),
        Some(&anchor_name),
        1,
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| {
        format!(
            "no component spec is stored for '{scope}', so there is nothing for a record to ratify"
        )
    })?;

    let record_node = component_spec::component_record_node(record);
    let mut properties = encode_props(&record_node.properties);
    properties.insert(
        PROP_LOGICAL.to_string(),
        serde_json::json!(record_node.name),
    );
    let node = AttrNode {
        id: uuid::Uuid::new_v4().to_string(),
        node_type: MG_NODE_TYPE.to_string(),
        subtype: Some(node::DESIGN_RECORD.to_string()),
        name: mem_name(&scope, &record_node.name),
        description: record_node.description.clone(),
        properties,
        embedding_id: None,
        created_at: now(),
        updated_at: now(),
        version: 1,
    };
    let stored = merge_node(mg_tx, node)
        .await
        .map_err(|e| format!("the component record for '{scope}' was not stored: {e}"))?;
    if let Err(e) = create_rel(mg_tx, edge::HAS_DESIGN_RECORD, &anchor.id, &stored.id).await {
        warn!(
            "[ComponentPersist] record {} not linked to the anchor for '{scope}': {e}",
            stored.id
        );
    }
    info!(
        "[ComponentPersist] stored component record for '{scope}': v{} from {} ({})",
        record.decision.version,
        record.decision.source.as_str(),
        record.decision.fingerprint
    );
    Ok(stored.id)
}

/// Read a component's stored record, when it has one.
///
/// `Ok(None)` means **no decision was ever recorded**, never "the record is broken".
pub async fn load_component_record(
    mg_tx: &tokio::sync::mpsc::Sender<MemoryGraphMessage>,
    library: &str,
    name: &str,
) -> Result<Option<ComponentRecord>, String> {
    let scope = component_scope(library, name);
    let nodes = query_nodes(
        mg_tx,
        Some(MG_NODE_TYPE),
        Some(node::DESIGN_RECORD),
        Some(&mem_name(&scope, DESIGN_RECORD_NAME)),
        QUERY_LIMIT_ALL,
    )
    .await?;
    let Some(node) = nodes.into_iter().next() else {
        return Ok(None);
    };
    let g = component_spec::ComponentGraph {
        nodes: vec![component_spec::ComponentNode {
            node_type: node::DESIGN_RECORD.to_string(),
            name: DESIGN_RECORD_NAME.to_string(),
            description: node.description.clone(),
            properties: decode_props(&node.properties),
        }],
        edges: Vec::new(),
    };
    match component_spec::reconstruct_component_record(&g)? {
        Some(record) => Ok(Some(record)),
        None => Err(format!(
            "the stored component record for '{scope}' is present but unreadable"
        )),
    }
}

/// **Does the stored record still ratify the stored spec?** — [`DesignFreshness`], read out of the
/// memory graph.
///
/// `Err` only when the spec is stored and broken; a component with nothing stored is
/// [`DesignFreshness::Absent`], the honest "nobody has decided this yet".
pub async fn load_component_freshness(
    mg_tx: &tokio::sync::mpsc::Sender<MemoryGraphMessage>,
    library: &str,
    name: &str,
) -> Result<DesignFreshness, String> {
    match load_component_spec(mg_tx, library, name).await? {
        None => Ok(DesignFreshness::Absent),
        Some(spec) => {
            let record = load_component_record(mg_tx, library, name).await?;
            Ok(component_spec::freshness(record.as_ref(), &spec))
        }
    }
}

/// **Record `spec` as decided**, keeping the record that still ratifies it.
///
/// The rule the composition's accepting leg follows, applied to a component: an unchanged spec is not
/// decided twice, and an edited one supersedes its record with the next version. So a fill that
/// changed nothing leaves the record alone, and a fill that changed the prose or added a fact is a new
/// decision — which is what makes `freshness` say something after the next store.
///
pub async fn decide_component_spec(
    mg_tx: &tokio::sync::mpsc::Sender<MemoryGraphMessage>,
    library: &str,
    spec: &ComponentSpec,
    source: ComponentSource,
) -> Result<ComponentRecord, String> {
    let existing = load_component_record(mg_tx, library, &spec.name).await?;
    if let Some(record) = &existing {
        if record.ratifies(spec) {
            return Ok(record.clone());
        }
    }
    let version = existing
        .as_ref()
        .map(|record| record.decision.version + 1)
        .unwrap_or(1);
    let record = ComponentRecord::now(spec, source, version);
    store_component_record(mg_tx, library, &spec.name, &record).await?;
    Ok(record)
}

/// The spec a component's **tree** implies, for a component that predates this graph.
///
/// The least anyone can honestly say without asking: its name, and the kind its own `CMakeLists.txt`
/// states. No prose, no device, no roles — an invented description would be a claim nobody made, and
/// the fill prose is exactly the thing a person is supposed to supply. A name with no directory is
/// refused with the path it looked in, and a component whose manifest states no kind is refused by
/// name, because the kind decides the skeleton and is not recoverable.
pub fn implied_component_spec(root: &Path, name: &str) -> Result<ComponentSpec, String> {
    let ident = ident_of(name);
    if !root.join("components").join(&ident).is_dir() {
        return Err(format!(
            "{} has no component '{ident}': there is no components/{ident} directory.",
            root.display()
        ));
    }
    let kind = component_kind(root, &ident).ok_or_else(|| {
        format!(
            "components/{ident} states no kind in its own CMakeLists.txt (no `{}` line), so there is \
             no spec to read from the tree",
            crate::build::idf_projects::COMPONENT_KIND_KEY
        )
    })?;
    Ok(ComponentSpec::new(&ident, kind))
}

/// The spec for a component, **backfilled** when the graph has none.
///
/// This is the first-edit door the older components come through: a library of fourteen components
/// that predate the graph must still be editable, and refusing them until someone hand-writes a spec
/// would make the feature unusable on exactly the projects it is for. The backfill stores the implied
/// spec and records it as [`ComponentSource::Stub`] — nobody has filled it, and the record says so
/// rather than pretending a decision was taken.
///
/// Returns `(spec, backfilled)`; `backfilled = false` means the graph already had one.
pub async fn backfill_component_spec(
    mg_tx: &tokio::sync::mpsc::Sender<MemoryGraphMessage>,
    root: &Path,
    library: &str,
    name: &str,
) -> Result<(ComponentSpec, bool), String> {
    if let Some(spec) = load_component_spec(mg_tx, library, name).await? {
        return Ok((spec, false));
    }
    let spec = implied_component_spec(root, name)?;
    store_component_spec(mg_tx, library, &spec).await?;
    let record = ComponentRecord::now(&spec, ComponentSource::Stub, 1);
    if let Err(e) = store_component_record(mg_tx, library, name, &record).await {
        warn!("[ComponentPersist] backfilled spec for '{library}::{name}' with no record: {e}");
    }
    Ok((spec, true))
}

/// **Check the spec's kind against the component's own `CMakeLists.txt`.**
///
/// The manifest is authoritative — it is what every other leg reads, and it is what decided the
/// skeleton — so a spec that says something else describes a *different* component. Refused rather
/// than reconciled: the spec cannot know which of the two a person meant, and picking one would write
/// the wrong skeleton's code. A component whose manifest states no kind (written by hand) is not
/// checked: there is nothing to disagree with.
pub fn check_kind(root: &Path, spec: &ComponentSpec) -> Result<(), String> {
    let Some(stated) = component_kind(root, &spec.name) else {
        return Ok(());
    };
    if stated == spec.kind {
        return Ok(());
    }
    Err(format!(
        "the spec for '{}' says it is a {}, but components/{}/CMakeLists.txt states `{}` — the manifest is authoritative. Correct one of the two and try again; nothing was written.",
        spec.name,
        spec.kind.as_str(),
        ident_of(&spec.name),
        stated.as_str()
    ))
}

/// Write the **rendered copy** of `spec` beside the component's own files.
///
/// A projection, and the only file this module touches: the graph is what every flow reads, and the
/// file exists so a person can open the component's directory and see what it is supposed to be. A
/// write failure is the caller's to weigh — see `idf_add_component` — because losing a copy is not
/// losing the contract.
pub fn write_component_spec_file(
    root: &Path,
    name: &str,
    spec: &ComponentSpec,
) -> Result<PathBuf, String> {
    let ident = ident_of(name);
    let dir = root.join("components").join(&ident);
    if !dir.is_dir() {
        return Err(format!(
            "{} has no component '{ident}': there is no components/{ident} directory.",
            root.display()
        ));
    }
    let path = dir.join(COMPONENT_SPEC_FILE);
    std::fs::write(&path, component_spec::render_yaml(spec))
        .map_err(|e| format!("{} could not be written: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::idf_projects::{component_kind_line, ComponentKind};
    use spire_core::actors::Actor;
    use spire_core::subsystems::graph::memory_graph::MemoryGraphActor;
    use tokio::sync::mpsc;

    /// A real `MemoryGraphActor` over a fresh in-memory store — the actor the app runs, so the round
    /// trip under test is the real one, not a stand-in.
    async fn fresh_graph(dir: &std::path::Path) -> mpsc::Sender<MemoryGraphMessage> {
        let (tx, rx) = mpsc::channel(64);
        let _join = MemoryGraphActor::new().spawn(rx);
        let (reply, ready) = tokio::sync::oneshot::channel();
        tx.send(MemoryGraphMessage::InitializeInMemory {
            data_dir: dir.to_path_buf(),
            reply_to: reply,
        })
        .await
        .expect("send InitializeInMemory");
        ready.await.expect("init reply").expect("init");
        tx
    }

    fn sps30() -> ComponentSpec {
        ComponentSpec {
            name: "sps30".to_string(),
            kind: ComponentKind::Driver,
            description: "Read PM2.5 from the SPS30 over I2C.".to_string(),
            invariants: vec!["a failed checksum is a reading, not an exception".to_string()],
            device: Some(component_spec::ComponentDevice {
                part: "sps30".to_string(),
                bus: "i2c".to_string(),
                address: Some("0x69".to_string()),
            }),
            provides: vec![
                "sensing.particulate".to_string(),
                "sensing.temperature".to_string(),
            ],
            facts: vec![component_spec::FactRef {
                corpus: "device-facts".to_string(),
                source: "sps30.md".to_string(),
            }],
        }
    }

    #[tokio::test]
    async fn a_spec_roundtrips_through_the_memory_graph() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let graph = fresh_graph(tmp.path()).await;
        store_component_spec(&graph, "spire-idf", &sps30())
            .await
            .expect("stored");
        assert_eq!(
            load_component_spec(&graph, "spire-idf", "sps30").await,
            Ok(Some(sps30()))
        );
    }

    #[tokio::test]
    async fn a_component_with_no_spec_reads_as_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let graph = fresh_graph(tmp.path()).await;
        assert_eq!(
            load_component_spec(&graph, "spire-idf", "sps30").await,
            Ok(None)
        );
    }

    #[tokio::test]
    async fn one_components_spec_is_not_anothers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let graph = fresh_graph(tmp.path()).await;
        store_component_spec(&graph, "spire-idf", &sps30())
            .await
            .expect("stored");
        assert_eq!(
            load_component_spec(&graph, "other-lib", "sps30").await,
            Ok(None),
            "the same component name in another library is another spec"
        );
    }

    #[tokio::test]
    async fn a_re_store_replaces_the_children_the_spec_no_longer_states() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let graph = fresh_graph(tmp.path()).await;
        store_component_spec(&graph, "spire-idf", &sps30())
            .await
            .expect("stored");

        let mut edited = sps30();
        edited.provides = vec!["sensing.particulate".to_string()];
        edited.facts.clear();
        store_component_spec(&graph, "spire-idf", &edited)
            .await
            .expect("re-stored");

        assert_eq!(
            load_component_spec(&graph, "spire-idf", "sps30").await,
            Ok(Some(edited)),
            "a removed role is removed, not left beside the new ones"
        );
    }

    #[tokio::test]
    async fn a_spec_with_an_unknown_role_is_refused_before_anything_is_stored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let graph = fresh_graph(tmp.path()).await;
        let mut spec = sps30();
        spec.provides = vec!["sensing.particulates".to_string()];
        assert!(store_component_spec(&graph, "spire-idf", &spec)
            .await
            .is_err());
        assert_eq!(
            load_component_spec(&graph, "spire-idf", "sps30").await,
            Ok(None)
        );
    }

    #[tokio::test]
    async fn the_record_decides_the_spec_and_reads_current_until_the_spec_moves() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let graph = fresh_graph(tmp.path()).await;
        store_component_spec(&graph, "spire-idf", &sps30())
            .await
            .expect("stored");
        assert_eq!(
            load_component_freshness(&graph, "spire-idf", "sps30").await,
            Ok(DesignFreshness::Absent)
        );

        let record = ComponentRecord::now(&sps30(), ComponentSource::Stated, 1);
        store_component_record(&graph, "spire-idf", "sps30", &record)
            .await
            .expect("recorded");
        assert_eq!(
            load_component_record(&graph, "spire-idf", "sps30").await,
            Ok(Some(record.clone()))
        );
        assert_eq!(
            load_component_freshness(&graph, "spire-idf", "sps30").await,
            Ok(DesignFreshness::Current)
        );

        let mut edited = sps30();
        edited.description = "Read PM2.5 and PM10.".to_string();
        store_component_spec(&graph, "spire-idf", &edited)
            .await
            .expect("re-stored");
        assert!(matches!(
            load_component_freshness(&graph, "spire-idf", "sps30").await,
            Ok(DesignFreshness::Stale { .. })
        ));
        // …and the re-store did not eat the record, which is the decision, not part of the spec.
        assert_eq!(
            load_component_record(&graph, "spire-idf", "sps30").await,
            Ok(Some(record))
        );
    }

    #[tokio::test]
    async fn deciding_an_unchanged_spec_keeps_the_record_and_an_edited_one_supersedes_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let graph = fresh_graph(tmp.path()).await;
        store_component_spec(&graph, "spire-idf", &sps30())
            .await
            .expect("stored");

        let first = decide_component_spec(&graph, "spire-idf", &sps30(), ComponentSource::Stated)
            .await
            .expect("decided");
        assert_eq!(first.decision.version, 1);

        let again = decide_component_spec(&graph, "spire-idf", &sps30(), ComponentSource::Filled)
            .await
            .expect("decided");
        assert_eq!(again, first, "an unchanged spec is not decided twice");

        let mut edited = sps30();
        edited.description = "Read PM2.5 and PM10.".to_string();
        store_component_spec(&graph, "spire-idf", &edited)
            .await
            .expect("re-stored");
        let next = decide_component_spec(&graph, "spire-idf", &edited, ComponentSource::Filled)
            .await
            .expect("decided");
        assert_eq!(next.decision.version, 2);
        assert!(next.ratifies(&edited));
        assert_eq!(
            load_component_freshness(&graph, "spire-idf", "sps30").await,
            Ok(DesignFreshness::Current)
        );
    }

    #[tokio::test]
    async fn a_record_without_a_spec_is_refused() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let graph = fresh_graph(tmp.path()).await;
        let record = ComponentRecord::now(&sps30(), ComponentSource::Stated, 1);
        assert!(
            store_component_record(&graph, "spire-idf", "sps30", &record)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_component_that_predates_the_graph_is_backfilled_once_as_a_stub() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let dir = root.join("components").join("sps30");
        std::fs::create_dir_all(dir.join("include")).expect("component dir");
        std::fs::write(
            dir.join("CMakeLists.txt"),
            format!("{}\n", component_kind_line(ComponentKind::Driver)),
        )
        .expect("manifest");
        let graph = fresh_graph(tmp.path()).await;

        let (spec, backfilled) = backfill_component_spec(&graph, root, "spire-idf", "sps30")
            .await
            .expect("backfilled");
        assert!(backfilled);
        assert_eq!(spec, ComponentSpec::new("sps30", ComponentKind::Driver));
        let record = load_component_record(&graph, "spire-idf", "sps30")
            .await
            .expect("record reads")
            .expect("there is a record");
        assert_eq!(record.decision.source, ComponentSource::Stub);
        assert_eq!(record.decision.version, 1);
        assert!(record.ratifies(&spec));

        let (again, backfilled) = backfill_component_spec(&graph, root, "spire-idf", "sps30")
            .await
            .expect("reads back");
        assert!(!backfilled, "the second read is not a second decision");
        assert_eq!(again, spec);
    }

    #[test]
    fn a_spec_that_disagrees_with_the_manifest_is_refused() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("components").join("sps30");
        std::fs::create_dir_all(&dir).expect("component dir");
        std::fs::write(
            dir.join("CMakeLists.txt"),
            format!(
                "{}
",
                component_kind_line(ComponentKind::Driver)
            ),
        )
        .expect("manifest");

        assert!(check_kind(tmp.path(), &sps30()).is_ok());
        let mut lying = sps30();
        lying.kind = ComponentKind::Library;
        let err = check_kind(tmp.path(), &lying).expect_err("refused");
        assert!(err.contains("says it is a library"), "{err}");
        assert!(err.contains("states `driver`"), "{err}");
    }

    #[test]
    fn a_component_whose_manifest_states_no_kind_is_not_checked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("components").join("sps30");
        std::fs::create_dir_all(&dir).expect("component dir");
        std::fs::write(
            dir.join("CMakeLists.txt"),
            "idf_component_register()
",
        )
        .expect("manifest");
        assert!(check_kind(tmp.path(), &sps30()).is_ok());
    }

    #[test]
    fn a_name_with_no_directory_is_refused_with_the_path_it_looked_in() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = implied_component_spec(tmp.path(), "sps30").expect_err("refused");
        assert!(err.contains("components/sps30"), "{err}");
        assert!(err.contains("no component"), "{err}");
    }

    #[test]
    fn a_component_without_a_stated_kind_cannot_be_backfilled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("components").join("handwritten"))
            .expect("component dir");
        std::fs::write(
            tmp.path().join("components/handwritten/CMakeLists.txt"),
            "idf_component_register()\n",
        )
        .expect("manifest");
        let err = implied_component_spec(tmp.path(), "handwritten").expect_err("refused");
        assert!(err.contains("states no kind"), "{err}");
    }

    #[test]
    fn the_rendered_copy_lands_beside_the_component() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("components").join("sps30");
        std::fs::create_dir_all(&dir).expect("component dir");
        let path = write_component_spec_file(tmp.path(), "sps30", &sps30()).expect("written");
        assert_eq!(path, dir.join(COMPONENT_SPEC_FILE));
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("name: sps30"), "{text}");
        assert!(text.contains("part: sps30"), "{text}");
    }
}
