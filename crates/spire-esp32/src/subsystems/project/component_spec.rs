// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **Graph-native component spec** — a component's contract as a decomposed property graph.
//!
//! The same discipline the application's composition follows, one level down: a component's spec is
//! typed nodes and edges in the memory graph rather than a blob in a file, and `SPIRE.component.yaml`
//! is a rendered copy of it, never its source. What the spec states is what a create/edit flow needs
//! to start from — there is no second place to look:
//!
//! * the **anchor** ([`node::ANCHOR`]) — what the component is: its name, its [`ComponentKind`], the
//!   fill prose that says what it should become, and the invariants it must hold either way;
//! * the **device** ([`node::DEVICE`], [`edge::HAS_DEVICE`]) — the part, the bus and the address,
//!   when the component drives one. The `part` is the authoritative key into the `device-facts`
//!   corpus, so a driver is looked up by what it *is* rather than by the word its directory uses;
//! * the **roles** ([`node::ROLE`], [`edge::PROVIDES`]) — the capability paths the component
//!   provides, in the order the spec states them, each one a path the shared
//!   [`capability_vocabulary`] knows;
//! * the **facts** ([`node::FACT`], [`edge::HAS_FACT`]) — the corpus documents the spec stands on,
//!   so a reader can tell a protocol written from the datasheet from one written from nothing;
//! * the **design record** ([`node::DESIGN_RECORD`]) — the shared [`Decision`] that ratified this
//!   spec, exactly as an application's composition carries one.
//!
//! This module is pure: [`decompose`] turns a [`ComponentSpec`] into a [`ComponentGraph`] of node/edge
//! *specs*, [`reconstruct`] rebuilds it, and round-tripping is a hard property
//! (`reconstruct(decompose(spec)) == spec`). The actor layer is
//! [`super::component_persist`], which maps the specs onto `AttrNode`/`CreateRelationship` messages.
//!
//! # The record, and staleness
//!
//! A spec can be **stored** without having been **decided** — the backfill of a component that
//! predates this graph writes one from what the tree already says — so the decision is a node of its
//! own rather than a flag on the anchor. [`ComponentRecord`] carries the door
//! ([`ComponentSource`]) and the shared [`Decision`]; [`freshness`] then answers whether the record
//! still ratifies the spec in front of it, the way an application's record does.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use super::decision::{self, Decision, DesignFreshness};
use crate::build::idf_projects::{
    document_is_the_parts_own, ident_of, ComponentKind, DEVICE_FACTS_DOMAIN,
};
use crate::capability_vocabulary;

/// Node-type discriminators for the decomposed component spec (stored as the `subtype` of an
/// `idf_component` AttrNode — see [`super::component_persist`]).
pub mod node {
    pub const ANCHOR: &str = "anchor";
    /// The device behind a driver: part, bus, address.
    pub const DEVICE: &str = "device";
    /// One capability path the component provides.
    pub const ROLE: &str = "role";
    /// One corpus document the spec stands on.
    pub const FACT: &str = "fact";
    /// The decision that produced this spec (one per component; see [`super::ComponentRecord`]).
    pub const DESIGN_RECORD: &str = "design_record";
}

/// Edge predicates of the decomposed component spec.
pub mod edge {
    /// Anchor → the [`node::DEVICE`] the component drives.
    pub const HAS_DEVICE: &str = "HAS_DEVICE";
    /// Anchor → a [`node::ROLE`] capability path.
    pub const PROVIDES: &str = "PROVIDES";
    /// Anchor → a [`node::FACT`] the spec stands on.
    pub const HAS_FACT: &str = "HAS_FACT";
    /// Anchor → the [`node::DESIGN_RECORD`] that ratifies the spec.
    pub const HAS_DESIGN_RECORD: &str = "HAS_DESIGN_RECORD";
}

/// Logical name of the anchor node. The store scopes it `{library}::{component}`; `reconstruct`
/// finds the anchor by its type, so the node's name is only a label here.
pub const ROOT: &str = "component";

/// Logical name of the single design-record node. The store scopes it
/// `{library}::{component}::{this}`, so a component's record never collides with another's.
pub const DESIGN_RECORD_NAME: &str = "component_design_record";

