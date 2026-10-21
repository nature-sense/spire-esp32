// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **The versioned decision** — the part of a spec's provenance that every spec shares.
//!
//! A *spec* (an application's composition, a component's contract) is data; the **decision** is the
//! record of someone having accepted one. Whatever the spec is, the record has the same shape: which
//! door it came through ([`Source`]), how many times this spec has been decided ([`Decision::version`]),
//! when, and the [`fingerprint`] of the spec it ratifies. Only the summary *beside* that core differs —
//! an application records its framework and justification, a component its kind and description — so
//! those stay on the per-spec record ([`super::composition_graph::DesignRecord`] and
//! [`super::component_spec::ComponentRecord`]), and the core lives here.
//!
//! [`freshness`] is the question a reader has: **does the decision still ratify the spec in front of
//! it?** Equal fingerprints are [`DesignFreshness::Current`]; a spec edited since the decision is
//! [`DesignFreshness::Stale`]; no decision at all is [`DesignFreshness::Absent`], which is honest
//! rather than an error — it is the ordinary state of a spec nobody has decided yet.
//!
//! The fingerprint is taken over a value's **canonical JSON** — the bytes the rendered copy of a spec
//! holds — so two things that are the same spec fingerprint the same however they were serialized.
//! FNV-1a rather than a cryptographic hash: this answers "has it changed?" for two values one process
//! already holds, and it is a change detector rather than a defence against a forger.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

/// A door a spec's decision can come through.
///
/// Each spec owns the doors that make sense for it — an application is decided from its
/// `composition.spire` or a form, a component from what a person stated or a model filled — but a
/// reader of *any* record parses the door the same way, and a stored record has to be readable by a
/// build that did not write it. So the vocabulary is per spec and the *shape* is here.
pub trait Source: Copy + PartialEq + Eq + std::fmt::Debug {
    /// The stored name, and the name a reader sees.
    fn as_str(self) -> &'static str;

    /// The door a stored name denotes. `Err` is a corrupt record, not an absent one: a reader that
    /// silently mapped an unknown door to a default would claim a provenance nobody recorded.
    fn from_name(name: &str) -> Result<Self, String>;
}

/// The property keys a decision projects to on its record node.
///
/// Shared so a reader can find a decision's provenance on any spec's record node without knowing
/// which spec wrote it.
pub mod prop {
    pub const SOURCE_KIND: &str = "source_kind";
    pub const VERSION: &str = "design_version";
    pub const DECIDED_AT: &str = "decided_at";
    pub const FINGERPRINT: &str = "fingerprint";
}

/// **The versioned core of a decision**: which door, how many times, when, and what it ratifies.
///
/// What this deliberately does *not* carry is the spec's own summary. A decision should read as one
/// self-contained statement, so the summary stays on the record beside this core rather than here —
/// the alternative is a shared type that knows about frameworks and components, which is exactly the
/// coupling this module exists to undo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision<S> {
    /// The door the spec came through.
    pub source: S,
    /// Which decision this is for the spec (the leg that accepted it counts the specs it has decided).
    pub version: u32,
    /// When it was decided.
    pub decided_at: DateTime<Utc>,
    /// [`fingerprint`] of the spec **as decided**. The basis of staleness: the spec may be edited
    /// afterwards, and this record has to be able to say so.
    pub fingerprint: String,
}

impl<S> Decision<S> {
    /// The decision for `value`, decided now, through `source`, as decision `version`.
    pub fn now<T: Serialize + ?Sized>(value: &T, source: S, version: u32) -> Self {
        Self::at(value, source, version, Utc::now())
    }

    /// [`Self::now`] with the clock passed in, so a caller that already stamped the decision records
    /// *that* instant rather than a second one microseconds later.
    pub fn at<T: Serialize + ?Sized>(
        value: &T,
        source: S,
        version: u32,
        decided_at: DateTime<Utc>,
    ) -> Self {
        Self {
            source,
            version,
            decided_at,
            fingerprint: fingerprint(value),
        }
    }

