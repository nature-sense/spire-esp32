// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **Graph transport** — the one place that speaks to the memory-graph actor on behalf of a spec.
//!
//! Every spec Spire stores (an `AppSpec`, an application's composition, a component's contract) is
//! persisted the same way: one `AttrNode` per logical node, one `Custom(<predicate>)` relationship per
//! edge, and a store-side *envelope* that a reader has to peel back to get the spec's own properties.
//! This module is that envelope and those round-trips, in one place, so a writer never has to
//! re-implement — or subtly disagree about — the codec.
//!
//! # The property envelope
//!
//! Scalars are stored verbatim; arrays and objects are stored as canonical JSON strings under the
//! [`PROP_JSON_PREFIX`] (`json_`) prefix. The prefix has to be a legal *property name*: the AttrNode
//! store inlines the key into GQL (`SET n.<key> = …`, `INSERT (n:SPIRE_NODE {<key>: …})`) and a `:`
//! is not accepted there — `json:wiring` made SeleneDB refuse the whole write, so every array/object
//! property was silently lost. An underscore keeps the same "this value is encoded JSON" reading while
//! staying storable.
//!
//! [`PROP_LOGICAL`] is the store's own marker (the node's logical name before `{scope}::` scoping) and
//! is never part of a spec; [`decode_props`] drops it. Each spec's rendered review copy
//! (`composition.spire`, `spec_md`, `SPIRE.component.yaml`) is likewise store-side and is filtered by
//! the caller that reads the anchor, where it lives.
//!
//! # Naming
//!
//! A stored node is named `{scope}::{logical}`, because a logical name is only unique within its
//! scope (a project, a component library). [`mem_name`]/[`un_mem_name`] are the pair, so the read
//! side can tell *this* scope's nodes from a sibling's in the shared graph database.

use std::collections::HashMap;

use spire_core::models::memory_graph::{AttrNode, GraphEdge, RelationshipInput, RelationshipType};
use spire_core::subsystems::graph::memory_graph::MemoryGraphMessage;
use tokio::sync::{mpsc, oneshot};

/// Property that records a node's logical name (before the `{scope}::` prefix) so reads are
/// unambiguous without re-parsing names. Store-side only; never decoded into a spec.
pub(crate) const PROP_LOGICAL: &str = "logical";
/// Complex (non-scalar) properties are serialized to canonical JSON strings under this prefix;
/// scalar values are stored verbatim.
pub(crate) const PROP_JSON_PREFIX: &str = "json_";
/// A ceiling that means "every node of this kind in this store", for a query that must walk a whole
/// decomposition rather than pick one node.
pub(crate) const QUERY_LIMIT_ALL: u32 = 100_000;

/// Scoped memory name `{scope}::{logical}`.
pub(crate) fn mem_name(scope: &str, logical: &str) -> String {
    format!("{scope}::{logical}")
}

/// The logical name behind a scoped memory name, when `mem` belongs to `scope`.
pub(crate) fn un_mem_name<'a>(scope: &str, mem: &'a str) -> Option<&'a str> {
    mem.strip_prefix(&format!("{scope}::"))
}

/// Property encoding: scalars verbatim, arrays/objects as canonical JSON strings under
/// [`PROP_JSON_PREFIX`] so the open AttrNode store never loses structure.
pub(crate) fn encode_props(
    props: &[(String, serde_json::Value)],
) -> HashMap<String, serde_json::Value> {
    let mut out = HashMap::with_capacity(props.len());
    for (k, v) in props {
        match v {
            serde_json::Value::String(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::Bool(_) => {
                out.insert(k.clone(), v.clone());
            }
            _ => {
                out.insert(
                    format!("{PROP_JSON_PREFIX}{k}"),
                    serde_json::json!(v.to_string()),
                );
            }
        }
    }
    out
}

/// Inverse of [`encode_props`]: `json_`-prefixed keys are parsed back, and the store's own
/// [`PROP_LOGICAL`] marker is dropped.
pub(crate) fn decode_props(
    map: &HashMap<String, serde_json::Value>,
) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::with_capacity(map.len());
    for (k, v) in map {
        if let Some(real) = k.strip_prefix(PROP_JSON_PREFIX) {
            if let Some(s) = v.as_str() {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                    out.push((real.to_string(), parsed));
                    continue;
                }
            }
            out.push((real.to_string(), v.clone()));
        } else if k != PROP_LOGICAL {
            out.push((k.clone(), v.clone()));
        }
    }
    out
}

// ============================================================================
// Round-trips — one message per operation. Failures come back as strings so a
// caller can decide whether to log and carry on or refuse.
// ============================================================================

/// Store a new node (insert-only; a name collision is the caller's problem).
pub(crate) async fn store_node(
    graph: &mpsc::Sender<MemoryGraphMessage>,
    node: AttrNode,
) -> Result<AttrNode, String> {
    let (reply, rx) = oneshot::channel();
    graph
        .send(MemoryGraphMessage::StoreAttrNode {
            node,
            reply_to: reply,
        })
        .await
        .map_err(|e| format!("memory graph channel closed: {e}"))?;
    rx.await
        .map_err(|e| format!("store reply lost: {e}"))?
        .map_err(|e| format!("store failed: {e}"))
}