/// Ordering property carried by ordered collections (`provides`, `facts`).
pub const PROP_ORDER: &str = "order";

/// The component's name. **Not** `name`: the AttrNode store reserves `name` for the node's own
/// identity and silently drops a property of that name (see `memory_graph`'s `BASE_KEYS`), which
/// would leave a stored spec unable to say which component it describes. `component` is the word
/// the edit history already uses for the same fact.
const P_NAME: &str = "component";
const P_KIND: &str = "kind";
const P_INVARIANTS: &str = "invariants";
/// The record's one-line summary. Not `description` either — same reserved-key reason; the anchor's
/// fill prose rides on the node's own `description` field instead of a property.
const P_DESCRIPTION: &str = "summary";
const P_PART: &str = "part";
const P_BUS: &str = "bus";
const P_ADDRESS: &str = "address";
const P_PATH: &str = "path";
const P_CORPUS: &str = "corpus";
const P_SOURCE: &str = "source";

/// The graph key of the `index`th role node.
fn role_node(index: usize) -> String {
    format!("role.{index}")
}

/// The graph key of the `index`th fact node.
fn fact_node(index: usize) -> String {
    format!("fact.{index}")
}

/// A node specification in the decomposed component spec (logical, pre-actor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentNode {
    /// Discriminator (one of the `node::` constants).
    pub node_type: String,
    /// Logical, graph-unique identity (`component`, `device`, `role.0`…).
    pub name: String,
    pub description: Option<String>,
    pub properties: Vec<(String, Value)>,
}

impl ComponentNode {
    fn new(node_type: &str, name: &str) -> Self {
        Self {
            node_type: node_type.to_string(),
            name: name.to_string(),
            description: None,
            properties: Vec::new(),
        }
    }

    fn described(mut self, description: &str) -> Self {
        if !description.is_empty() {
            self.description = Some(description.to_string());
        }
        self
    }

    fn with(mut self, key: &str, value: Value) -> Self {
        self.properties.push((key.to_string(), value));
        self
    }
}

/// An edge specification (endpoints by logical node `name`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentEdge {
    pub predicate: String,
    pub from_name: String,
    pub to_name: String,
}

/// The decomposed component spec as pure data (no actor I/O).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentGraph {
    pub nodes: Vec<ComponentNode>,
    pub edges: Vec<ComponentEdge>,
}

fn prop<'a>(n: &'a ComponentNode, key: &str) -> Option<&'a Value> {
    n.properties.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn prop_str(n: &ComponentNode, key: &str) -> Option<String> {
    prop(n, key).and_then(|v| v.as_str().map(str::to_string))
}

fn prop_list(n: &ComponentNode, key: &str) -> Vec<String> {
    prop(n, key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn order_of(n: &ComponentNode) -> usize {
    prop(n, PROP_ORDER).and_then(Value::as_u64).unwrap_or(0) as usize
}

/// Child nodes of `parent` via `predicate`, ordered by their `order` property.
fn children<'a>(g: &'a ComponentGraph, parent: &str, predicate: &str) -> Vec<&'a ComponentNode> {
    let mut out: Vec<&ComponentNode> = g
        .edges
        .iter()
        .filter(|e| e.predicate == predicate && e.from_name == parent)
        .filter_map(|e| g.nodes.iter().find(|n| n.name == e.to_name))
        .collect();
    out.sort_by_key(|n| order_of(n));
    out
}

fn edge(predicate: &str, from: &str, to: &str) -> ComponentEdge {
    ComponentEdge {
        predicate: predicate.to_string(),
        from_name: from.to_string(),
        to_name: to.to_string(),
    }
}

/// The device a driver drives.
///
/// `part` is the **authoritative** key into the `device-facts` corpus — the part number the datasheet
/// is named for, which is not always the word the directory uses (`sht20` may drive a `SHT20`, but a
/// `bme280` driver is looked up as `bme280` even if the directory were called `environment`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentDevice {
    /// The part number, as `device-facts` keys it.
    pub part: String,
    /// The bus it sits on (`i2c`, `spi`, …), lower-cased.
    pub bus: String,
    /// The bus address, when the part has one (`0x44`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// One corpus document the spec stands on.
