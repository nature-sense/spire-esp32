// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! Dev harness: install the **bundled RAG manifests** into the user's KnowledgeStore.
//!
//! Usage:
//!   cargo run -p spire-esp32 --example install_rag_bundles
//!
//! **Why this exists.** Installing the bundle is a JSON-RPC call
//! (`rag/install-bundle-manifests`, served by the coordinator), which makes it awkward to run on a
//! machine whose app is not up — and a store filled by an *older* binary is exactly the situation it
//! is for: a corpus added since then is written into the app's knowledge store by the next install
//! and by nothing else. This drives the **same** `install_into` the handler calls, so what it writes
//! is the installer's own bytes rather than a hand-copied manifest.
//!
//! **Which store** is the part that bites, so it is stated rather than implied. The knowledge store
//! is scoped by the *application's* name (`~/.spire/<app>/knowledge`), and the library cannot know
//! it: a binary that names nothing resolves to the neutral default `spire`. A harness that skipped
//! the naming step would therefore install the whole bundle into `~/.spire/spire/knowledge` — a path
//! that looks entirely plausible, that the app never opens, and where the manifests would sit being.
//! So this names the app exactly as the FFI entry point does (`ffi.rs`: `set_app_name(env!
//! ("CARGO_PKG_NAME"))`, i.e. `spire-esp32` — the same reason a *test* binary is run with
//! `SPIRE_APP_NAME=spire-esp32`) and prints the scope it resolved, so a wrong one is loud rather than
//! silent. `SPIRE_KNOWLEDGE_DIR` overrides the whole path outright if a caller wants somewhere else.
//!
//! What it does and does not do:
//!
//!   * it **writes** each manifest as `<corpus>/ingest.yaml` (and `device-facts`' accompanying
//!     documents beside their manifest), which is what makes a corpus *offered* — RagView lists it
//!     and its Ingest button becomes meaningful. It is idempotent and safe to re-run; an existing
//!     manifest is replaced by the shipped one, which is the point.
//!   * it does **not** ingest. Embedding a corpus is expensive and deliberate, and it is the one
//!     thing this must not do behind a person's back — so the harness says so and stops.
//!
//! The corpus list it installs is `actors::rag_bundle::BUNDLE`, the same one the server, the
//! manifest tests and the live fill read.

use spire_core::config::knowledge_dir;

fn main() {
    // Before anything resolves a config path — the store is scoped by the application's name, and
    // the neutral default is not this application's scope (see the module docs).
    spire_core::config::set_app_name(env!("CARGO_PKG_NAME"));

    let dir = knowledge_dir();
    let installed = match spire_esp32::actors::rag_bundle::install_into(&dir) {
        Ok(names) => names,
        Err(e) => {
            eprintln!("install failed: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "app scope '{}': {} corpora installed into {}",
        spire_core::config::app_name(),
        installed.len(),
        dir.display()
    );
    for name in &installed {
        println!("  {name}");
    }
    println!(
        "\n---\nthe manifests are written; the corpora are **not** ingested.\n\
         Ingest each one from RagView (or `rag/ingest-manifest`) — ingesting is where the\n\
         embedding cost is paid, and it is a person's decision, not a side effect of installing."
    );
}
