// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! The **live fill**: install the bundled corpora and ingest them into the KnowledgeStore.
//!
//! This is the step that turns a manifest into retrievable knowledge, and it is the one that has to
//! touch the network — nearly every corpus clones from GitHub (`spire-idf` and `device-facts` are
//! the local sources, and the device/board libraries are the largest clones) — and a real embedder, so
//! it is `#[ignore]`d and run deliberately:
//!
//! ```sh
//! # every corpus, into ~/.spire/knowledge
//! cargo test -p spire-esp32 --test rag_fill_tests -- --ignored --nocapture
//!
//! # just one, when refreshing it
//! SPIRE_FILL_CORPUS=esp-idf-lib cargo test -p spire-esp32 --test rag_fill_tests -- --ignored --nocapture
//! ```
//!
//! It writes to the **user-level** store (`~/.spire/knowledge`, or `$SPIRE_KNOWLEDGE_DIR`), the same
//! one the app reads — which is the point: the RAG the app searches is this store. Because both would
//! then be writing it, **quit the app first** (one writer at a time).
//!
//! The wiring below is the app's, in miniature: a KnowledgeStore graph at `knowledge_dir`, a second
//! graph for provenance edges, an embedder registered as a service, and a `RagActor` built from the
//! registry. Deliberately the same shape as `ffi.rs` — a fill that worked here and not in the app
//! would be worse than no test.

use spire_actor::registry::ServiceRegistry;
use spire_actor::ActorSystem;
use spire_core::actors::rag::{EmbedderService, RagActor, RagMessage};
use spire_core::actors::MemoryGraphMessage as MgMsg;
use spire_core::models::embedding::Embedder;
use spire_core::subsystems::graph::memory_graph::MemoryGraphActor;
use spire_esp32::actors::rag_bundle;
use spire_esp32::build::idf_projects::promote_part_document;
use std::sync::Arc;

/// Which corpora to ingest: all of them, or the ones named in `SPIRE_FILL_CORPUS` (comma-separated)
/// so one can be refreshed without re-cloning the others.
fn corpora() -> Vec<String> {
    match std::env::var("SPIRE_FILL_CORPUS") {
        Ok(list) if !list.trim().is_empty() => list
            .split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect(),
        _ => rag_bundle::BUNDLE
            .iter()
            .map(|(c, _)| (*c).to_string())
            .collect(),
    }
}

/// Initialize a graph actor at `dir` — the two calls the app makes at startup.
async fn init_graph(tx: &tokio::sync::mpsc::Sender<MgMsg>, dir: &std::path::Path) {
    let (t, r) = tokio::sync::oneshot::channel();
    tx.send(MgMsg::Initialize {
        data_dir: dir.to_path_buf(),
        reply_to: t,
    })
    .await
    .expect("send Initialize");
    r.await.expect("Initialize reply").expect("Initialize");
}

/// The real embedder, or `None` with a line saying why — in which case the caller must **return**,
/// not substitute `NoopEmbedder`.
///
/// `NoopEmbedder` is not a slower embedder: it fails every call on purpose, so that RAG surfaces a
/// missing model instead of degrading to zero vectors. A store therefore cannot even be *ingested*
/// without one — the ingest embeds each chunk — which is why there is no lexical-only fallback to
/// fall back to and a machine without the model gets a skip rather than an assertion about a store
/// that was never filled.
///
/// Built **inside** the runtime deliberately: the model is cached on this machine, so no Hugging Face
/// round trip happens and hf-hub's blocking client is never reached. (`ffi.rs` builds it *outside* the
/// runtime because a first-ever download would panic otherwise; if this ever panics with "Cannot start
/// a runtime from within a runtime", that is what changed.)
fn real_embedder() -> Option<Arc<dyn Embedder>> {
    match spire_core::embedder::CandleEmbedder::new() {
        Ok(embedder) => Some(Arc::new(embedder)),
        Err(e) => {
            eprintln!(
                "no Candle embedder ({e}) — nothing can be ingested without one, so this is skipped"
            );
            None
        }
    }
}