///
/// A *reference*, not the text: the fact itself lives in the knowledge store, and copying it here
/// would make the spec a stale transcript. `corpus` + `source` is what a reader (or a later fill)
/// uses to go and get it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FactRef {
    /// The corpus the fact came from (`device-facts`, `esp-idf-lib`).
    pub corpus: String,
    /// The document within it — its file name (`sht20.md`), which is both the corpus's key and the
    /// form a rendered spec reads.
    pub source: String,
}

/// **A component's contract** — what it is, and what a fill is asked to make it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentSpec {
    /// The component as the library names it (`sps30`), normalised to its identifier.
    pub name: String,
    /// What it is — and the question the skeleton follows from.
    pub kind: ComponentKind,
    /// The **fill prose**: what this component should become, in the words the edit box starts from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// What it must hold true regardless of how it is written.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub invariants: Vec<String>,
    /// The device behind it, when it drives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<ComponentDevice>,
    /// The capability paths it provides, in the order the spec states them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>,
    /// The corpus documents the spec stands on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<FactRef>,
}

impl ComponentSpec {
    /// The least a component can state: its name and kind. Everything else is genuinely optional —
    /// a component added as a stub has no prose, no device facts and no roles yet.
    pub fn new(name: &str, kind: ComponentKind) -> Self {
        Self {
            name: ident_of(name),
            kind,
            description: String::new(),
            invariants: Vec::new(),
            device: None,
            provides: Vec::new(),
            facts: Vec::new(),
        }
    }

    /// **Does this spec say anything the tree does not?**
    ///
    /// The question a row asks before it calls a component filled. A stored spec is not the same as a
    /// described one: the backfill of a component that predates the graph — and the stub half of
    /// `idf_add_component` — writes a spec from the manifest and decides it, so "stored" and "ratified"
    /// are both true of a component nobody has said anything about. What separates them is a *word*: prose,
    /// an invariant, a device, a role or a fact. The name and kind are excluded because the manifest
    /// already states both, and they are the only two fields [`Self::new`] can fill.
    ///
    /// A predicate, not a verdict: it says nothing about whether the decision still stands
    /// ([`freshness`]) and nothing about whether the prose is any good.
    pub fn has_contract(&self) -> bool {
        !self.description.trim().is_empty()
            || !self.invariants.is_empty()
            || self.device.is_some()
            || !self.provides.is_empty()
            || !self.facts.is_empty()
    }

