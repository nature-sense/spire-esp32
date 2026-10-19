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
//! the one **Spire** corpus, `spire-idf`, describes how the component library an ESP-IDF
//! application is built on works, while the **embedded** ones are what a generated C++ firmware
//! component is written against: the ESP-IDF SDK and its documentation, and
//! the **library** corpora a hardware task starts from: `esp-idf-lib` (ninety-odd device drivers),
//! `esp-bsp` (board support packages, M5Stack's among them), `m5unified` (M5Stack's own board HAL),
//! `waveshare` (Waveshare's BSPs and display drivers), `lvgl` (the graphics library, vendored into
//! the `idf/` library and pinned there — its handbook and its API), and `esp-dl` (the on-device
//! inference runtime, its prebuilt models and the examples that drive them — the one corpus an
//! application that runs a *model* cannot do without).
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
//! `include_str!` rather than a directory scan: the point is to ship exactly these manifests (and
//! the `device-facts` documents), and a file that sits in the directory without an entry here is not
//! shipped at all. The `spire-idf` corpus is the exception that proves the rule: it ships no
//! documents, only a manifest whose `local` source is the embedded `idf/` project, so its contents
//! are read from the live tree rather than listed here.

use std::path::Path;

/// Each bundled manifest as `(corpus, the file's text)`.
///
/// The corpus is also the directory name the installer uses — `RagManifestInfo::domain` is resolved
/// from the manifest itself, and keeping the two equal is what makes
/// `~/.spire/knowledge/<corpus>/ingest.yaml` discoverable by the scan.
pub const BUNDLE: &[(&str, &str)] = &[
    (
        "spire-idf",
        include_str!("../../resources/rag-ingest/spire-idf.ingest.yaml"),
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
/// The **opening sentences** matter as much as the name. The embedder's context is shorter than a
/// document, so the vector a query is answered by comes from the head of the chunk while the whole
/// chunk is what the answer carries: a part named only in the title and the `Part number` line can
/// rank **fourth**, behind a document about another device entirely. `tests/rag_fill_tests.rs`
/// measures it (`st7789`: 0.054 before its opening paragraph named it, 0.097 after), so a new
/// document states its part number in the first two sentences.
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
///
/// The last four are the protocol of drivers this tree already contains, transcribed from their own
/// `.cpp` — `axp2101`, `bm8563`, `bmi270` and `ili9342` — so a regeneration can consult a part's
/// registers when the header states only the shape. `st7789` is the fifth and the odd one out, and
/// says so in its own text: its driver is still a stub, so there is nothing of its own to transcribe,
/// and the document is derived from the **vendored LVGL MIPI driver** (`lv_st7789.c`, whose init table
/// is LovyanGFX's, and `lv_lcd_generic_mipi.c`) plus this component's header. That is the honest
/// source available until the driver is written — a `st7789` regeneration copies the tuning from it,
/// and the panel's own datasheet remains the authority for the electricals.
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
    (
        "axp2101.md",
        include_str!("../../resources/device-facts/axp2101.md"),
    ),
    (
        "bm8563.md",
        include_str!("../../resources/device-facts/bm8563.md"),
    ),
    (
        "bmi270.md",
        include_str!("../../resources/device-facts/bmi270.md"),
    ),
    (
        "ili9342.md",
        include_str!("../../resources/device-facts/ili9342.md"),
    ),
    (
        "st7789.md",
        include_str!("../../resources/device-facts/st7789.md"),
    ),
];

/// The corpus generated from the embedded component library: the vendored `idf/` project's own
/// words, read from the live tree rather than shipped as copies.
///
/// The directory name the installer uses — it is also the corpus id — and the manifest it writes
/// carries [`SPIRE_LIBRARY_ROOT_PLACEHOLDER`], which [`install_into`] replaces with
/// [`crate::library::library_root`]. A document added to the library therefore reaches the corpus on
/// the next ingest without being listed anywhere: the manifest selects by convention
/// (`**/SPIRE.md`, `**/README.md`, `**/API.md`) from the tree itself.
pub const IDF_CORPUS: &str = "spire-idf";

/// The token the `spire-idf` manifest uses for the library root, substituted by [`install_into`].
///
/// It is a placeholder rather than an absolute path because the manifest ships with the binary and
/// the library's location is a property of the machine, not of the manifest: writing the path at
/// install time is what keeps the manifest valid in the source tree (and in the tests that parse it)
/// while the installed copy points at the tree on disk.
pub const SPIRE_LIBRARY_ROOT_PLACEHOLDER: &str = "__SPIRE_LIBRARY_ROOT__";

/// Write the bundle into `dir`, as `<corpus>/ingest.yaml`, and return what was written.
///
/// The KnowledgeStore's scan reads that layout, so this is the step that makes a corpus *offered*
/// (RagView lists the manifest) rather than ingested — the ingest itself is a separate, expensive
/// call. Shared by `rag/install-bundle-manifests` and the live fill test so the two cannot disagree
/// about where a manifest goes.
///
/// [`FACTS_DOCS`] are written too, as `<store>/<corpus>/docs/…`, because that corpus has no
/// repository to fetch them from: its manifest points at `docs` **relative to its own directory**,
/// so installing the manifest is what puts the documents where the ingest will look for them. A
/// store filled by an older binary therefore gains the corpus by re-installing, and its ingest is
/// the same deliberate step every other corpus needs.
///
/// The `spire-idf` manifest is the one that is **not** copied from: its `local` source names the
/// library on disk ([`crate::library::library_root`]), substituted for
/// [`SPIRE_LIBRARY_ROOT_PLACEHOLDER`] as it is written. The corpus is generated from the embedded
/// `idf/` project, so a document added there reaches the next ingest with no change here. The
/// document copy an older installer wrote to `<store>/spire-idf/docs/` is removed, because the
/// manifest no longer references it and a stale second copy of the library is exactly the drift
/// this replaces.
pub fn install_into(dir: &Path) -> Result<Vec<String>, String> {
    let library_root = crate::library::library_root();
    let mut installed = Vec::new();
    for (name, content) in BUNDLE {
        let target = dir.join(name).join("ingest.yaml");
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        }
        // The `spire-idf` manifest names the library by a placeholder: the tree is on disk, and
        // resolving it here is what makes the corpus follow the library's own files instead of
        // shipping a copy that can drift. Every other manifest is written verbatim.
        let content = if *name == IDF_CORPUS {
            content.replace(
                SPIRE_LIBRARY_ROOT_PLACEHOLDER,
                &library_root.to_string_lossy(),
            )
        } else {
            (*content).to_string()
        };
        std::fs::write(&target, content).map_err(|e| format!("write {name}: {e}"))?;
        if *name == IDF_CORPUS {
            // Only after the new manifest is on disk: a store is never left with neither source.
            remove_legacy_document_copy(&dir.join(IDF_CORPUS).join("docs"))?;
        }
        installed.push((*name).to_string());
    }
    for (name, content) in FACTS_DOCS {
        let target = dir.join(DEVICE_FACTS_CORPUS).join("docs").join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
        }
        std::fs::write(&target, content).map_err(|e| format!("write {name}: {e}"))?;
    }
    Ok(installed)
}

/// Remove the copy of the library's documents an older installer wrote beside the manifest.
///
/// `<store>/spire-idf/docs/` was the corpus's source when the manifest was relative (`path: docs`);
/// it is unreferenced now, and leaving it would leave a stale second copy of documents the live tree
/// owns. A missing directory (a store that never had a copy) is not an error.
fn remove_legacy_document_copy(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "remove the superseded document copy at {}: {e}",
            path.display()
        )),
    }
}