#[ignore = "live fill: clones from GitHub (or reads the local SDK) and embeds — minutes"]
#[tokio::test]
async fn the_bundled_corpora_ingest_into_the_knowledge_store() {
    let store = spire_core::config::knowledge_dir();
    std::fs::create_dir_all(&store).expect("create knowledge dir");
    println!("KnowledgeStore: {}", store.display());

    // 1. Install: the manifests, as `<corpus>/ingest.yaml`, which is what RagView lists.
    let installed = rag_bundle::install_into(&store).expect("install the bundle");
    println!("installed: {installed:?}");

    // 2. The app's RAG wiring.
    let system = ActorSystem::new();
    let (memory_graph_tx, _) = system.spawn(MemoryGraphActor::new());
    let (knowledge_tx, _) = system.spawn(MemoryGraphActor::new());
    // The project graph is only a provenance sink here; the corpus lives in the store.
    let project_data = tempfile::tempdir().expect("project data dir");
    init_graph(&memory_graph_tx, project_data.path()).await;
    init_graph(&knowledge_tx, &store).await;

    // The real embedder, built inside the runtime — see [`real_embedder`].
    let Some(embedder) = real_embedder() else {
        return;
    };
    let registry = Arc::new(ServiceRegistry::new());
    let _ = registry.register_service("embedder", Arc::new(EmbedderService(embedder.clone())));
    let _ = registry.register::<MgMsg>("knowledge_graph", knowledge_tx.clone());
    let _ = registry.register::<MgMsg>("memory_graph", memory_graph_tx.clone());
    {
        let (t, r) = tokio::sync::oneshot::channel();
        knowledge_tx
            .send(MgMsg::InitializeEmbedder {
                model_path: None,
                embedder: Some(embedder),
                reply_to: t,
            })
            .await
            .expect("send InitializeEmbedder");
        let _ = r.await;
    }
    let (rag_tx, _) = system.spawn(RagActor::from_registry(
        knowledge_tx,
        memory_graph_tx,
        registry.clone(),
    ));

    // 3. Ingest each corpus, and keep the counts so retrieval can be checked against them.
    let mut filled: Vec<(String, u32)> = Vec::new();
    for corpus in corpora() {
        let manifest = store.join(&corpus).join("ingest.yaml");
        assert!(
            manifest.is_file(),
            "{} is not installed — is '{corpus}' spelled as it is in the bundle?",
            manifest.display()
        );
        let (t, r) = tokio::sync::oneshot::channel();
        // Reingest *replaces* the domain (clear, then ingest) rather than merging into it, which is
        // what a corrected manifest needs: merging would leave the chunks that the old patterns
        // matched and the new ones beside them. Set `SPIRE_FILL_REINGEST=1` when a manifest changed.
        if std::env::var("SPIRE_FILL_REINGEST").is_ok() {
            rag_tx
                .send(RagMessage::ReingestGraphConfig {
                    manifest_path: manifest,
                    project_root: None,
                    reply_to: t,
                })
                .await
                .expect("send ReingestGraphConfig");
        } else {
            rag_tx
                .send(RagMessage::IngestGraphConfig {
                    manifest_path: manifest,
                    project_root: None,
                    reply_to: t,
                })
                .await
                .expect("send IngestGraphConfig");
        }
        let report = r.await.expect("reply").expect("ingest");
        for source in &report.sources {
            println!(
                "  {} [{}] {} files, {} chunks{}",
                source.id,
                source.source_type,
                source.files,
                source.chunks,
                if source.status == "ok" {
                    String::new()
                } else {
                    format!(" — {}: {}", source.status, source.reason)
                }
            );
        }
        println!(
            "{corpus}: {} chunks, {} entities, {} relationships (skipped: {:?})",
            report.chunks, report.entities, report.relationships, report.sources_skipped
        );
        assert!(
            report.chunks > 0,
            "{corpus} ingested nothing — a manifest whose patterns match no file looks exactly like \
             this: {report:?}"
        );
        filled.push((corpus, report.chunks));
    }

    // 3b. What the store actually holds, per domain. This is the diagnostic that separates "the
    // ingest did not write" from "the write is not visible to a search": `chunk_count` is counted
    // directly over the graph, while `semantic_retrieve` only scans the first 500 `rag_chunk` nodes
    // it is handed — with the pre-existing corpora in this store, a fresh corpus can sit outside that
    // window and be unfindable however well it was written.
    {
        let (t, r) = tokio::sync::oneshot::channel();
        rag_tx
            .send(RagMessage::ListDomains { reply_to: t })
            .await
            .expect("send ListDomains");
        let mut domains = r.await.expect("reply").expect("list");
        domains.sort_by_key(|d| std::cmp::Reverse(d.chunk_count));
        println!("--- domains in the store ({}):", domains.len());
        for d in domains.iter().take(12) {
            println!(
                "    {:<16} {:>6} chunks  {:>4} sources  {:>4} entities",
                d.id, d.chunk_count, d.source_count, d.entity_count
            );
        }
        println!(
            "    total chunks: {}",
            domains.iter().map(|d| d.chunk_count).sum::<u64>()
        );
    }

    // 4. Retrieval, against the domains just filled — the half a chunk count does not prove.
    for (corpus, _) in &filled {
        let (t, r) = tokio::sync::oneshot::channel();
        rag_tx
            .send(RagMessage::Query {
                domain: corpus.clone(),
                query: "toggle a GPIO output pin".to_string(),
                top_k: 3,
                reply_to: t,
            })
            .await
            .expect("send Query");
        let hits = r.await.expect("reply").expect("query");
        let top = hits.first();
        println!(
            "{corpus}: {} hits, best {:.3} from {}",
            hits.len(),
            top.map(|h| h.score).unwrap_or(0.0),
            top.map(|h| h.source_path.clone())
                .unwrap_or_else(|| "—".to_string())
        );
        assert!(
            !hits.is_empty(),
            "{corpus} answered a query with nothing, so its chunks are not retrievable"
        );
    }
}