    /// Whether this decision still ratifies `value` — [`freshness`], for the single-record case.
    pub fn ratifies<T: Serialize + ?Sized>(&self, value: &T) -> bool {
        self.fingerprint == fingerprint(value)
    }
}

impl<S: Source> Decision<S> {
    /// The decision's provenance as node properties, under the keys every spec's record node uses
    /// (see [`prop`]).
    pub fn props(&self) -> Vec<(String, Value)> {
        vec![
            (prop::SOURCE_KIND.to_string(), json!(self.source.as_str())),
            (prop::VERSION.to_string(), json!(self.version)),
            (
                prop::DECIDED_AT.to_string(),
                json!(self.decided_at.to_rfc3339()),
            ),
            (prop::FINGERPRINT.to_string(), json!(self.fingerprint)),
        ]
    }

    /// Read a decision back out of a node's properties.
    ///
    /// A node that is *there* but missing its door, timestamp or fingerprint is refused (`Err`),
    /// because reading a corrupt decision as no decision is the confident kind of wrong: a caller
    /// would report the spec as merely undecided and let it be re-decided over a decision nobody
    /// superseded.
    pub fn from_props(props: &[(String, Value)]) -> Result<Self, String> {
        let get = |key: &str| props.iter().find(|(k, _)| k == key).map(|(_, v)| v);
        let missing = |what: &str| format!("the design record is missing its '{what}' property");
        let source = get(prop::SOURCE_KIND)
            .and_then(Value::as_str)
            .ok_or_else(|| missing(prop::SOURCE_KIND))?;
        let decided_at = get(prop::DECIDED_AT)
            .and_then(Value::as_str)
            .ok_or_else(|| missing(prop::DECIDED_AT))?;
        let fingerprint = get(prop::FINGERPRINT)
            .and_then(Value::as_str)
            .ok_or_else(|| missing(prop::FINGERPRINT))?;
        Ok(Self {
            source: S::from_name(source)?,
            version: get(prop::VERSION)
                .and_then(Value::as_u64)
                .map(|v| v as u32)
                .unwrap_or(1),
            decided_at: DateTime::parse_from_rfc3339(decided_at)
                .map_err(|e| format!("the design record's 'decided_at' is not a timestamp: {e}"))?
                .with_timezone(&Utc),
            fingerprint: fingerprint.to_string(),
        })
    }
}

/// The canonical fingerprint of a spec: order-exact, and stable for one design.
///
/// Taken over the value's **canonical JSON** — the very bytes the rendered copy of the spec holds —
/// so the fingerprint a decision carries is the hash of a file a person can open.
pub fn fingerprint<T: Serialize + ?Sized>(value: &T) -> String {
    let canonical = serde_json::to_string(value).unwrap_or_default();
    format!("{:016x}", fnv1a64(&canonical))
}

