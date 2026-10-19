// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! Where the **component library** lives — the vendored `idf/` project at the repository root.
//!
//! Two callers need the same answer and must not derive it separately:
//!
//! * the host's icon bar opens the library as a project (`project/open`), so its components become
//!   subprojects and its build/test/lint flows are reachable from the app;
//! * `rag_bundle::install_into` writes the `spire-idf` manifest with this path as its `local` source,
//!   so the corpus is generated from the same tree rather than from a copy that could drift.
//!
//! The library is **not** inside the app bundle: it is a sibling of `crates/`, checked into this
//! repository. `CARGO_MANIFEST_DIR` is therefore the stable anchor — it is the crate's source
//! directory at build time, which is exactly what `idf/` sits relative to. The coordinator already
//! locates the bundled `config/mcp-config.json` the same way; the alternative (a hard-coded
//! `~/naturesense/...`) only works on the machine it was written for.
//!
//! [`LIBRARY_ROOT_ENV`] overrides the path for a checkout that was moved after the binary was built,
//! or for a test that wants to point at a fixture. An empty value is ignored rather than meaning
//! "the current directory".

use std::path::{Path, PathBuf};

/// Environment variable that overrides [`library_root`], e.g. `SPIRE_LIBRARY_ROOT=/tmp/idf`.
pub const LIBRARY_ROOT_ENV: &str = "SPIRE_LIBRARY_ROOT";

/// The root of the vendored ESP-IDF component library (`idf/`), as an absolute path.
///
/// Resolved as, in order:
///
/// 1. `$SPIRE_LIBRARY_ROOT`, when set and non-empty;
/// 2. `<crate source>/../../idf` — `crates/spire-esp32` → the repository root → `idf/` — made
///    canonical when it exists, so the path a manifest records has no `..` in it.
///
/// The path is returned even when it does not exist: "the library is missing" is a caller's
/// question (the icon says so, the ingest fails on the empty directory), not a reason to panic at
/// a path that is conceptually well-defined.
pub fn library_root() -> PathBuf {
    if let Some(path) = std::env::var_os(LIBRARY_ROOT_ENV) {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            // Canonicalised when it exists, so a relative override cannot leave the
            // installed manifest naming a path that resolves against the manifest's
            // own directory instead of the caller's.
            return path.canonicalize().unwrap_or(path);
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../idf");
    root.canonicalize().unwrap_or(root)
}