/// The `device-facts` corpus' premise, on its own: a **part number** answers with that part.
///
/// That corpus is asked by *key* — one document per part or board, named for it — and the documents that ship
/// are deliberately close (`sht20.md` and `sht30.md`: one family, both measuring without clock
/// stretching, the same CRC polynomial, and a different address, command set, framing and CRC seed).
/// `ft6336.md` and `gt911.md` are the second pair, and close in *purpose* rather than in text: two
/// touch controllers on the same kind of panel, whose register maps share nothing.
/// `st7789.md` and `ili9342.md` are the third: two SPI panels with the same command/data conversation
/// and different command words, so a document that described only the *shape* would answer for both.
/// So this is the property the corpus exists for, and the one a similarity search can fail: asking
/// for one part must return *that* part's document and not its neighbour's.
///
/// Unlike the live fill it is **not** `#[ignore]`d: the corpus is a `local` source, so nothing here
/// touches the network. The embedder, though, has to be the real one — `NoopEmbedder` **fails every
/// call** rather than degrading to zero vectors, so a store cannot even be *ingested* without the
/// model; when it is not on this machine the test says so and returns. That is what makes the
/// assertion below a claim about the corpus as the app will search it.
///
/// **The margin is small, and that is the finding worth knowing.** The two documents are ~90%
/// identical text, so for `sht20` the scores came out 0.049 / 0.047 against 0.093 / 0.051 for
/// `sht30`: the right document wins both times, by a hair in one case. Nothing about that is
/// random — the same model and the same text give the same answer every run — but it does mean a
/// change to either document must be **re-measured** rather than assumed to have kept the order, and
/// it is why the seam labels every chunk with its source path (`#### …/sht20.md`) instead of letting
/// the model infer which of two near-identical protocols is this part's. The scores are printed on
/// every run so the margin is visible rather than inferred from a pass.
///
/// The later pairs measure the same thing at two different distances: `ft6336` / `gt911` are close
/// in *purpose* rather than in text (0.096 against 0.079), and `cores3` is not a near miss at all yet
/// competes with `ft6336.md` because both describe the board's display path (0.134 against 0.112).
/// The third is the SPI panels, `st7789` / `ili9342` — measured in the paragraph below, because the
/// first version of `st7789.md` lost to `gt911.md`, which is not even a panel. Re-measure all of them
/// after editing either document in a pair.
///
/// **The head of a document is the whole of its embedding.** A 3 KB chunk is longer than the
/// embedder's context, so the vector that decides retrieval comes from the opening lines and the
/// rest is only what the *answer* carries. This was measured on `st7789.md`: with the part number in
/// the title and the `Part number` line alone, a query for `st7789` ranked its own document **fourth**
/// (0.054, behind `gt911.md`'s 0.075) — repeating `ST7789` twice in the opening paragraph moved it to
/// first (0.097) and changed nothing else in the file. So a document is not answerable because it
/// *contains* the part number; it is answerable because the part number is in its first two sentences,
/// and a new document has to be checked here rather than assumed to work from its key.
///
/// **The key alone is not enough, and that is the finding a batch regeneration produced.** Asking for
/// `axp2101` returned `bmi270.md` first and `axp2101.md` third; asking for `ili9342` returned
/// `gt911.md` first. The embedding of a bare part number carries almost no signal, and every
/// document opens with the same boilerplate. So the seam now does what a corpus keyed by part should:
/// it asks for enough chunks to hold the whole corpus, **promotes the document named for the part to
/// the front by name**, and records only that one as the spec's fact — the near-misses stay in the
/// prompt, after it, as the context they are for. The raw scores are still printed below, because
/// they are what says whether a document's head is doing its job.
#[tokio::test]
async fn a_device_facts_query_answers_with_that_part_and_not_its_neighbour() {
    let Some(embedder) = real_embedder() else {
        return;
    };
    let tmp = tempfile::tempdir().expect("temp dir");
    let store = tmp.path().join("knowledge");
    std::fs::create_dir_all(&store).expect("create store");
    // The installer is what puts each document where the manifest's relative `path` resolves to.
    rag_bundle::install_into(&store).expect("install the bundle");

    let system = ActorSystem::new();
    let (memory_graph_tx, _) = system.spawn(MemoryGraphActor::new());
    let (knowledge_tx, _) = system.spawn(MemoryGraphActor::new());
    let project_data = tmp.path().join("project");
    std::fs::create_dir_all(&project_data).expect("create project data dir");
    init_graph(&memory_graph_tx, &project_data).await;
    init_graph(&knowledge_tx, &store).await;

    // Deliberately the real embedder: see the doc comment — retrieval is a vector search, and a
    // store cannot be filled without the model at all.
    {
        let (t, r) = tokio::sync::oneshot::channel();
        knowledge_tx
            .send(MgMsg::InitializeEmbedder {
                model_path: None,
                embedder: Some(embedder.clone()),
                reply_to: t,
            })
            .await
            .expect("send InitializeEmbedder");
        let _ = r.await;
    }
    let registry = Arc::new(ServiceRegistry::new());
    let _ = registry.register_service("embedder", Arc::new(EmbedderService(embedder)));
    let _ = registry.register::<MgMsg>("knowledge_graph", knowledge_tx.clone());
    let _ = registry.register::<MgMsg>("memory_graph", memory_graph_tx.clone());
    let (rag_tx, _) = system.spawn(RagActor::from_registry(
        knowledge_tx,
        memory_graph_tx,
        registry.clone(),
    ));

    let (t, r) = tokio::sync::oneshot::channel();
    rag_tx
        .send(RagMessage::IngestGraphConfig {
            manifest_path: store
                .join(rag_bundle::DEVICE_FACTS_CORPUS)
                .join("ingest.yaml"),
            project_root: None,
            reply_to: t,
        })
        .await
        .expect("send IngestGraphConfig");
    let report = r.await.expect("reply").expect("ingest");
    assert_eq!(
        report.chunks as usize,
        rag_bundle::FACTS_DOCS.len(),
        "one document is one chunk — a split document is half a protocol, which is what the corpus' \
         chunk_size exists to prevent"
    );

    // Each part, and the neighbour that must not answer for it. The byte is what a *retrieved* chunk
    // has to carry to be usable: the command table, not just the framing it shares with the neighbour.
    // The pairs live in `MEASURED` below so the **coverage** of this corpus is checkable without a
    // model: a table that is inline in a loop cannot be counted, and an unmeasured document is exactly
    // how `axp2101` and `ili9342` came to be answered by their neighbours.
    let mut lost: Vec<String> = Vec::new();
    for &(part, neighbour, fact) in MEASURED {
        let (t, r) = tokio::sync::oneshot::channel();
        rag_tx
            .send(RagMessage::Query {
                domain: rag_bundle::DEVICE_FACTS_CORPUS.to_string(),
                query: part.to_string(),
                // The whole corpus, so the part's own document is *there* to be selected even when
                // the embedding ranks it below a neighbour. `top_k = 3` was the earlier measurement,
                // and it hid `axp2101.md` completely on the `axp2101` query.
                top_k: rag_bundle::FACTS_DOCS.len(),
                reply_to: t,
            })
            .await
            .expect("send Query");
        let raw = r.await.expect("reply").expect("query");

        // Printed before promotion, because this is the embedding's own answer: it is what says
        // whether a document's head is doing its job, and it is where the near-miss problem shows.
        println!(
            "{part}: {} hits (its near miss is {neighbour}), retrieval's order — {:#?}",
            raw.len(),
            raw.iter()
                .map(|h| (h.source_path.as_str(), h.score))
                .collect::<Vec<_>>()
        );

        let mut hits = raw;
        promote_part_document(part, &mut hits);
        let Some(top) = hits.first() else {
            lost.push(format!("{part}: answered with nothing"));
            continue;
        };
        println!(
            "{part}: answered by {} ({:.4}) after promotion",
            top.source_path, top.score
        );

        if !top.source_path.ends_with(&format!("{part}.md")) {
            lost.push(format!(
                "{part}: answered by {} — the promotion did not find the part's own document, so it \
                 was not in the corpus' answer at all",
                top.source_path
            ));
        }
        // No assertion about *which* near-misses came back: the store drops chunks whose cosine is
        // not positive, so which neighbours survive the cut is the embedding's business rather than a
        // property of the corpus. The pair that has to be visible — `sht20` before `sht30` — is
        // asserted in `idf_component_edit_tests`, where the prompt is the thing under test.
        if !top.text.contains(fact) {
            lost.push(format!(
                "{part}: the chunk retrieved does not carry its own command word ({fact}), so it is \
                 not the whole document"
            ));
        }
    }
    // One run names every part that lost, rather than the first: the corpus is re-measured after any
    // document edit, and a test that stopped at the first miss would hide the rest of them.
    assert!(
        lost.is_empty(),
        "these parts were not answered by their own protocol:\n  {}",
        lost.join("\n  ")
    );
}
/// The measured pairs: one per document in [`rag_bundle::FACTS_DOCS`] — the key the corpus is asked
/// with, the neighbour that must not answer for it, and a fact only its own document carries.
///
/// A table, not an array inside the loop, because **coverage is part of the claim**. The batch
/// regeneration that wrote `bmi270.md` into `axp2101`'s spec is what made this explicit: the documents
/// nobody measured were the ones that lost. [`every_facts_document_is_measured_as_itself`] is the
/// lock-in; this is the evidence, and a new document is unmeasured until a row here says otherwise.
const MEASURED: &[(&str, &str, &str)] = &[
    ("sht20", "sht30.md", "0xF3"),
    ("sht30", "sht20.md", "0x2400"),
    // The touch pair: the FT6336's firmware-id register is not the GT911's buffer-status register,
    // and a driver written from the wrong one does not talk to the part at all.
    ("ft6336", "gt911.md", "0xA6"),
    ("gt911", "ft6336.md", "0x814E"),
    // A board is keyed by its own name and has to answer with its wiring, not with the protocol of
    // the part that sits on it — `GPIO35` is the DC/MISO overlap, which only the board knows.
    ("cores3", "ft6336.md", "GPIO35"),
    // The third near miss: two SPI panels with the same DCS-shaped conversation, so a document that
    // only said "a colour LCD driven by commands" would answer for either. `0xB7` is the ST7789's
    // gate control, a command the ILI9342 document does not carry.
    ("st7789", "ili9342.md", "0xB7"),
    // The parts below had no retrieval case until a batch regeneration wrote the **wrong** document
    // into `axp2101`'s and `ili9342`'s specs — `bmi270.md` and `gt911.md`. A corpus this small is
    // searched by an embedding of the opening lines, so every document needs a measured case, not
    // just the pairs somebody happened to think about: the ones that were never measured are the ones
    // that lost.
    ("axp2101", "bmi270.md", "0x4A"),
    ("bm8563", "bmi270.md", "century_months"),
    ("bmi270", "axp2101.md", "0x24"),
    ("ili9342", "st7789.md", "panel's own requirement"),
    ("sps30", "sht30.md", "0xD033"),
];

