// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **A spec is a provenance record, and a record that cannot be followed is a lie.**
//!
//! Every shipped `components/*/SPIRE.component.yaml` states what the component is, what it must keep
//! true, and — for a driver — the part its protocol comes from and the documents it stood on. The
//! graph is what the app reads; the file is what a person opens and what the tree carries, so it is
//! checked here against the tree itself: the directory it ships in, the kind its own
//! `CMakeLists.txt` declares, and the `device-facts` corpus.
//!
//! The device-facts rule is the one with a history. An embedding of a bare part number carries almost
//! no signal, so retrieval answered `axp2101` with `bmi270.md` and `ili9342` with `gt911.md`, and those
//! neighbours were recorded into the specs before the seam promoted the part's own document. A spec
//! that keeps such a fact is a component whose protocol would be another device's ever after — which
//! is why the rule lives in [`document_is_the_parts_own`] at retrieval time and is checked again here
//! at the record, where it can no longer change.

mod common;

use std::path::{Path, PathBuf};

use serde_yaml::Value;
use spire_esp32::actors::rag_bundle;
use spire_esp32::build::idf_projects::{self, document_is_the_parts_own, ComponentKind};

const SPEC_FILE: &str = "SPIRE.component.yaml";

/// Every way the spec at `spec_path` disagrees with the tree under `root`, as sentences a person can
/// act on. Empty means the record can be followed.
///
/// Split out from the walk so the refusals can be **proven** with planted specs, the way the mutation
/// tests prove the component gate: a check that only ever sees good input is indistinguishable from no
/// check at all.
fn problems_with(root: &Path, spec_path: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(directory) = spec_path.parent() else {
        return vec!["has no directory".to_string()];
    };
    let ident = directory
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let text = match std::fs::read_to_string(spec_path) {
        Ok(text) => text,
        Err(e) => return vec![format!("cannot be read: {e}")],
    };
    let doc: Value = match serde_yaml::from_str(&text) {
        Ok(doc) => doc,
        Err(e) => return vec![format!("is not YAML: {e}")],
    };

    // The name is how every other leg finds the component, and the directory is where the door writes.
    let name = doc
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if name.is_empty() {
        problems.push("states no name".to_string());
    } else if name != ident {
        problems.push(format!("names '{name}', but ships in components/{ident}"));
    }

    // The manifest is authoritative — it decided the skeleton — and a component whose manifest states
    // nothing is one no door can read back (`implied_component_spec` refuses it by name).
    let stated_kind = doc.get("kind").and_then(Value::as_str);
    match (stated_kind, idf_projects::component_kind(root, &ident)) {
        (Some(stated), Some(actual)) if stated == actual.as_str() => {}
        (Some(stated), Some(actual)) => problems.push(format!(
            "says it is a {stated}, but components/{ident}/CMakeLists.txt states `{}`",
            actual.as_str()
        )),
        (None, Some(_)) => problems.push("states no kind".to_string()),
        (_, None) => problems.push(format!(
            "components/{ident}/CMakeLists.txt states no kind, so no edit door can read this component"
        )),
    }

    // A device belongs to a driver, and the part is the authoritative `device-facts` key.
    let device = doc.get("device");
    let part = device
        .and_then(|d| d.get("part"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let bus = device
        .and_then(|d| d.get("bus"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    match (stated_kind, device) {
        (Some("driver"), Some(_)) => {
            if part.is_empty() {
                problems.push("is a driver but names no part".to_string());
            }
            if bus.is_empty() {
                problems.push("is a driver but names no bus".to_string());
            }
        }
        (Some("driver"), None) => problems.push("is a driver but names no device".to_string()),
        (_, Some(_)) => problems.push(format!(
            "names a device, but a {} has none",
            stated_kind.unwrap_or("component")
        )),
        (_, None) => {}
    }

    // Each fact is a document something can go and get again. For `device-facts` the corpus is keyed
    // by part — one document per part, named for it — so a fact there is either *this* part's
    // document or it is another device's protocol recorded as this one's.
    let facts = doc
        .get("facts")
        .and_then(Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    let mut records_own_document = false;
    for (index, fact) in facts.iter().enumerate() {
        let corpus = fact
            .get("corpus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let source = fact
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if corpus.is_empty() || source.is_empty() {
            problems.push(format!("fact {} names no corpus or no document", index + 1));
            continue;
        }
        if corpus != idf_projects::DEVICE_FACTS_DOMAIN {
            continue;
        }
        if part.is_empty() {
            problems.push(format!(
                "stands on '{source}' from device-facts with no device to look it up by"
            ));
            continue;
        }
        if !document_is_the_parts_own(part, source) {
            problems.push(format!(
                "records '{source}' as a fact about '{part}', but that is another part's document"
            ));
            continue;
        }
        records_own_document = true;
        if !rag_bundle::FACTS_DOCS
            .iter()
            .any(|(name, _)| *name == source)
        {
            problems.push(format!(
                "records '{source}', which is not in FACTS_DOCS — the corpus ships no such document"
            ));
        }
    }
    if stated_kind == Some("driver") && !part.is_empty() && !records_own_document {
        problems.push(format!(
            "is a driver for '{part}' but records no device-facts document of its own"
        ));
    }

    problems
}

/// The specs the tree ships, in a stable order (a report that depends on readdir order is not a
/// report).
fn shipped_specs(root: &Path) -> Vec<PathBuf> {
    let mut specs: Vec<PathBuf> = std::fs::read_dir(root.join("components"))
        .expect("the library has a components/ directory")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let spec = path.join(SPEC_FILE);
            (path.is_dir() && spec.is_file()).then_some(spec)
        })
        .collect();
    specs.sort();
    specs
}

/// **Every shipped spec can be followed**: it names its own directory, agrees with its manifest, and
/// every document it records is one this part owns and this build ships.
#[test]
fn every_shipped_component_spec_resolves_to_its_own_part() {
    let root = common::idf_root();
    let specs = shipped_specs(&root);
    assert!(
        specs.len() >= 10,
        "the walk found {} specs under {} — it is not finding the library",
        specs.len(),
        root.join("components").display()
    );

    let mut problems = Vec::new();
    for spec in &specs {
        for problem in problems_with(&root, spec) {
            let shown = spec.strip_prefix(&root).unwrap_or(spec);
            problems.push(format!("{}: {problem}", shown.display()));
        }
    }
    assert!(
        problems.is_empty(),
        "the shipped component specs disagree with the tree:\n  {}",
        problems.join("\n  ")
    );
}

/// **The check refuses the records that matter** — proven on planted specs, because a checker that has
/// only ever seen good input cannot be told apart from one that checks nothing.
#[test]
fn the_spec_check_refuses_a_record_that_cannot_be_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for (ident, kind) in [
        ("sps30", ComponentKind::Driver),
        ("acme1", ComponentKind::Driver),
        ("rolling_average", ComponentKind::Library),
    ] {
        let dir = root.join("components").join(ident);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("CMakeLists.txt"),
            format!("{}\n", idf_projects::component_kind_line(kind)),
        )
        .unwrap();
    }

    let check = |ident: &str, body: &str| -> Vec<String> {
        let path = root.join("components").join(ident).join(SPEC_FILE);
        std::fs::write(&path, body).unwrap();
        problems_with(root, &path)
    };

    // The control: a record that can be followed is accepted, so the refusals below are not merely a
    // checker that complains about everything.
    let accepted = check(
        "sps30",
        "\
name: sps30
kind: driver
device:
  part: sps30
  bus: i2c
facts:
- corpus: device-facts
  source: sps30.md
",
    );
    assert!(accepted.is_empty(), "{accepted:?}");

    // The historical bug: this part's spec standing on the neighbour's document.
    let neighbour = check(
        "sps30",
        "\
name: sps30
kind: driver
device:
  part: sps30
  bus: i2c
facts:
- corpus: device-facts
  source: sht20.md
",
    );
    assert!(
        neighbour
            .iter()
            .any(|p| p.contains("another part's document")),
        "{neighbour:?}"
    );

    // A part with no shipped document: the record names a file nothing can produce.
    let unshipped = check(
        "acme1",
        "\
name: acme1
kind: driver
device:
  part: acme1
  bus: i2c
facts:
- corpus: device-facts
  source: acme1.md
",
    );
    assert!(
        unshipped
            .iter()
            .any(|p| p.contains("the corpus ships no such document")),
        "{unshipped:?}"
    );

    // A driver that records nothing: the audit would have no bytes to stand on.
    let undocumented = check(
        "sps30",
        "\
name: sps30
kind: driver
device:
  part: sps30
  bus: i2c
",
    );
    assert!(
        undocumented
            .iter()
            .any(|p| p.contains("records no device-facts document of its own")),
        "{undocumented:?}"
    );

    // A library that names a device is a device it never touches.
    let library_with_device = check(
        "rolling_average",
        "\
name: rolling_average
kind: library
device:
  part: sps30
  bus: i2c
",
    );
    assert!(
        library_with_device.iter().any(|p| p.contains("has none")),
        "{library_with_device:?}"
    );

    // A spec whose kind disagrees with the manifest describes a different component.
    let wrong_kind = check(
        "rolling_average",
        "\
name: rolling_average
kind: driver
",
    );
    assert!(
        wrong_kind
            .iter()
            .any(|p| p.contains("CMakeLists.txt states")),
        "{wrong_kind:?}"
    );

    // A fact that names nothing is not a record of anything.
    let empty_fact = check(
        "sps30",
        "\
name: sps30
kind: driver
device:
  part: sps30
  bus: i2c
facts:
- corpus: ''
  source: sps30.md
",
    );
    assert!(
        empty_fact
            .iter()
            .any(|p| p.contains("names no corpus or no document")),
        "{empty_fact:?}"
    );
}