fn fnv1a64(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Whether a decision still ratifies the spec beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignFreshness {
    /// No design was ever recorded for this spec — there is nothing to be stale against.
    Absent,
    /// The record ratifies the spec as it stands.
    Current,
    /// **The spec moved on.** `recorded` is the fingerprint the design was decided from and `current`
    /// the one in the spec now, so a reader sees that they differ without re-deriving either.
    Stale { recorded: String, current: String },
}

impl DesignFreshness {
    pub fn is_current(&self) -> bool {
        matches!(self, DesignFreshness::Current)
    }

    /// Whether a design was decided at all — `Current` *or* `Stale`. Only [`Self::Absent`] is "no":
    /// a stale record is a decision that no longer applies, which is not the same answer as none.
    pub fn is_recorded(&self) -> bool {
        !matches!(self, DesignFreshness::Absent)
    }
}

/// Does `record` still ratify the spec whose fingerprint is `current`?
///
/// The pure derivation behind both specs' freshness readers; each one computes `current` from its
/// own spec type and hands it here.
pub fn freshness<S>(record: Option<&Decision<S>>, current: &str) -> DesignFreshness {
    let Some(record) = record else {
        return DesignFreshness::Absent;
    };
    if record.fingerprint == current {
        DesignFreshness::Current
    } else {
        DesignFreshness::Stale {
            recorded: record.fingerprint.clone(),
            current: current.to_string(),
        }
    }
}

/// A [`DesignFreshness`] as the doors answer it: the state's name, and for `stale` the two
/// fingerprints — so a reader sees *that* they differ, without re-deriving either.
///
/// Lives here, beside the derivation, rather than in the caller that used to hold it: both the
/// application door (`project/verifyApplication`) and the component doors answer the same shape, and
/// two copies of this mapping would be two places for the wire to drift.
pub fn freshness_json(freshness: &DesignFreshness) -> Value {
    match freshness {
        DesignFreshness::Absent => json!({ "state": "absent" }),
        DesignFreshness::Current => json!({ "state": "current" }),
        DesignFreshness::Stale { recorded, current } => json!({
            "state": "stale",
            "recorded": recorded,
            "current": current,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Door {
        Stated,
        Inherited,
    }

    impl Source for Door {
        fn as_str(self) -> &'static str {
            match self {
                Door::Stated => "stated",
                Door::Inherited => "inherited",
            }
        }

        fn from_name(name: &str) -> Result<Self, String> {
            match name {
                "stated" => Ok(Door::Stated),
                "inherited" => Ok(Door::Inherited),
                other => Err(format!("'{other}' is not a door")),
            }
        }
    }

    #[derive(Serialize)]
    struct Spec {
        name: String,
        parts: Vec<String>,
    }

    fn spec() -> Spec {
        Spec {
            name: "meter".to_string(),
            parts: vec!["a".to_string(), "b".to_string()],
        }
    }

    #[test]
    fn a_fingerprint_is_stable_for_one_value_and_moves_with_it() {
        let a = spec();
        let b = spec();
        assert_eq!(fingerprint(&a), fingerprint(&b), "same value, same hash");

        let mut edited = spec();
        edited.parts.push("c".to_string());
        assert_ne!(fingerprint(&a), fingerprint(&edited));
    }

    #[test]
    fn a_decision_ratifies_the_value_it_was_decided_from() {
        let record = Decision::now(&spec(), Door::Stated, 3);
        assert!(record.ratifies(&spec()));
        assert_eq!(
            freshness(Some(&record), &fingerprint(&spec())),
            DesignFreshness::Current
        );
        assert_eq!(record.version, 3);
        assert_eq!(record.source, Door::Stated);
    }

    #[test]
    fn a_value_edited_after_the_decision_reads_stale() {
        let recorded = Decision::now(&spec(), Door::Stated, 1);
        let mut edited = spec();
        edited.parts.push("c".to_string());
        let current = fingerprint(&edited);
        match freshness(Some(&recorded), &current) {
            DesignFreshness::Stale {
                recorded: was,
                current: now,
            } => {
                assert_eq!(was, recorded.fingerprint);
                assert_eq!(now, current);
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn no_decision_is_absent_and_is_not_stale() {
        let fresh = freshness::<Door>(None, &fingerprint(&spec()));
        assert_eq!(fresh, DesignFreshness::Absent);
        assert!(!fresh.is_current());
        assert!(!fresh.is_recorded());
    }

    #[test]
    fn props_round_trip_through_from_props() {
        let record = Decision::at(&spec(), Door::Inherited, 2, Utc::now());
        let props = record.props();
        let read = Decision::<Door>::from_props(&props).expect("props round-trip");
        assert_eq!(read, record);
    }

    #[test]
    fn a_record_missing_its_door_is_refused_not_read_as_absent() {
        let props: Vec<(String, Value)> = vec![
            (prop::DECIDED_AT.to_string(), json!(Utc::now().to_rfc3339())),
            (prop::FINGERPRINT.to_string(), json!("00")),
        ];
        assert!(Decision::<Door>::from_props(&props).is_err());
    }

    #[test]
    fn an_unknown_door_is_an_error() {
        let props: Vec<(String, Value)> = vec![
            (prop::SOURCE_KIND.to_string(), json!("guessed")),
            (prop::DECIDED_AT.to_string(), json!(Utc::now().to_rfc3339())),
            (prop::FINGERPRINT.to_string(), json!("00")),
        ];
        assert!(Decision::<Door>::from_props(&props).is_err());
    }
}