/// Upsert a node by `(node_type, subtype, name)`.
///
/// **Merge is for nodes a spec owns**, never for a node that other nodes hang off: the store merges
/// with a `DETACH DELETE` and a re-insert, which would take the node's relationships with it. A
/// caller that must keep an edge (a component's edit history off its module node, say) reads first
/// and creates once instead.
pub(crate) async fn merge_node(
    graph: &mpsc::Sender<MemoryGraphMessage>,
    node: AttrNode,
) -> Result<AttrNode, String> {
    let (reply, rx) = oneshot::channel();
    graph
        .send(MemoryGraphMessage::MergeAttrNode {
            node,
            reply_to: reply,
        })
        .await
        .map_err(|e| format!("memory graph channel closed: {e}"))?;
    rx.await
        .map_err(|e| format!("merge reply lost: {e}"))?
        .map_err(|e| format!("merge failed: {e}"))
}

/// Create one relationship of `edge_type` from `from_id` to `to_id`.
pub(crate) async fn create_edge(
    graph: &mpsc::Sender<MemoryGraphMessage>,
    edge_type: RelationshipType,
    from_id: &str,
    to_id: &str,
) -> Result<GraphEdge, String> {
    let (reply, rx) = oneshot::channel();
    graph
        .send(MemoryGraphMessage::CreateRelationship {
            rel: RelationshipInput {
                edge_type,
                from_id: from_id.to_string(),
                to_id: to_id.to_string(),
                properties: None,
                weight: None,
            },
            reply_to: reply,
        })
        .await
        .map_err(|e| format!("memory graph channel closed: {e}"))?;
    rx.await
        .map_err(|e| format!("relationship reply lost: {e}"))?
        .map_err(|e| format!("relationship failed: {e}"))
}

/// Create one `Custom(predicate)` relationship — the edge a decomposed spec stores.
///
/// The predicate survives the store-and-read as `Custom` because spire-core writes the label as the
/// predicate text and reads it back with `relationship_type_from_gql_label` (the inverse of the
/// writer), so a reader can match on the predicate rather than see it flattened to `Unknown`.
pub(crate) async fn create_rel(
    graph: &mpsc::Sender<MemoryGraphMessage>,
    predicate: &str,
    from_id: &str,
    to_id: &str,
) -> Result<GraphEdge, String> {
    create_edge(
        graph,
        RelationshipType::Custom(predicate.to_string()),
        from_id,
        to_id,
    )
    .await
}

/// Query nodes by any of the store's identity axes.
pub(crate) async fn query_nodes(
    graph: &mpsc::Sender<MemoryGraphMessage>,
    node_type: Option<&str>,
    subtype: Option<&str>,
    name: Option<&str>,
    limit: u32,
) -> Result<Vec<AttrNode>, String> {
    let (reply, rx) = oneshot::channel();
    graph
        .send(MemoryGraphMessage::QueryAttrNodes {
            node_type: node_type.map(str::to_string),
            subtype: subtype.map(str::to_string),
            name: name.map(str::to_string),
            limit: Some(limit),
            reply_to: reply,
        })
        .await
        .map_err(|e| format!("memory graph channel closed: {e}"))?;
    rx.await
        .map_err(|e| format!("query reply lost: {e}"))?
        .map_err(|e| format!("query failed: {e}"))
}

/// Query nodes by scalar property equality — the read a caller uses when the identity it knows is a
/// property ("every edit of `sps30`") rather than a name.
pub(crate) async fn query_by_props(
    graph: &mpsc::Sender<MemoryGraphMessage>,
    subtype: &str,
    props: &[(&str, &str)],
    limit: u32,
) -> Result<Vec<AttrNode>, String> {
    let (reply, rx) = oneshot::channel();
    graph
        .send(MemoryGraphMessage::QueryAttrNodesWhere {
            subtype: Some(subtype.to_string()),
            props: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            limit: Some(limit),
            reply_to: reply,
        })
        .await
        .map_err(|e| format!("memory graph channel closed: {e}"))?;
    rx.await
        .map_err(|e| format!("query reply lost: {e}"))?
        .map_err(|e| format!("query failed: {e}"))
}

/// Delete one node (and the relationships the store detaches with it).
pub(crate) async fn delete_node(
    graph: &mpsc::Sender<MemoryGraphMessage>,
    node_id: &str,
) -> Result<(), String> {
    let (reply, rx) = oneshot::channel();
    graph
        .send(MemoryGraphMessage::DeleteNode {
            id: node_id.to_string(),
            reply_to: reply,
        })
        .await
        .map_err(|e| format!("memory graph channel closed: {e}"))?;
    rx.await
        .map_err(|e| format!("delete reply lost: {e}"))?
        .map_err(|e| format!("delete failed: {e}"))
}

/// Every relationship touching `node_id`, in both directions.
pub(crate) async fn rels_of_node(
    graph: &mpsc::Sender<MemoryGraphMessage>,
    node_id: &str,
) -> Result<Vec<GraphEdge>, String> {
    let (reply, rx) = oneshot::channel();
    graph
        .send(MemoryGraphMessage::GetRelationships {
            node_id: node_id.to_string(),
            reply_to: reply,
        })
        .await
        .map_err(|e| format!("memory graph channel closed: {e}"))?;
    rx.await
        .map_err(|e| format!("relationships reply lost: {e}"))?
        .map_err(|e| format!("relationships query failed: {e}"))
}