    /// Refuse a spec that states something the rest of the system cannot honour.
    ///
    /// Four checks, each one a contract that would otherwise lie:
    /// * a **role** has to be a capability path the vocabulary knows — a misspelling would read as
    ///   "provides nothing", which is the silent absence the vocabulary exists to prevent;
    /// * a **device** belongs to a driver: a library naming one is a device it never touches;
    /// * the device's own fields and the component's name have to be there, because the graph keys
    ///   and the `device-facts` lookup both need them;
    /// * a **fact** has to name a fetchable document, and a `device-facts` fact has to be *this
    ///   part's own*. That corpus is keyed by part — one document per part, named for it — so a
    ///   neighbour's document recorded here is another device's protocol kept as this one's. The
    ///   retrieval seam promotes the part's own document for exactly this reason
    ///   ([`document_is_the_parts_own`]); refusing it at the record is what stops it returning
    ///   through a backfill, a hand edit or a merge.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("a component spec needs a name".to_string());
        }
        if let Some(unknown) = self
            .provides
            .iter()
            .find(|path| !capability_vocabulary::is_known_path(path))
        {
            return Err(format!(
                "'{unknown}' is not a capability path the vocabulary knows — add it to \
                 schema/capabilities.yaml or correct the spelling; a role nobody named would read \
                 as this component providing nothing"
            ));
        }
        match (&self.device, self.kind) {
            (None, _) => {}
            (Some(_), ComponentKind::Driver) => {}
            (Some(_), kind) => {
                return Err(format!(
                    "a {} names a device, but has none: only a driver owns a part, a bus and an \
                     address",
                    kind.as_str()
                ));
            }
        }
        if let Some(device) = &self.device {
            if device.part.trim().is_empty() {
                return Err(
                    "a device needs a part number (the key into `device-facts`)".to_string()
                );
            }
            if device.bus.trim().is_empty() {
                return Err(format!(
                    "the device '{}' needs the bus it sits on",
                    device.part
                ));
            }
        }
        if let Some(blank) = self.invariants.iter().find(|i| i.trim().is_empty()) {
            return Err(format!("an invariant is blank: {blank:?}"));
        }
        if let Some(fact) = self
            .facts
            .iter()
            .find(|f| f.corpus.trim().is_empty() || f.source.trim().is_empty())
        {
            return Err(format!(
                "a fact names no corpus or no document (corpus {:?}, source {:?}) — a reference \
                 nothing can be fetched from is not a record of anything",
                fact.corpus, fact.source
            ));
        }
        for fact in &self.facts {
            if fact.corpus != DEVICE_FACTS_DOMAIN {
                continue;
            }
            let Some(device) = &self.device else {
                return Err(format!(
                    "the spec stands on '{}' from device-facts but names no device to look it up \
                     by",
                    fact.source
                ));
            };
            if !document_is_the_parts_own(&device.part, &fact.source) {
                return Err(format!(
                    "'{}' is recorded as a fact about '{}', but device-facts is keyed by part and \
                     that is another part's document — a neighbour may be context, never this \
                     component's protocol",
                    fact.source, device.part
                ));
            }
        }
        Ok(())
    }
}

/// Decompose a component spec into its graph: the anchor, the device and roles/facts when stated,
/// and one ordered edge per collection.
///
/// The kinds of node and edge are deliberately few — a spec states a device, some capabilities and
/// some sources, and every one of them is a fact a fill prompt reads. Anything a fill *thinks* lives
/// in the code it writes, not here.
pub fn decompose(spec: &ComponentSpec) -> ComponentGraph {
    let mut g = ComponentGraph::default();
    g.nodes.push(
        ComponentNode::new(node::ANCHOR, ROOT)
            .described(&spec.description)
            .with(P_NAME, json!(spec.name))
            .with(P_KIND, json!(spec.kind.as_str()))
            .with(P_INVARIANTS, json!(spec.invariants)),
    );

    if let Some(device) = &spec.device {
        let name = "device";
        let mut n = ComponentNode::new(node::DEVICE, name)
            .described(&format!("{} on {}", device.part, device.bus))
            .with(P_PART, json!(device.part))
            .with(P_BUS, json!(device.bus));
        if let Some(address) = &device.address {
            n = n.with(P_ADDRESS, json!(address));
        }
        g.nodes.push(n);
        g.edges.push(edge(edge::HAS_DEVICE, ROOT, name));
    }

    for (index, path) in spec.provides.iter().enumerate() {
        let name = role_node(index);
        g.nodes.push(
            ComponentNode::new(node::ROLE, &name)
                .described(path)
                .with(P_PATH, json!(path))
                .with(PROP_ORDER, json!(index)),
        );
        g.edges.push(edge(edge::PROVIDES, ROOT, &name));
    }

    for (index, fact) in spec.facts.iter().enumerate() {
        let name = fact_node(index);
        g.nodes.push(
            ComponentNode::new(node::FACT, &name)
                .with(P_CORPUS, json!(fact.corpus))
                .with(P_SOURCE, json!(fact.source))
                .with(PROP_ORDER, json!(index)),
        );
        g.edges.push(edge(edge::HAS_FACT, ROOT, &name));
    }

    g
}

