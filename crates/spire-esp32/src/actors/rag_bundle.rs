// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! The **bundled RAG manifests** — the corpora this binary can install.
//!
//! One list, in one place, because three things read it and none of them may drift:
//!
//! * `rag/install-bundle-manifests` writes each manifest into the KnowledgeStore's scan dirs
//!   (`~/.spire/knowledge/<corpus>/ingest.yaml`), which is what makes RagView offer it;
//! * `tests/rag_bundle_manifests.rs` parses each one, so a typo in a YAML fails the suite rather
//!   than silently ingesting an empty corpus;
//! * the live fill test ingests them, so "the bundle" means the same set everywhere.
//!
//! Two families, and the split is worth keeping in mind when reading a corpus's retrieval quality:
//! the **Spire** corpora describe how this application works — and, for `spire-idf`, how the
//! component library an ESP-IDF application is built on works — while the **embedded** ones are what
//! a generated firmware crate is written against: the esp-rs book, `esp-idf-hal`, ESP-IDF and its
//! documentation, the Rust std the board toolchain compiles, and the **library** corpora a hardware
//! task starts from: `esp-idf-lib` (ninety-odd device drivers), `esp-bsp` (board support packages,
//! M5Stack's among them), `m5unified` (M5Stack's own board HAL), `waveshare` (Waveshare's BSPs and
//! display drivers), `lvgl` (the graphics library, vendored into `spire-idf` and pinned there — its
//! handbook and its API), and `esp-dl` (the on-device inference runtime, its prebuilt models and the
//! examples that drive them — the one corpus an application that runs a *model* cannot do without).
//!
//! `device-facts` is a third thing, and the only corpus here that is neither this application nor
//! somebody else's code: one document per **key** — a part number, or a board name for the wiring
//! its parts sit in — stating that part's protocol (command words, framing, checksum, malformed
//! replies) or that board's pins, with its provenance. The library corpora answer
//! "has somebody written this device, and how is it driven"; they cannot answer "what are the bytes
//! for *this* part", because a device the library does not carry is not in it (there is no `sht20`
//! in `esp-idf-lib`, only `sht3x` — a different device with different commands) and because the
//! bytes live in the `components/*/*.c` files those manifests exclude. So the two are asked
//! **together**, and the seam labels which section is which: the facts for the bytes, `esp-idf-lib`
//! for the shape.
//!
//! `include_str!` rather than a directory scan: the point is to ship exactly these files, and a file
//! that sits in the directory without an entry here is not shipped at all.

use std::path::Path;

/// Each bundled manifest as `(corpus, the file's text)`.
///
/// The corpus is also the directory name the installer uses — `RagManifestInfo::domain` is resolved
/// from the manifest itself, and keeping the two equal is what makes
/// `~/.spire/knowledge/<corpus>/ingest.yaml` discoverable by the scan.
pub const BUNDLE: &[(&str, &str)] = &[
    (
        "spire-core",
        include_str!("../../resources/rag-ingest/spire-core.ingest.yaml"),
    ),
    (
        "spire-actor",
        include_str!("../../resources/rag-ingest/spire-actor.ingest.yaml"),
    ),
    (
        "spire-idf",
        include_str!("../../resources/rag-ingest/spire-idf.ingest.yaml"),
    ),
    (
        "esp-rs-book",
        include_str!("../../resources/rag-ingest/esp-rs-book.ingest.yaml"),
    ),
    (
        "esp-idf-hal",
        include_str!("../../resources/rag-ingest/esp-idf-hal.ingest.yaml"),
    ),
    (
        "esp-idf",
        include_str!("../../resources/rag-ingest/esp-idf.ingest.yaml"),
    ),
    (
        "esp-idf-lib",
        include_str!("../../resources/rag-ingest/esp-idf-lib.ingest.yaml"),
    ),
    (
        "esp-bsp",
        include_str!("../../resources/rag-ingest/esp-bsp.ingest.yaml"),
    ),
    (
        "m5unified",
        include_str!("../../resources/rag-ingest/m5unified.ingest.yaml"),
    ),
    (
        "esp-dl",
        include_str!("../../resources/rag-ingest/esp-dl.ingest.yaml"),
    ),
    (
        "waveshare",
        include_str!("../../resources/rag-ingest/waveshare.ingest.yaml"),
    ),
    (
        "lvgl",
        include_str!("../../resources/rag-ingest/lvgl.ingest.yaml"),
    ),
    (
        "rust",
        include_str!("../../resources/rag-ingest/rust.ingest.yaml"),
    ),
    (
        DEVICE_FACTS_CORPUS,
        include_str!("../../resources/rag-ingest/device-facts.ingest.yaml"),
    ),
];

/// The corpus whose documents ship beside its manifest, instead of being fetched at ingest time.
///
/// The directory name the installer uses, so the manifest's relative `path: docs` resolves against
/// the manifest's own directory — see [`install_into`].
pub const DEVICE_FACTS_CORPUS: &str = "device-facts";

