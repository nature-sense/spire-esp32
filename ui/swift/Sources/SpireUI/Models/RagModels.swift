import Foundation

/// A per-domain summary of the ingested RAG corpus ("state of the data"),
/// mirror of `spire_core::actors::rag::RagDomainInfo`.
struct RagDomainInfo: Codable, Identifiable, Hashable {
    /// Clean corpus/domain id (e.g. "rpi5", "spire-core") — NOT `rag_domain:<id>`.
    let id: String
    let name: String
    let description: String
    let chunkCount: Int
    let sourceCount: Int
    let corpusVersion: String
    let tokenCount: Int

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case description
        case chunkCount = "chunk_count"
        case sourceCount = "source_count"
        case corpusVersion = "corpus_version"
        case tokenCount = "token_count"
    }
}

/// A discoverable ingestion manifest ("ingest script"), mirror of
/// `spire_core::actors::rag::RagManifestInfo`. `domain` is the corpus the
/// script builds — declared via `pipeline.corpus`, else the manifest's own
/// directory name (`~/.spire/knowledge/<corpus>/ingest.yaml`).
struct RagManifestInfo: Codable, Identifiable, Hashable {
    let domain: String
    /// Absolute path to the `ingest.yaml` file.
    let path: String
    let corpusVersion: String
    let description: String

    var id: String { path }

    enum CodingKeys: String, CodingKey {
        case domain
        case path
        case corpusVersion = "corpus_version"
        case description
    }
}

/// A single retrieved RAG chunk, mirror of `spire_core::actors::rag::RagChunkResult`.
struct RagChunkResult: Codable, Identifiable, Hashable {
    let domain: String
    let sourcePath: String
    let chunkIndex: Int
    let text: String
    let score: Double

    var id: String { "\(domain):\(sourcePath):\(chunkIndex)" }

    enum CodingKeys: String, CodingKey {
        case domain
        case sourcePath = "source_path"
        case chunkIndex = "chunk_index"
        case text
        case score
    }
}

/// One source's status from the latest ingest run, mirror of
/// `spire_core::actors::rag_ingest::SourceStatus`.
struct RagSourceStatus: Codable, Identifiable, Hashable {
    let id: String
    let sourceType: String
    let status: String       // "ok" | "skipped"
    let reason: String
    let chunks: Int
    let files: Int

    enum CodingKeys: String, CodingKey {
        case id
        case sourceType = "source_type"
        case status
        case reason
        case chunks
        case files
    }
}

/// Full result of an ingest run, mirror of
/// `spire_core::actors::rag_ingest::IngestReport`.

struct RagIngestReport: Codable, Hashable {
    let domain: String
    let corpusVersion: String
    let chunks: Int
    let entities: Int
    let relationships: Int
    let sources: [RagSourceStatus]

    enum CodingKeys: String, CodingKey {
        case domain
        case corpusVersion = "corpus_version"
        case chunks
        case entities
        case relationships
        case sources
    }
}

/// Legacy alias: number of chunks stored (kept for callers of the old shape).
typealias RagIngestResult = UInt32