/// Rebuild a component spec from its decomposition.
///
/// A node that is there but incomplete is **refused**, not defaulted: a device with no part would
/// silently stop naming the corpus a driver is written from, and a spec with no kind cannot say what
/// skeleton it wants. A *missing optional* node is ordinary — most components have no device and no
/// facts.
pub fn reconstruct(g: &ComponentGraph) -> Result<ComponentSpec, String> {
    let anchor = g
        .nodes
        .iter()
        .find(|n| n.node_type == node::ANCHOR)
        .ok_or_else(|| "the component spec has no anchor node".to_string())?;
    let name = prop_str(anchor, P_NAME)
        .ok_or_else(|| "the component anchor is missing its 'name' property".to_string())?;
    let kind = prop_str(anchor, P_KIND)
        .ok_or_else(|| "the component anchor is missing its 'kind' property".to_string())?;
    let kind = ComponentKind::from_str(&kind).ok_or_else(|| {
        format!(
            "'{}' is not a component kind (driver, library, actor or stage)",
            kind
        )
    })?;

    let device = match children(g, ROOT, edge::HAS_DEVICE).first() {
        None => None,
        Some(n) => {
            let part = prop_str(n, P_PART).ok_or_else(|| {
                "the component's device is missing its 'part' property".to_string()
            })?;
            let bus = prop_str(n, P_BUS).ok_or_else(|| {
                "the component's device is missing its 'bus' property".to_string()
            })?;
            Some(ComponentDevice {
                part,
                bus,
                address: prop_str(n, P_ADDRESS),
            })
        }
    };

    let provides = children(g, ROOT, edge::PROVIDES)
        .iter()
        .map(|n| {
            prop_str(n, P_PATH)
                .ok_or_else(|| "a component role is missing its 'path' property".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;

    let facts = children(g, ROOT, edge::HAS_FACT)
        .iter()
        .map(|n| {
            Ok(FactRef {
                corpus: prop_str(n, P_CORPUS).ok_or_else(|| {
                    "a component fact is missing its 'corpus' property".to_string()
                })?,
                source: prop_str(n, P_SOURCE).ok_or_else(|| {
                    "a component fact is missing its 'source' property".to_string()
                })?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(ComponentSpec {
        name,
        kind,
        description: anchor.description.clone().unwrap_or_default(),
        invariants: prop_list(anchor, P_INVARIANTS),
        device,
        provides,
        facts,
    })
}

/// **How a component spec came to be** — the door it arrived through.
///
/// Recorded rather than inferred, for the same reason an application's door is: a spec a person
/// *stated* is not the same evidence as a skeleton that nobody has filled, and a record that did not
/// say which would make "this component exists" mean two different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentSource {
    /// A person stated what the component is — the `idf_add_component` door.
    Stated,
    /// The skeleton exists and its code is still the stub: the backfill of a component that predates
    /// its spec, or one added **stub only**.
    Stub,
    /// The fill wrote the code and it passed the component's own gate.
    Filled,
}

impl ComponentSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ComponentSource::Stated => "stated",
            ComponentSource::Stub => "stub",
            ComponentSource::Filled => "filled",
        }
    }

    pub fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "stated" => Ok(ComponentSource::Stated),
            "stub" => Ok(ComponentSource::Stub),
            "filled" => Ok(ComponentSource::Filled),
            other => Err(format!(
                "'{other}' is not a component source (expected `stated`, `stub` or `filled`)"
            )),
        }
    }
}

impl decision::Source for ComponentSource {
    fn as_str(self) -> &'static str {
        ComponentSource::as_str(self)
    }

    fn from_name(name: &str) -> Result<Self, String> {
        ComponentSource::from_name(name)
    }
}

/// **The component record**: the decision that produced a spec, plus the summary that makes it read
/// as one statement.
///
/// The versioned core is the shared [`Decision`]; `kind` and `description` mirror the anchor the way
/// an application's framework and justification mirror theirs. Stored as a [`node::DESIGN_RECORD`]
/// child of the anchor ([`edge::HAS_DESIGN_RECORD`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentRecord {
    /// The shared versioned core: the door, when, the version and what it ratifies.
    pub decision: Decision<ComponentSource>,
    /// The kind the spec states (mirrored so the record reads on its own).
    pub kind: ComponentKind,
    /// What the component is, in the words the record was decided from.
    pub description: String,
}

impl ComponentRecord {
    /// The record for `spec`, decided now, through `source`, as spec `version`.
    pub fn now(spec: &ComponentSpec, source: ComponentSource, version: u32) -> Self {
        Self::at(spec, source, version, Utc::now())
    }