/// The documents the `device-facts` corpus ships, as `(file name, the text)`, installed beside its
/// manifest as `<store>/device-facts/docs/<file name>`.
///
/// An explicit list, and `include_str!`, for the reason [`BUNDLE`] gives: a document added to
/// `resources/device-facts/` without an entry here is **not shipped**, so what the corpus can answer
/// is reviewable in the source tree rather than assembled at install time. The manifest globs the
/// installed directory (`**/*.md`) rather than naming these files, which is only safe because
/// *this* list is what decides what reaches that directory — the gate and the glob cannot disagree
/// about which documents are part of the corpus.
///
/// The file name is the **key the corpus is asked with** — a part number (`sht20`) for a device, or
/// the board name (`cores3`) for a board's wiring — because that is what makes the lookup keyed
/// rather than a similarity search: a component's edit looks its device up as `sht20`, and a
/// document named `sht20.md` is what answers it.
///
/// More than one document ships, and the pairs that share a family are deliberately **near misses** of
/// each other: `sht20.md` and `sht30.md` are one family measuring without clock stretching with the same
/// CRC polynomial, and they disagree about the address, the command words, the framing and the
/// checksum's seed. A corpus of unrelated documents would be answered correctly by accident and
/// prove nothing; a near-miss pair is what the key has to separate, and
/// `tests/rag_fill_tests.rs` pins that a query for one part answers with that part's document rather
/// than its neighbour's — by a small margin, which is exactly why the seam labels every chunk with
/// its source path rather than trusting the ranking alone.
///
/// The touch pair is a second near miss of a different kind: `ft6336.md` and `gt911.md` are two
/// capacitive controllers doing the same job with different framing (one-byte vs. two-byte register
/// addresses, high-nibble-first twelve-bit vs. little-endian 16-bit coordinates), so a driver written
/// for one is wrong for the other. `cores3.md` is not a near miss at all: it answers a **board**
/// question — the pins, the expander lines and the backlight rail — rather than a part's protocol.
///
/// `sps30.md` ships for a **different question**, so it is no near-miss of the Sensirion humidity pair:
/// the design step has no part number yet and queries the corpus with a *description* ("a PM2.5 meter"),
/// so this document is what a semantic match has to reach to supply an external part's address instead
/// of the model asking a person for it.
pub const FACTS_DOCS: &[(&str, &str)] = &[
    (
        "sht20.md",
        include_str!("../../resources/device-facts/sht20.md"),
    ),
    (
        "sht30.md",
        include_str!("../../resources/device-facts/sht30.md"),
    ),
    (
        "sps30.md",
        include_str!("../../resources/device-facts/sps30.md"),
    ),
    (
        "ft6336.md",
        include_str!("../../resources/device-facts/ft6336.md"),
    ),
    (
        "gt911.md",
        include_str!("../../resources/device-facts/gt911.md"),
    ),
    (
        "cores3.md",
        include_str!("../../resources/device-facts/cores3.md"),
    ),
];

/// The corpus whose documents ship beside its manifest: the vendored component library's own words.
///
/// The directory name the installer uses, so the manifest's relative `path: docs` resolves against
/// the manifest's own directory — see [`install_into`].
pub const IDF_CORPUS: &str = "spire-idf";

/// The documents the `spire-idf` corpus ships, as `(path under docs/, the text)`, installed beside
/// its manifest as `<store>/spire-idf/docs/<path>`.
///
/// The library itself lives at `<repo>/idf/`; these are the files its manifest globs, listed
/// explicitly for the same reason [`FACTS_DOCS`] is: what the corpus can answer is reviewable in
/// the source tree, and a file added to the library without an entry here is not shipped.
///
/// `API.md` travels with them because it is the function reference — the document that answers
/// "what does this component expose" when retrieval reaches the library outside a checkout.
pub const IDF_DOCS: &[(&str, &str)] = &[
    ("SPIRE.md", include_str!("../../../../idf/SPIRE.md")),
    ("README.md", include_str!("../../../../idf/README.md")),
    ("API.md", include_str!("../../../../idf/API.md")),
    (
        "components/lvgl/README.md",
        include_str!("../../../../idf/components/lvgl/README.md"),
    ),
];

/// Write the bundle into `dir`, as `<corpus>/ingest.yaml`, and return what was written.
///
/// The KnowledgeStore's scan reads that layout, so this is the step that makes a corpus *offered*
/// (RagView lists the manifest) rather than ingested — the ingest itself is a separate, expensive
/// call. Shared by `rag/install-bundle-manifests` and the live fill test so the two cannot disagree
/// about where a manifest goes.
///
/// [`FACTS_DOCS`] and [`IDF_DOCS`] are written too, as `<store>/<corpus>/docs/…`, because those
/// corpora have no repository to fetch them from: their manifests point at `docs` **relative to
/// their own directory**, so installing the manifest is what puts the documents where the ingest
/// will look for them. A store filled by an older binary therefore gains the corpora by
/// re-installing, and its ingest is the same deliberate step every other corpus needs.
pub fn install_into(dir: &Path) -> Result<Vec<String>, String> {
    let mut installed = Vec::new();
    for (name, content) in BUNDLE {
        let target = dir.join(name).join("ingest.yaml");
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        }
        std::fs::write(&target, content).map_err(|e| format!("write {name}: {e}"))?;
        installed.push((*name).to_string());
    }
    for (name, content) in FACTS_DOCS {
        let target = dir.join(DEVICE_FACTS_CORPUS).join("docs").join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        }
        std::fs::write(&target, content).map_err(|e| format!("write {name}: {e}"))?;
    }
    for (name, content) in IDF_DOCS {
        let target = dir.join(IDF_CORPUS).join("docs").join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        }
        std::fs::write(&target, content).map_err(|e| format!("write {name}: {e}"))?;
    }
    Ok(installed)
}