/// **Every facts document is measured, and measured as itself.**
///
/// The live measurement above is the evidence; this is the lock-in that keeps the evidence in step
/// with the corpus, and it needs no model and no store — so it runs in the ordinary suite. A document
/// added to [`rag_bundle::FACTS_DOCS`] without a row here fails immediately, rather than waiting for
/// a live run to reveal that nobody thought to ask about it.
///
/// It is a *set* comparison rather than a count: a table that measured one part twice and skipped its
/// neighbour would have the right length and still leave a document unchecked.
#[test]
fn every_facts_document_is_measured_as_itself() {
    let mut measured: Vec<&str> = MEASURED.iter().map(|(part, _, _)| *part).collect();
    measured.sort_unstable();
    let mut documents: Vec<&str> = rag_bundle::FACTS_DOCS
        .iter()
        .map(|(file, _)| {
            file.strip_suffix(".md")
                .expect("a facts document is named <key>.md")
        })
        .collect();
    documents.sort_unstable();
    assert_eq!(
        measured, documents,
        "the measurement table and the shipped corpus must name the same parts, one row each: a \
         document nobody measured is a document nobody checked"
    );
}

/// **A document's opening sentences are what its key can reach.**
///
/// The embedder's context is shorter than a document, so the vector a query is answered by comes from
/// the head. `st7789.md` measured 0.054 and ranked **fourth** while its part number appeared only in
/// the title and the `Part number` line; repeating `ST7789` in the opening paragraph moved it to
/// first (0.097) and changed nothing else in the file. The seam's promotion-by-key then makes the
/// right document win regardless of rank — but promotion can only reach a document retrieval returned
/// at all, so the head still has to name the part.
///
/// This is checked statically, over every document, because the live measurement is expensive and
/// easy to let fall behind: a new document is reviewed here rather than discovered by a batch run
/// writing the wrong protocol into a spec.
#[test]
fn every_facts_document_names_its_part_in_its_opening_sentences() {
    let mut lost = Vec::new();
    for (file, text) in rag_bundle::FACTS_DOCS {
        let key = file
            .strip_suffix(".md")
            .expect("a facts document is named <key>.md");
        let opening = opening_sentences(text, 2);
        if !opening.to_lowercase().contains(key) {
            lost.push(format!(
                "{file}: its first two sentences do not name `{key}`, so the head of the chunk does \
                 not carry the key: {opening:?}"
            ));
        }
    }
    assert!(
        lost.is_empty(),
        "these documents cannot be reached by their own key:\n  {}",
        lost.join("\n  ")
    );
}

/// The first `count` sentences of a document's body — headings removed, lines flattened to one.
///
/// A sentence ends at a period followed by whitespace. That is enough for prose written to be read by
/// an embedder, and a real sentence splitter would be a dependency answering a question this corpus'
/// own style already makes easy.
fn opening_sentences(text: &str, count: usize) -> String {
    let body: String = text
        .lines()
        .skip_while(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");

    let mut opening = String::new();
    let mut sentences = 0;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        opening.push(c);
        if c == '.' && matches!(chars.peek(), Some(next) if next.is_whitespace()) {
            sentences += 1;
            if sentences == count {
                break;
            }
        }
    }
    opening
}