    /// [`Self::now`] with the clock passed in, so a caller that already stamped the decision records
    /// *that* instant rather than a second one microseconds later.
    pub fn at(
        spec: &ComponentSpec,
        source: ComponentSource,
        version: u32,
        decided_at: DateTime<Utc>,
    ) -> Self {
        Self {
            decision: Decision::at(spec, source, version, decided_at),
            kind: spec.kind,
            description: spec.description.clone(),
        }
    }

    /// Whether this record still ratifies `spec` — [`freshness`], for the single-record case.
    pub fn ratifies(&self, spec: &ComponentSpec) -> bool {
        self.decision.ratifies(spec)
    }
}

/// The `design_record` node for `record` — a projection of the *decision*, not of the spec.
pub fn component_record_node(record: &ComponentRecord) -> ComponentNode {
    let mut n = ComponentNode::new(node::DESIGN_RECORD, DESIGN_RECORD_NAME)
        .described(&record.description)
        .with(P_KIND, json!(record.kind.as_str()))
        .with(P_DESCRIPTION, json!(record.description));
    n.properties.extend(record.decision.props());
    n
}

/// Attach `record` to a decomposed spec: the record node, plus the anchor's
/// [`edge::HAS_DESIGN_RECORD`].
///
/// Deliberately **not** part of [`decompose`]: decomposing a spec is not deciding one. [`reconstruct`]
/// ignores the node, so attaching a record never changes the spec read back.
pub fn attach_component_record(g: &mut ComponentGraph, record: &ComponentRecord) {
    g.nodes.push(component_record_node(record));
    g.edges
        .push(edge(edge::HAS_DESIGN_RECORD, ROOT, DESIGN_RECORD_NAME));
}

/// Read the [`ComponentRecord`] back out of a decomposed spec, when it carries one.
///
/// `Ok(None)` means *no record*, never *a broken record*: a node that is there but incomplete is
/// refused, because reading a corrupt decision as no decision would let a spec be re-decided over a
/// decision nobody superseded.
pub fn reconstruct_component_record(g: &ComponentGraph) -> Result<Option<ComponentRecord>, String> {
    let Some(n) = g.nodes.iter().find(|n| n.node_type == node::DESIGN_RECORD) else {
        return Ok(None);
    };
    let kind = prop_str(n, P_KIND)
        .ok_or_else(|| "the component record is missing its 'kind' property".to_string())?;
    let kind = ComponentKind::from_str(&kind)
        .ok_or_else(|| format!("'{}' is not a component kind", kind))?;
    Ok(Some(ComponentRecord {
        decision: Decision::from_props(&n.properties)?,
        kind,
        description: prop_str(n, P_DESCRIPTION).unwrap_or_default(),
    }))
}

/// Does `record` still ratify `spec`? The pure derivation behind
/// [`load_component_freshness`](super::component_persist::load_component_freshness).
pub fn freshness(record: Option<&ComponentRecord>, spec: &ComponentSpec) -> DesignFreshness {
    match record {
        None => DesignFreshness::Absent,
        Some(record) => decision::freshness(Some(&record.decision), &decision::fingerprint(spec)),
    }
}

/// Add the facts a fill stood on to the spec's, keeping the first occurrence of each
/// `corpus` + `source` pair.
///
/// Appended rather than replaced: the spec is a growing account of what the component was written
/// from, and a fill that retrieved nothing new must not erase the datasheet an earlier one used.
pub fn merge_facts(existing: &mut Vec<FactRef>, new: &[FactRef]) {
    for fact in new {
        if !existing
            .iter()
            .any(|f| f.corpus == fact.corpus && f.source == fact.source)
        {
            existing.push(fact.clone());
        }
    }
}

/// The rendered `SPIRE.component.yaml` copy of `spec` — a file a person can read and edit the way
/// they read `composition.spire`. It is a projection: the graph is what a flow reads.
pub fn render_yaml(spec: &ComponentSpec) -> String {
    serde_yaml::to_string(spec).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sps30() -> ComponentSpec {
        ComponentSpec {
            name: "sps30".to_string(),
            kind: ComponentKind::Driver,
            description: "Read PM2.5 from the SPS30 over I2C.".to_string(),
            invariants: vec![
                "no blocking read longer than the measurement interval".to_string(),
                "a failed checksum is a reading, not an exception".to_string(),
            ],
            device: Some(ComponentDevice {
                part: "sps30".to_string(),
                bus: "i2c".to_string(),
                address: Some("0x69".to_string()),
            }),
            provides: vec![
                "sensing.particulate".to_string(),
                "sensing.temperature".to_string(),
            ],
            facts: vec![FactRef {
                corpus: "device-facts".to_string(),
                source: "sps30.md".to_string(),
            }],
        }
    }

    fn count(g: &ComponentGraph, subtype: &str) -> usize {
        g.nodes.iter().filter(|n| n.node_type == subtype).count()
    }

    #[test]
    fn a_component_spec_roundtrips_through_its_graph() {
        let spec = sps30();
        let g = decompose(&spec);
        assert_eq!(reconstruct(&g), Ok(spec), "the contract survives the graph");
    }

    #[test]
    fn the_shape_is_the_taxonomy_and_nothing_else() {
        let g = decompose(&sps30());
        assert_eq!(count(&g, node::ANCHOR), 1);
        assert_eq!(count(&g, node::DEVICE), 1);
        assert_eq!(count(&g, node::ROLE), 2);
        assert_eq!(count(&g, node::FACT), 1);
        assert_eq!(
            count(&g, node::DESIGN_RECORD),
            0,
            "decomposing is not deciding"
        );
        for (predicate, expected) in [
            (edge::HAS_DEVICE, 1),
            (edge::PROVIDES, 2),
            (edge::HAS_FACT, 1),
        ] {
            assert_eq!(
                g.edges.iter().filter(|e| e.predicate == predicate).count(),
                expected,
                "{predicate}"
            );
        }
    }

    #[test]
    fn extra_roles_keep_their_order() {
        let mut spec = sps30();
        spec.provides = vec![
            "sensing.temperature".to_string(),
            "sensing.particulate".to_string(),
            "sensing.humidity".to_string(),
        ];
        let g = decompose(&spec);
        assert_eq!(
            reconstruct(&g).expect("reads back").provides,
            spec.provides,
            "the order the spec states is the order a reader sees"
        );
    }

    #[test]
    fn a_library_roundtrips_with_no_device_and_no_facts() {
        let spec = ComponentSpec {
            name: "moving_average".to_string(),
            kind: ComponentKind::Library,
            description: "A windowed mean.".to_string(),
            invariants: Vec::new(),
            device: None,
            provides: Vec::new(),
            facts: Vec::new(),
        };
        let g = decompose(&spec);
        assert_eq!(count(&g, node::DEVICE), 0);
        assert_eq!(reconstruct(&g), Ok(spec));
    }

    #[test]
    fn the_record_ratifies_the_spec_it_was_decided_from_and_goes_stale_when_it_moves() {
        let spec = sps30();
        let record = ComponentRecord::now(&spec, ComponentSource::Stated, 1);
        assert!(record.ratifies(&spec));
        assert_eq!(freshness(Some(&record), &spec), DesignFreshness::Current);

        let mut edited = spec.clone();
        edited.description = "Read PM2.5 and PM10.".to_string();
        assert!(!record.ratifies(&edited));
        assert!(matches!(
            freshness(Some(&record), &edited),
            DesignFreshness::Stale { .. }
        ));
        assert_eq!(freshness(None, &spec), DesignFreshness::Absent);
    }

    #[test]
    fn the_record_attaches_without_disturbing_the_spec() {
        let spec = sps30();
        let mut g = decompose(&spec);
        let before = g.clone();
        attach_component_record(
            &mut g,
            &ComponentRecord::now(&spec, ComponentSource::Filled, 2),
        );
        assert_eq!(
            reconstruct(&g),
            Ok(spec),
            "the record is not part of the contract"
        );
        assert_eq!(g.nodes.len(), before.nodes.len() + 1);
        assert_eq!(g.edges.len(), before.edges.len() + 1);

        let record = reconstruct_component_record(&g)
            .expect("reads back")
            .expect("there is a record");
        assert_eq!(record.decision.source, ComponentSource::Filled);
        assert_eq!(record.decision.version, 2);
        assert_eq!(record.kind, ComponentKind::Driver);
    }

    #[test]
    fn every_door_roundtrips_through_its_record() {
        let spec = sps30();
        for source in [
            ComponentSource::Stated,
            ComponentSource::Stub,
            ComponentSource::Filled,
        ] {
            let mut g = decompose(&spec);
            attach_component_record(&mut g, &ComponentRecord::now(&spec, source, 1));
            let back = reconstruct_component_record(&g)
                .expect("reads back")
                .expect("present");
            assert_eq!(back.decision.source, source);
        }
    }

    #[test]
    fn a_broken_record_is_refused_rather_than_read_as_no_record() {
        let mut g = ComponentGraph::default();
        g.nodes.push(
            ComponentNode::new(node::DESIGN_RECORD, DESIGN_RECORD_NAME)
                .with(P_KIND, json!("driver")),
        );
        assert!(reconstruct_component_record(&g).is_err());
    }

    #[test]
    fn a_misspelled_role_is_refused() {
        let mut spec = sps30();
        spec.provides = vec!["sensing.particulates".to_string()];
        let err = spec.validate().expect_err("refused");
        assert!(err.contains("sensing.particulates"), "{err}");
        assert!(sps30().validate().is_ok());
    }

    #[test]
    fn only_a_driver_may_name_a_device() {
        let mut spec = sps30();
        spec.kind = ComponentKind::Library;
        let err = spec.validate().expect_err("refused");
        assert!(err.contains("library"), "{err}");
    }

    #[test]
    fn a_device_needs_its_part_and_bus() {
        let mut spec = sps30();
        spec.device.as_mut().unwrap().part = "  ".to_string();
        assert!(spec.validate().is_err());
        let mut spec = sps30();
        spec.device.as_mut().unwrap().bus = String::new();
        assert!(spec.validate().is_err());
    }

    /// **The record cannot be made to lie.** `device-facts` is keyed by part, so a fact naming
    /// another part's document is refused however it arrived — a backfill, a hand edit, or a
    /// retrieval that ranked the neighbour first.
    #[test]
    fn a_fact_must_be_this_parts_own_document() {
        let mut spec = sps30();
        spec.facts[0].source = "sht20.md".to_string();
        let err = spec.validate().expect_err("refused");
        assert!(err.contains("another part's document"), "{err}");
        assert!(
            err.contains("sht20.md"),
            "the refusal names the document: {err}"
        );

        // The near-miss pair is the case that matters: both documents exist, one is this part's.
        assert!(sps30().validate().is_ok());
    }

    #[test]
    fn a_device_fact_needs_a_device_and_a_document() {
        // A device-facts reference with no device has no key to be looked up by.
        let mut spec = sps30();
        spec.device = None;
        let err = spec.validate().expect_err("refused");
        assert!(err.contains("names no device"), "{err}");

        // A fact that names no document is not a record of anything.
        let mut spec = sps30();
        spec.facts[0].source = "  ".to_string();
        let err = spec.validate().expect_err("refused");
        assert!(err.contains("names no corpus or no document"), "{err}");

        // A library may stand on a corpus document that is not device-facts: it has no part to own.
        let mut spec = sps30();
        spec.kind = ComponentKind::Library;
        spec.device = None;
        spec.facts[0].corpus = "esp-idf-lib".to_string();
        assert!(spec.validate().is_ok(), "{:?}", spec.validate());
    }

    #[test]
    fn the_rendered_yaml_is_the_spec() {
        let spec = sps30();
        let yaml = render_yaml(&spec);
        let back: serde_json::Value = serde_yaml::from_str(&yaml).expect("parses");
        assert_eq!(back["name"], "sps30");
        assert_eq!(back["kind"], "driver");
        assert_eq!(back["device"]["part"], "sps30");
        assert_eq!(back["provides"][0], "sensing.particulate");
    }
}
