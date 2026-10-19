// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! Shared helpers for the language-specific build modules.
//!
//! `run_cmd` executes an external command in a project directory and captures
//! structured `BuildOutput`. `parse_key_value` extracts `name` / `version` /
//! `description` from a simple `key = value` config file, optionally scoped to
//! a `[section]` (empty section = whole file).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

use super::{AstEdgeData, AstNodeData, AstParseResult, BuildOutput};
use regex::Regex;
use sha2::{Digest, Sha256};
use spire_core::build_types::{BuildMetadata, BuildSpec};
use std::collections::HashSet;

/// Execute a program in a project dir and capture structured output.
pub async fn run_cmd(path: &Path, program: &str, args: &[&str]) -> Result<BuildOutput, String> {
    run_cmd_with_env(path, program, args, &[]).await
}

/// Execute a program in a project dir with an optional env overrides list,
/// capturing structured output. `working_dir` (when non-empty) is joined onto
/// `path` so a `BuildSpec` can target a subdirectory of the module root.
pub async fn run_cmd_with_env(
    path: &Path,
    program: &str,
    args: &[&str],
    env: &[(String, String)],
) -> Result<BuildOutput, String> {
    let start = Instant::now();
    let command_str = format!("{program} {}", args.join(" "));
    let mut cmd = Command::new(program);
    cmd.current_dir(path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to execute {program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    Ok(BuildOutput {
        success: output.status.success(),
        command: command_str,
        duration_secs: start.elapsed().as_secs_f64(),
        output: if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}\n{stderr}")
        },
        exit_code: output.status.code(),
    })
}

/// `[package] name` from the project's `Cargo.toml`, or `None`.
///
/// A crate's binary is named after its package unless a `[[bin]]` section overrides it — and when
/// it does, the derived path simply does not exist and the caller's refusal names the path it
/// looked for, which is more useful than running whatever it found.
///
/// Read by hand rather than with a TOML parser: it is one key, and the modules that need it already
/// read their manifests this way.
pub fn cargo_package_name(root: &Path) -> Option<String> {
    let content = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("name") {
            if let Some(value) = value.trim_start().strip_prefix('=') {
                let name = value.trim().trim_matches('"').trim();
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// The binary a `cargo build --target <triple>` writes, or `None` when its name cannot be known.
///
/// `target/<triple>/<profile>/<name>`: the triple is a **subdirectory** because `--target` was
/// passed (a host build puts the binary straight in `target/<profile>/`), and the profile is
/// `release` only for a release build — flashing a debug artifact from the release path, or the
/// reverse, is a wrong-binary flash that no tool would catch.
///
/// `explicit` wins when given (a `[[bin]]` name, or an artifact some other step produced);
/// otherwise the name is `package`, falling back to the `[package] name` in `Cargo.toml`.
///
/// Shared by the platform build modules (esp-idf, rp2040): both cross-compile with `--target`, so
/// both derive the artifact the same way and only the invocation differs.
pub fn cargo_artifact_path(
    root: &Path,
    triple: &str,
    mode: &str,
    package: Option<&str>,
    explicit: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        // Relative paths are relative to the project, like every other path in a build request.
        return Some(if explicit.is_absolute() {
            explicit.to_path_buf()
        } else {
            root.join(explicit)
        });
    }
    let name = package
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| cargo_package_name(root))?;
    let profile = if mode.eq_ignore_ascii_case("release") {
        "release"
    } else {
        "debug"
    };
    Some(root.join("target").join(triple).join(profile).join(name))
}

/// A host `[program, args…]` as a [`BuildSpec`] — the counterpart to a `cargo` invocation.
///
/// **No environment**, deliberately: a flasher is a host binary (cargo-installed), so unlike a
/// cross-build it needs neither a toolchain on `PATH` nor a vendor SDK's variables. Setting them
/// here would be cargo-cult, and a `PATH` that hides the user's own tools is a real failure mode
/// rather than a theoretical one.
pub fn build_spec_from_command(mut command: Vec<String>) -> BuildSpec {
    // The builders always name a program, so the first element is the program; the empty case is
    // handled rather than asserted so a future caller that passes nothing gets an empty command
    // that fails visibly, not a panic in a flash path.
    let arguments = if command.is_empty() {
        Vec::new()
    } else {
        command.split_off(1)
    };
    BuildSpec {
        command: command.into_iter().next().unwrap_or_default(),
        arguments,
        working_dir: String::new(),
        env: Vec::new(),
    }
}

/// Execute a normalized `BuildSpec` invocation: `working_dir` (relative to the
/// module root) is joined onto `path`, env overrides are applied, and the
/// command's stdout/stderr are captured into a `BuildOutput`. Modules route
/// `Build`/`BuildStreaming` directly to this when the selected target carries a
/// `build_spec`.
pub async fn run_build_spec(path: &Path, spec: &BuildSpec) -> Result<BuildOutput, String> {
    let cwd = if spec.working_dir.is_empty() {
        path.to_path_buf()
    } else {
        path.join(&spec.working_dir)
    };
    let args: Vec<&str> = spec.arguments.iter().map(|s| s.as_str()).collect();
    run_cmd_with_env(&cwd, &spec.command, &args, &spec.env).await
}

/// Lightweight regex-based source parser for generic modules (go, cmake, make,
/// maven, gradle, ruby). Extracts functions, classes, and imports without a
/// full tree-sitter grammar.
pub fn parse_source_file_basic(
    file_path: &Path,
    language: &str,
    content: &str,
    content_hash: &str,
) -> AstParseResult {
    let mut nodes: Vec<AstNodeData> = Vec::new();
    let mut edges: Vec<AstEdgeData> = Vec::new();

    // ── Pass 1: imports ────────────────────────────────────────────
    let import_patterns: &[(&str, &str)] = &[
        ("import", r"(?m)^\s*(?:import|from)\s+([A-Za-z0-9_\.]+)"),
        (
            "use",
            r"(?m)^\s*(?:use|using|include|require)\s+([A-Za-z0-9_\.:\/]+)",
        ),
    ];
    let mut import_lines: HashSet<u32> = HashSet::new();
    for (label, pat) in import_patterns {
        let re = match Regex::new(pat) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for cap in re.captures_iter(content) {
            let name = cap.get(1).map(|m| m.as_str().to_string());
            let pos = cap.get(0).map(|m| m.start()).unwrap_or(0);
            let (line, col) = offset_to_line_col(content, pos);
            import_lines.insert(line);
            let node = AstNodeData {
                node_type: "import".to_string(),
                name: name.clone(),
                text: cap
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
                start_line: line,
                start_col: col,
                end_line: line,
                end_col: col + cap.get(0).map(|m| m.as_str().len() as u32).unwrap_or(0),
                depth: 0,
                is_public: false,
                is_async: false,
                signature: None,
                return_type: None,
                children: Vec::new(),
            };
            let idx = nodes.len();
            nodes.push(node);
            edges.push(AstEdgeData {
                from_index: idx,
                to_index: idx,
                edge_type: "imports".to_string(),
                order: Some(idx as u32),
                field: Some(label.to_string()),
            });
        }
    }

    // ── Pass 2: function / class declarations ──────────────────────
    let decl_patterns: &[(&str, &str)] = &[
        (
            "function",
            r"(?m)^\s*(?:pub\s+|public\s+|export\s+)?(?:async\s+|fn\s+|func\s+|def\s+|function\s+)([A-Za-z0-9_]+)\s*(?:<[^>]+>)?\s*\(",
        ),
        (
            "class",
            r"(?m)^\s*(?:pub\s+|public\s+|export\s+)?(?:class\s+|struct\s+|trait\s+|type\s+|interface\s+|enum\s+|protocol\s+|extension\s+)([A-Za-z0-9_]+)",
        ),
    ];
    for (label, pat) in decl_patterns {
        let re = match Regex::new(pat) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for cap in re.captures_iter(content) {
            let name = cap.get(1).map(|m| m.as_str().to_string());
            let pos = cap.get(0).map(|m| m.start()).unwrap_or(0);
            let (line, col) = offset_to_line_col(content, pos);
            if import_lines.contains(&line) {
                continue;
            }
            let full_match = cap
                .get(0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let is_public = full_match.starts_with("pub ")
                || full_match.starts_with("public ")
                || full_match.starts_with("export ");
            let is_async = full_match.contains("async ");
            let text = full_match.clone();
            let signature_len = full_match.len() as u32;
            let node = AstNodeData {
                node_type: label.to_string(),
                name: name.clone(),
                text,
                start_line: line,
                start_col: col,
                end_line: line,
                end_col: col + signature_len,
                depth: 0,
                is_public,
                is_async,
                signature: Some(full_match),
                return_type: None,
                children: Vec::new(),
            };
            nodes.push(node);
        }
    }

    AstParseResult {
        file_path: file_path.to_string_lossy().to_string(),
        language: language.to_string(),
        content_hash: content_hash.to_string(),
        has_errors: nodes.is_empty(),
        nodes,
        edges,
    }
}

/// Convert a byte offset in a string to a (1-based line, 0-based col).
pub fn offset_to_line_col(content: &str, offset: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut col = 0u32;
    for (i, ch) in content.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Compute a SHA-256 hex digest of file content (for incremental re-parse).
pub fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Parse a source file from disk using the basic regex parser. Returns an
/// `AstParseResult` with a computed content hash. This is the default
/// implementation for modules without a dedicated tree-sitter grammar.
pub fn parse_source_file_std(file_path: &Path, language: &str) -> Result<AstParseResult, String> {
    let content = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?;
    let hash = sha256_hex(&content);
    Ok(parse_source_file_basic(
        file_path, language, &content, &hash,
    ))
}

/// Parse a C/C++ source/header file. Runs the basic parser (imports + class
/// declarations), then `extract_cpp_methods` to add member-function nodes
/// (interface methods, constructors, destructors) as `child` edges of their
/// enclosing class/struct, with `signature`/`return_type` populated and the
/// `virtual`/`= 0`/`= default` markers preserved in `signature`.
///
/// NOTE: this is the generic AST-storage parser (headers parsed into the
/// knowledge graph via `ParseSourceFile`); it only provides the AST nodes.
pub fn parse_cpp_source_file_std(file_path: &Path) -> Result<AstParseResult, String> {
    let content = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?;
    let hash = sha256_hex(&content);

    // Pass 1: the generic parser (imports + class/struct headers).
    let mut base = parse_source_file_basic(file_path, "C/C++", &content, &hash);

    // Pass 2: member-function extraction. `extract_cpp_methods` returns
    // (enclosing class name, method node) pairs; resolve each method's parent
    // to the class node index in the base list and emit the `child` edge.
    use std::collections::HashMap;
    let extracted = extract_cpp_methods(&content);
    if !extracted.is_empty() {
        let mut class_index: HashMap<&str, usize> = HashMap::new();
        for (i, n) in base.nodes.iter().enumerate() {
            if n.node_type == "class" {
                if let Some(name) = n.name.as_deref() {
                    class_index.entry(name).or_insert(i);
                }
            }
        }
        let mut method_nodes: Vec<AstNodeData> = Vec::new();
        let mut child_edges: Vec<AstEdgeData> = Vec::new();
        for (class_name, method) in extracted {
            let method_idx = base.nodes.len() + method_nodes.len();
            if let Some(&parent) = class_index.get(class_name.as_str()) {
                child_edges.push(AstEdgeData {
                    from_index: parent,
                    to_index: method_idx,
                    edge_type: "child".to_string(),
                    order: Some(method_nodes.len() as u32),
                    field: None,
                });
            }
            method_nodes.push(method);
        }
        base.nodes.extend(method_nodes);
        base.edges.extend(child_edges);
    }
    base.has_errors = base.nodes.is_empty();
    Ok(base)
}

/// Extract member-function declarations (methods, constructors, destructors)
/// from C++ `class`/`struct` bodies.
///
/// For each class/struct we brace-match its body and match member declarations
/// of the forms:
///
///   virtual ReturnType name(params) const override = 0;
///   ReturnType name(params) const;
///   explicit ClassName(args);
///   virtual ~ClassName();
///
/// Each match yields an `AstNodeData` with `node_type = "method"`, the full
/// declaration in `signature` (so `virtual` / `= 0` / `= default` are visible
/// to callers), `return_type` (None for ctor/dtor),
/// and `is_public` from the access specifier active at that method's line.
/// The returned vector is `(enclosing class name, method node)` — the caller
/// resolves the class node index to build the `child` edge.
fn extract_cpp_methods(content: &str) -> Vec<(String, AstNodeData)> {
    let mut out: Vec<(String, AstNodeData)> = Vec::new();

    // Class/struct header with an opening brace on the same logical line:
    //   class CameraHAL {   /   struct Foo : public Bar {
    let class_re =
        Regex::new(r"(?m)^\s*(?:class|struct)\s+([A-Za-z_][A-Za-z0-9_]*)\b[^{]*\{").unwrap();

    // Member-function declaration regex with named groups. The return-type
    // group is greedy but followed by a method-name token + `(`, so it stops
    // at the longest prefix that leaves a valid name/`(`. `~`-prefixed names
    // are destructors (no return type). End delimiter `;` or `{` (inline).
    let method_re = Regex::new(
        r"(?m)^\s*(?P<head>virtual\s+|static\s+|explicit\s+|virtual\s+static\s+)*\s*(?P<ret>[\w:<>,.*&\[\]\s]+?)?\s*(?P<name>[A-Za-z_~][A-Za-z0-9_:~]*)\s*\((?P<params>[^;{}]*?)\)\s*(?:const\s+)?(?:override\s+)?(?P<purity>=\s*(?:0|default|delete))?\s*[;{]",
    )
    .unwrap();

    for cap in class_re.captures_iter(content) {
        let Some(cname) = cap.get(1) else { continue };
        let class_name = cname.as_str().to_string();
        let Some(body_start_match) = cap.get(0) else {
            continue;
        };
        // The match ends exactly at the class body's `{`.
        let body_open = body_start_match.end() - 1;
        let Some(body_close) = match_closing_brace(content, body_open) else {
            continue; // unbalanced — skip this class
        };
        let body = &content[body_open + 1..body_close];
        let body_start = body_open + 1;

        // Access region resolution: walk the body's lines and record per-line
        // the last access specifier seen. C++ default access differs by kind:
        //   class  → private
        //   struct → public
        // Determine the kind from the matched header (`struct X {` → public).
        let header_text = cap.get(0).map(|g| g.as_str()).unwrap_or("");
        let header_kind_struct = header_text.trim_start().starts_with("struct");
        let mut line_public: Vec<bool> = Vec::new();
        let mut is_public = header_kind_struct; // class default: private; struct default: public
        for line in body.lines() {
            let t = line.trim_start();
            if t.starts_with("public:") {
                is_public = true;
            } else if t.starts_with("private:") || t.starts_with("protected:") {
                is_public = false;
            }
            line_public.push(is_public);
        }

        for m in method_re.captures_iter(body) {
            let Some(mname) = m.name("name") else {
                continue;
            };
            let name = mname.as_str().trim();
            // Reject control-flow / statement keywords that a `(…)` could
            // otherwise capture (e.g. `if (…) {` inside an inline body).
            if name.is_empty()
                || name == "if"
                || name == "for"
                || name == "while"
                || name == "switch"
                || name == "return"
                || name == "catch"
                || name == "sizeof"
            {
                continue;
            }
            // The return-type group may have greedily eaten a scoped name
            // (e.g. `Some::Type method`), but `name` already matched a token
            // that includes `::` segments — dedupe: if the matched `name`
            // equals a word just seen in `ret`, treat it as part of the
            // qualified name. Simple heuristic: if `ret` ends with `name`,
            // the group boundary was wrong — prefer the LONGEST name (the
            // qualified one).
            let name = {
                let raw = name;
                if let Some(ret) = m.name("ret") {
                    let ret_last_ws = ret.as_str().trim_end();
                    let last_word = ret_last_ws
                        .rsplit(|c: char| c.is_whitespace() || c == ':' || c == '<' || c == ',')
                        .next()
                        .unwrap_or("");
                    if !last_word.is_empty() && raw.starts_with(last_word) {
                        // `ret` already contains the last segment — combine:
                        // qualified name = `<ret stripped of trailing ws> + raw`.
                        // This is rare; keep raw and drop the ret's last word.
                        format!("{}::{}", last_word, raw)
                    } else {
                        raw.to_string()
                    }
                } else {
                    raw.to_string()
                }
            };

            let full = m
                .get(0)
                .map(|g| g.as_str().trim().to_string())
                .unwrap_or_default();
            if full.is_empty() {
                continue;
            }

            // Return type from the named group (None for ctor/dtor).
            let return_type = if name.starts_with('~') || name.contains('~') {
                None
            } else {
                match m.name("ret") {
                    Some(ret) => {
                        let r = ret.as_str().trim();
                        if r.is_empty() {
                            None
                        } else {
                            Some(r.to_string())
                        }
                    }
                    None => None,
                }
            };

            // Purity marker (`= 0`, `= default`, `= delete`) preserved in the
            // signature and exposed as a property via `signature`.
            let purity = m.name("purity").map(|p| p.as_str().trim().to_string());

            // Absolute position for line/col.
            let rel = m.get(0).map(|g| g.start()).unwrap_or(0);
            let (abs_line, abs_col) = offset_to_line_col(content, body_start + rel);

            // Access region: the line the method starts on.
            let rel_line = body[..rel].matches('\n').count();
            let is_public = line_public.get(rel_line).copied().unwrap_or(false);

            let mut sig = full.clone();
            if let Some(p) = &purity {
                // Ensure the purity marker is retained even though the regex
                // captured it outside the `full` match is unlikely — but keep.
                if !sig.contains("=") && p.starts_with('=') {
                    sig.push(' ');
                    sig.push_str(p);
                }
            }

            // Precompute for `end_col` (full is moved into `text` below).
            let full_len = full.chars().count() as u32;

            out.push((
                class_name.clone(),
                AstNodeData {
                    node_type: "method".to_string(),
                    name: Some(name),
                    text: full,
                    start_line: abs_line,
                    start_col: abs_col,
                    // End = start + declared length (single-line decls).
                    end_line: abs_line,
                    end_col: abs_col + full_len,
                    depth: 1,
                    is_public,
                    is_async: false,
                    signature: Some(sig),
                    return_type,
                    children: Vec::new(),
                },
            ));
        }
    }

    out
}

/// Find the offset just past the `}` matching the `{` at `open`.
fn match_closing_brace(content: &str, open: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b'"' => {
                // skip string literal (best-effort)
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    break;
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract `name` / `version` / `description` from a `key = value` config.
///
/// `section` may be empty (parse the whole file) or a heading like `[project]`.
pub fn parse_key_value(path: &Path, config: &str, section: &str) -> Result<BuildMetadata, String> {
    let content = std::fs::read_to_string(path.join(config))
        .map_err(|e| format!("Failed to read {config}: {e}"))?;
    let mut name = None;
    let mut version = None;
    let mut description = None;
    let mut in_section = section.is_empty();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = section.is_empty() || trimmed[1..trimmed.len() - 1] == *section;
            continue;
        }
        if !in_section {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if name.is_none() && lower.starts_with("name = ") {
            if let Some(val) = trimmed.split('=').nth(1) {
                name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
        if version.is_none() && lower.starts_with("version = ") {
            if let Some(val) = trimmed.split('=').nth(1) {
                version = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
        if description.is_none() && lower.starts_with("description = ") {
            if let Some(val) = trimmed.split('=').nth(1) {
                description = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    }

    Ok(BuildMetadata {
        project_name: name,
        description,
        version,
        config_files: vec![config.to_string()],
        project_path: Some(path.to_string_lossy().to_string()),
        ..Default::default()
    })
}

/// Curated `spire-actor` + `spire-core` API surface injected into the fill
/// prompt for **SpireApp** projects, so the LLM builds on the actual framework
/// instead of inventing APIs.
pub fn spire_framework_hints() -> String {
    r#"SPIRE FRAMEWORK (spire-actor + spire-core) — build the app on these:

Spire-core API surface:
- build_types: BuildMetadata, BuildSpec, ProjectStructure, ProjectDomain,
  Dependency, BuildTarget, Platform (registry via Platform::from_registry)
- embedder::candle_embedder: CandleEmbedder — embed() -> embeddings; defaults
  to 384 dims; set SPIRE_USE_METAL=1 at runtime for the Metal/GPU backend
- rag: ingest + query over the ~/.spire/knowledge manifests (see docs/rag.md)
- subsystems::llm: LlmMessage::Complete { prompt, role, reply_to } with
  LlmModelRole::{Planning, Coding, ...}
- modules::FilesystemMessage: CreateDirectory / WriteFile / ReadFile
- config: config_dir(), Settings (API keys, model selection)
- actors: ToolInfo and the actor message protocol

Spire-actor API surface:
- Actor trait: `Actor` with an associated Message type + async handle(msg)
- Long-lived child actors spawned once at startup (the pattern spire-esp32 uses
  for its build/planning/rag actors); wire them with mpsc channels + oneshot
  reply_to, then route JSON requests to them.

Conventions:
- Keep `spire_send_json` (#[no_mangle] extern "C", JSON in -> JSON out) and
  `spire_free_string` exported from the crate: the SwiftUI app dlopens the
  dylib and talks JSON over these two symbols.
- App logic lives under crates/<crate>/src and the SwiftUI views under
  ui/swift/Sources (the fill roots). Build configs, the FFI bridge and
  assemble-app.sh are structural — never rewrite them.
- Deps are declared ONCE via declare_dependencies against the crate manifest."#
        .to_string()
}

/// One C++ syntax error: (line, col) + the tree-sitter node kind + context.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CppSyntaxError {
    pub line: u32,
    pub col: u32,
    pub kind: String,
    pub context: String,
}

/// Result of a structural C++ syntax check (tree-sitter CST).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CppSyntaxReport {
    pub ok: bool,
    #[serde(default)]
    pub errors: Vec<CppSyntaxError>,
}

/// Build a tree-sitter C++ parser (fresh per call — `Parser` is not Sync).
fn cpp_parser() -> tree_sitter::Parser {
    let mut p = tree_sitter::Parser::new();
    let _ = p.set_language(&tree_sitter_cpp::LANGUAGE.into());
    p
}

/// The C++ tree-sitter parser, for callers that need node SPANS.
///
/// `parse_with_tree_sitter` deliberately produces the canonical `AstNode` model, which
/// carries no byte offsets — so anything doing byte-precise work (locating the node a
/// diagnostic points at, replacing exactly that span) needs the raw tree instead. Sharing
/// this parser keeps "which grammar" in one place.
pub(crate) fn cpp_parser_for_spans() -> tree_sitter::Parser {
    cpp_parser()
}

/// Structural C++ syntax check using the same `tree-sitter-cpp` CST the
/// structural edit tooling consumes. `ok == false` means the parser hit
/// ERROR/MISSING nodes.
/// Structural only — missing `#include`s / ill-typed expressions need a compiler.
pub fn cpp_syntax_check(content: &str) -> CppSyntaxReport {
    let mut parser = cpp_parser();
    let Some(tree) = parser.parse(content, None) else {
        return CppSyntaxReport {
            ok: false,
            errors: Vec::new(),
        };
    };
    let root = tree.root_node();
    if !root.has_error() {
        return CppSyntaxReport {
            ok: true,
            errors: Vec::new(),
        };
    }
    let mut errors = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            let (row, col) = (node.start_position().row, node.start_position().column);
            let context = content
                .lines()
                .nth(row)
                .map(|l| l.trim().to_string())
                .unwrap_or_default();
            errors.push(CppSyntaxError {
                line: row as u32 + 1,
                col: col as u32 + 1,
                kind: node.kind().to_string(),
                context,
            });
            continue; // don't descend into erroneous nodes
        }
        let mut cursor = node.walk();
        for c in node.named_children(&mut cursor) {
            stack.push(c);
        }
    }
    CppSyntaxReport { ok: false, errors }
}

/// Strip a ```code fence (optionally tagged `cpp`) from an LLM response so the
/// returned text is a raw file. Shared by the coordinator's whole-file repair
/// flows.
pub fn strip_code_fences(text: &str) -> String {
    let t = text.trim();
    if t.starts_with("```") {
        let after = t.strip_prefix("```").unwrap_or(t);
        let after = after
            .strip_prefix("cpp")
            .map(str::trim_start)
            .unwrap_or_else(|| after.trim_start());
        let pos = after.find("```").unwrap_or(after.len());
        after[..pos].trim().to_string()
    } else {
        t.to_string()
    }
}

/// The **whole file** a rewrite reply states: [`strip_code_fences`], with the one newline a file
/// ends in.
///
/// The stripper has to trim — the fence line, the blank line a model sometimes leaves inside it, the
/// prose around it — and that trim also takes the file's last byte. Writing the result anyway leaves a
/// component's file without the newline every other file in the tree has, which is both a whole-file
/// diff for a one-line change and an exactness the gate cannot see (a compiler does not care). A live
/// audit of all ten components showed exactly that: every regenerated file differed from the one it
/// replaced by nothing but its final newline.
///
/// Only for a value that *is* a file. A path list or a prose answer stays with
/// [`strip_code_fences`], where the trailing newline is not part of what was asked.
pub fn strip_code_block_as_file(text: &str) -> String {
    let mut body = strip_code_fences(text);
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

/// the single-file dashboard flow, where there is nothing extra to show.
pub fn compile_fix_prompt(
    path: &str,
    content: &str,
    errors: &[String],
    related: &[(String, String)],
) -> String {
    let mut out = String::new();
    out.push_str("Fix the COMPILER diagnostics in this C/C++ file by rewriting the WHOLE file.\n");
    out.push_str(&format!("File: {path}\n\nDiagnostics:\n"));
    for (i, e) in errors.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, e));
    }
    out.push_str(&format!("\nCurrent content:\n```cpp\n{content}\n```\n"));
    // A mismatch spread across two files is fixed by lining them up, and the model can only do that
    // if it sees both. Read-only: the repair still rewrites exactly one file.
    if !related.is_empty() {
        out.push_str(
            "\nOther files the compiler named, shown only so you can see the types that must line \
             up. Do NOT rewrite these:\n",
        );
        for (rel_path, rel_content) in related {
            out.push_str(&format!("\n// {rel_path}\n```cpp\n{rel_content}\n```\n"));
        }
    }
    out.push_str(
        "\nReturn ONLY the complete corrected file inside a single ```cpp block.\n\
         Change ONLY what is required to resolve the reported diagnostics. Keep every \
         other line, signature, include and behaviour byte-identical: do NOT reformat, \
         do NOT reorder includes, do NOT rename anything, and add no commentary outside \
         the code block.",
    );
    out
}

/// One thing a compiler complained about, with everything it printed around it.
///
/// The three parts are all load-bearing for a repair pass, and the first one is why this is a struct
/// rather than a `Vec<String>`:
///
///  * **`related`** is the `In file included from …` chain and the files the `note:` lines name. The
///    fix usually lives in one of *these*, not in `file`. A live build of a generated application
///    reported `no matching function for call to 'app::Sampler::Sampler(…)'` inside the **library's**
///    `scheduler.hpp` — a header the application may not change — while the mistake was the
///    spawn call in `main.cpp`; a repair that read only the blamed file would look at the wrong file
///    and give up;
///  * **`lines`** carries the `note:` lines, which say what the compiler expected
///    (`no known conversion for argument 2 from 'ActorRef<Reading>' to 'ActorRef<Offsets>'` is the
///    whole repair, and passing it on costs nothing);
///  * **`file`** is the blamed file, kept as printed: it is what a diagnostics list keys by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildDiagnostic {
    /// The file the compiler blamed, exactly as it printed it.
    pub file: String,
    /// Every other file named around this diagnostic, innermost first: the include chain above it and
    /// the files its `note:` lines point at.
    pub related: Vec<String>,
    /// The `error:` line and the `note:` lines under it, verbatim, in the order they were printed.
    pub lines: Vec<String>,
}

impl BuildDiagnostic {
    /// The files a repair pass could act on, in the order it should try them: the file the compiler
    /// **blamed** first, then the files around it.
    ///
    /// The blamed file wins whenever a repair may touch it, because *the compiler blamed this file* is
    /// the strongest signal there is — a syntax error is in *that* file and nowhere else. The chain and
    /// the notes are the fallback that makes the interesting case work: a mismatch reported inside the
    /// library's `scheduler.hpp` is a call in the `main.cpp` that included it, and a repair that never
    /// looked past the blamed file would look at a header it must not change.
    ///
    /// The order was the other way round first, and a live run found the cost: a syntax error in
    /// `main/touch.hpp` arrived under an `In file included from …/main/main.cpp:8:` chain, so the repair
    /// rewrote `main.cpp` and left the broken file exactly as broken.
    pub fn candidates(&self) -> Vec<&str> {
        std::iter::once(self.file.as_str())
            .chain(self.related.iter().map(String::as_str))
            .collect()
    }

    /// Whether this is an overload-resolution failure — the one error shape whose fix is genuinely
    /// ambiguous between the two files it names.
    ///
    /// `no matching function for call to 'app::Power::Power(…)'` over a `candidate:` note that says
    /// `no known conversion for argument 1 from 'ActorRef<app::Report>' to 'ActorRef<app::Battery>'`
    /// is a shape a live build produced: the spawn site passed `ActorRef<Report>` and the constructor
    /// wanted `ActorRef<Battery>`, and **either** could be the mistake. A repair that fixed only the
    /// first fillable candidate — the call site, which the include chain reaches before the note —
    /// looped forever, because `main.cpp` cannot be made to match a declaration that is itself wrong.
    pub fn is_overload_mismatch(&self) -> bool {
        self.lines.iter().any(|line| {
            line.contains("no matching function for call")
                || line.contains("no known conversion for argument")
        })
    }
}

/// Read a compiler's output into the diagnostics a repair pass acts on.
///
/// Compilers print `path:line:col: error: …`, the `note:` lines that explain it, and — when the error
/// sits inside a header — an `In file included from …` chain above it. A line that is none of those is a
/// continuation (the source line, a caret diagram) and belongs to nothing. Warnings are **ignored**: a
/// warning is not a broken build, and a pass that rewrote files because of one would be a pass nobody
/// asked for.
///
/// GCC prints a chain as `In file included from A:N:` followed by `from B:N:` continuations, and IDF
/// prints one chain per diagnostic. Both forms are read here, because which one a compiler uses is the
/// compiler's business and not the caller's.
pub fn group_diagnostics(text: &str) -> Vec<BuildDiagnostic> {
    let mut found: Vec<BuildDiagnostic> = Vec::new();
    let mut chain: Vec<String> = Vec::new();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("In file included from ") {
            chain = vec![location_file(rest)];
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("from ") {
            if !chain.is_empty() {
                chain.push(location_file(rest));
            }
            continue;
        }
        let Some((file, severity)) = diagnostic_head(line) else {
            continue;
        };
        match severity {
            Severity::Error => found.push(BuildDiagnostic {
                file: file.clone(),
                related: std::mem::take(&mut chain),
                lines: vec![line.trim_end().to_string()],
            }),
            // A note explains the error above it, even when it names a *different* file — which is
            // exactly the case worth catching, because that other file is where the fix goes.
            Severity::Note => {
                if let Some(last) = found.last_mut() {
                    // Deduplicated: an include chain and a note often name the same file, and a
                    // candidate list that repeats itself makes a prompt that repeats itself.
                    if !last.related.contains(&file) {
                        last.related.push(file);
                    }
                    last.lines.push(line.trim_end().to_string());
                }
            }
        }
    }
    found
}

enum Severity {
    Error,
    Note,
}

/// The file and severity a compiler diagnostic line states, or `None` when the line is not one.
///
/// `fatal error:` is tried before `error:` because the former contains the latter. The path is
/// everything before the marker with its `:line:col` stripped, and it must contain no space — a
/// compiler prints `path:line:col: error: message`, and a looser match would turn a *message* that
/// quotes a diagnostic into a filename.
fn diagnostic_head(line: &str) -> Option<(String, Severity)> {
    for (marker, severity) in [
        ("fatal error: ", Severity::Error),
        ("error: ", Severity::Error),
        ("note: ", Severity::Note),
    ] {
        if let Some(at) = line.find(marker) {
            let before = line[..at].trim_end_matches(':').trim();
            if before.is_empty() || before.contains(' ') {
                return None;
            }
            return Some((location_file(before), severity));
        }
    }
    None
}

/// `path:line:col` — or `path:line`, or `path` — to `path`.
fn location_file(location: &str) -> String {
    let trimmed = location.trim_end_matches(':');
    // The column and the line are the last two colon-separated fields, and only when they are numbers:
    // a path may contain a colon, a version number may look like a line.
    match trimmed.rsplit_once(':') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) => {
            match head.rsplit_once(':') {
                Some((path, line)) if line.chars().all(|c| c.is_ascii_digit()) => path.to_string(),
                _ => head.to_string(),
            }
        }
        _ => trimmed.to_string(),
    }
}

/// The **fillable** files a set of diagnostics points at, each with the diagnostics that named it.
///
/// `fill_roots` is the scaffold's own rule — the directories a model may write into — and it is the
/// whole reason this exists: a repair that could rewrite anything would be a repair that could break the
/// scaffold it was reading. A diagnostic whose every candidate lies outside those roots is left out, and
/// the caller reports it: the mistake is then in a file the **library** owns, and the fix is a person's.
///
/// Candidates are tried in the order [`BuildDiagnostic::candidates`] gives, so a call site beats the
/// header it called into — which is the case a real build produced, where the error was reported inside
/// `scheduler.hpp` and belonged to `main.cpp`. Paths outside `root` are never ours, and a path
/// that is not a file is not repaired either, whatever the compiler said about it.
///
/// The one exception is an overload-resolution failure ([`BuildDiagnostic::is_overload_mismatch`]),
/// whose fix can sit at the call site **or** in the declaration the compiler offered as a `candidate:`.
/// There every fillable candidate gets a repair step, so the pass reaches the header the error was
/// really about instead of rewriting the call site into the same wrong shape forever.
pub fn repairable_sources(
    root: &std::path::Path,
    fill_roots: &[String],
    diagnostics: &[BuildDiagnostic],
) -> Vec<(std::path::PathBuf, Vec<BuildDiagnostic>)> {
    let prefix = format!("{}/", root.to_string_lossy());
    let mut by_file: Vec<(std::path::PathBuf, Vec<BuildDiagnostic>)> = Vec::new();
    for diagnostic in diagnostics {
        // Every other error has one home — the first fillable candidate — and stopping there keeps a
        // repair from sprawling into a file the compiler only named in passing. An overload-resolution
        // failure is the exception: it names a call site *and* the declaration it could not match, and
        // either could carry the fix, so both are repaired.
        let every_fillable_candidate = diagnostic.is_overload_mismatch();
        for candidate in diagnostic.candidates() {
            let relative = candidate.strip_prefix(&prefix).unwrap_or(candidate);
            let path = root.join(relative);
            let inside = fill_roots
                .iter()
                .any(|fill_root| path.starts_with(root.join(fill_root)));
            if !inside || !path.is_file() {
                continue;
            }
            match by_file.iter_mut().find(|(known, _)| known == &path) {
                Some((_, list)) => list.push(diagnostic.clone()),
                None => by_file.push((path, vec![diagnostic.clone()])),
            }
            if !every_fillable_candidate {
                break;
            }
        }
    }
    by_file
}

/// The **other** fillable files the compiler named around a diagnostic, with their current content,
/// for a repair prompt that has to line two files up.
///
/// [`repairable_sources`] decides what the repair may rewrite; this decides what it may *read*. The
/// file already being repaired is left out, as is anything outside the fill roots — the same rule, for
/// the same reason: a repair must not read the library into its own fix. It is read-only context, never
/// a second write, and it is what turns `no known conversion for argument 1 …` from a note the model
/// sees into the second file it needs to reconcile.
pub fn related_sources(
    root: &std::path::Path,
    fill_roots: &[String],
    diagnostics: &[BuildDiagnostic],
    repaired: &std::path::Path,
) -> Vec<(String, String)> {
    let prefix = format!("{}/", root.to_string_lossy());
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    for diagnostic in diagnostics {
        for candidate in diagnostic.candidates() {
            let relative = candidate.strip_prefix(&prefix).unwrap_or(candidate);
            let path = root.join(relative);
            let inside = fill_roots
                .iter()
                .any(|fill_root| path.starts_with(root.join(fill_root)));
            if !inside || !path.is_file() || path == repaired || seen.contains(&path) {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                let relative = path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                seen.push(path);
                out.push((relative, content));
            }
        }
    }
    out
}

/// Prompt for `modify/code`: carry out a free-text request by rewriting this file.
///
/// Deliberately narrow, in the same spirit as `compile_fix_prompt`: the request is the
/// only reason to touch anything, so the model is told to leave the rest of the file
/// byte-identical. A free-text request is exactly where a model is most tempted to
/// "improve" neighbouring code, and that is the opposite of what a verified,
/// rollback-safe change is for.
pub fn modify_code_prompt(path: &str, content: &str, request: &str) -> String {
    let mut out = String::new();
    out.push_str("Change this C/C++ file by rewriting the WHOLE file, to carry out the request.\n");
    out.push_str(&format!("File: {path}\n\nRequest:\n{request}\n\n"));
    out.push_str(&format!("Current content:\n```cpp\n{content}\n```\n\n"));
    out.push_str(
        "Return ONLY the complete file inside a single ```cpp block.\n\
         Make the smallest change that carries out the request, and keep every other \
         line, signature, include and behaviour byte-identical: do NOT reformat, do NOT \
         reorder includes, do NOT rename anything the request did not name, and add no \
         commentary outside the code block. If the request needs no change in this file, \
         return the file exactly as it is.",
    );
    out
}

/// Prompt for the file-selection step of `modify/code`: which of these files does the
/// request touch?
///
/// The answer is a plain list of paths, for two reasons. The reply stays small and
/// cheap to parse instead of holding several files' contents in one answer; and each
/// rewrite then goes through the same single-file path — and the same structural check
/// — that the compile-fix loop already trusts, so nothing new is written unattended
/// without being parsed first.
pub fn modify_scope_prompt(files: &[String], request: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "A change has been requested. List ONLY the files below that must be edited to \
         carry it out.\n\nRequest:\n",
    );
    out.push_str(request);
    out.push_str("\n\nFiles:\n");
    for file in files {
        out.push_str(&format!("- {file}\n"));
    }
    out.push_str(
        "\nAnswer with one file path per line, exactly as written above, and nothing \
         else. No explanations, no bullets, no code fences. If no file needs to change, \
         answer with the single word NONE.",
    );
    out
}

/// Prompt for the safe-warning phase: clear exactly the listed warnings without
/// changing behaviour.
///
/// The warnings that reach this prompt are only the ones classified as safe
/// (`deadcode.*`, unused values, self-assignment). The instruction to preserve
/// side effects is deliberate: a "dead" store to a volatile or otherwise
/// observable location must not lose its effect.
pub fn warning_fix_prompt(path: &str, content: &str, warnings: &[String]) -> String {
    let mut out = String::new();
    out.push_str("Resolve the ANALYZER warnings in this C/C++ file by rewriting the WHOLE file.\n");
    out.push_str(&format!("File: {path}\n\nWarnings:\n"));
    for (i, w) in warnings.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, w));
    }
    out.push_str(&format!("\nCurrent content:\n```cpp\n{content}\n```\n\n"));
    out.push_str(
        "Return ONLY the complete corrected file inside a single ```cpp block.\n\
         Address each reported warning (e.g. remove a value that is stored but never read, \
         or drop an unused variable/parameter) and change NOTHING else: keep every other \
         line, signature, include and behaviour byte-identical. Preserve any side effect of \
         the code you remove — if removing it would change behaviour, leave that code as it \
         is. Do NOT reformat, do NOT reorder includes, do NOT rename anything, and add no \
         commentary outside the code block.",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cpp_syntax_check` validates a header structurally (tree-sitter CST).
    #[test]
    fn cpp_syntax_check_reports_structural_errors() {
        let ok_content = "namespace demo { struct A { virtual void f() = 0; }; }";
        let r = cpp_syntax_check(ok_content);
        assert!(r.ok, "clean header must pass: {r:?}");

        let bad = "namespace demo { struct A { virtual void f() = 0; ";
        let r = cpp_syntax_check(bad);
        assert!(!r.ok, "unclosed brace must fail");
        assert!(!r.errors.is_empty(), "must report a location");
    }

    #[test]
    fn strip_code_fences_removes_cpp_block() {
        assert_eq!(
            strip_code_fences("```cpp\nint main() {}\n```"),
            "int main() {}"
        );
        assert_eq!(
            strip_code_fences("```\nint main() {}\n```"),
            "int main() {}"
        );
        assert_eq!(strip_code_fences("int main() {}"), "int main() {}");
    }

    /// **A file written from a reply ends with one newline**, whatever padding the fence carried —
    /// otherwise every rewritten component loses its last byte and a trivial change reads as a
    /// whole-file diff.
    #[test]
    fn a_code_block_written_as_a_file_ends_with_one_newline() {
        assert_eq!(
            strip_code_block_as_file("```cpp\nint main() {}\n```"),
            "int main() {}\n"
        );
        assert_eq!(
            strip_code_block_as_file("```cpp\nint main() {}\n\n```"),
            "int main() {}\n",
            "a blank line inside the fence is not part of the file"
        );
        assert_eq!(
            strip_code_block_as_file("int main() {}\n"),
            "int main() {}\n",
            "an unfenced reply is a file too"
        );
        assert_eq!(
            strip_code_block_as_file("```cpp\n```"),
            "",
            "an empty block stays empty rather than becoming a newline"
        );
    }

    #[test]
    fn spire_framework_hints_cover_the_framework() {
        let hints = spire_framework_hints();
        assert!(hints.contains("spire-actor"), "hints: {hints}");
        assert!(hints.contains("spire-core"), "hints: {hints}");
        assert!(hints.contains("spire_send_json"), "hints: {hints}");
        assert!(hints.contains("CandleEmbedder"), "hints: {hints}");
        assert!(hints.contains("declare_dependencies"), "hints: {hints}");
    }

    /// The diagnostics of a **real failing build** of a generated application, verbatim.
    ///
    /// Every part of the shape is here because it was here in the log: the error is reported inside the
    /// **library's** `scheduler.hpp` while the mistake is the spawn call in `main.cpp`, the notes name
    /// the file that carries the fix and say what the compiler expected, and an include chain sits above
    /// each of them. A parser that read only `error:` lines would send a repair at a header the
    /// application may not change.
    const A_REAL_FAILING_BUILD: &str = "\
[1062/1087] Building CXX object esp-idf/main/CMakeFiles/__idf_main.dir/main.cpp.obj
FAILED: esp-idf/main/CMakeFiles/__idf_main.dir/main.cpp.obj
In file included from /tmp/app/main/main.cpp:2:
/tmp/app/library/components/actors/include/scheduler.hpp:63:23: error: no matching function for call to 'app::Sampler::Sampler(spire::ActorRef<app::Reading>&, spire::ActorRef<app::Reading>&)'
In file included from /tmp/app/main/main.cpp:8:
/tmp/app/main/sampler.hpp:23:5: note: candidate: 'app::Sampler::Sampler(spire::ActorRef<app::Reading>, spire::ActorRef<app::Offsets>)'
   23 |     Sampler(spire::ActorRef<app::Reading> air, spire::ActorRef<app::Offsets> offsets)
      |     ^~~~~~~
/tmp/app/main/sampler.hpp:23:72: note:   no known conversion for argument 2 from 'ActorRef<app::Reading>' to 'ActorRef<app::Offsets>'
warning: unused variable 'x' [-Wunused-variable]
ninja: build stopped: subcommand failed.";

    /// A compiler's output, read for the repair pass: errors grouped with their notes, warnings left
    /// alone, and the include chain kept — because the chain is where the fix usually goes.
    #[test]
    fn a_build_s_output_is_read_into_diagnostics_a_repair_can_act_on() {
        let found = group_diagnostics(A_REAL_FAILING_BUILD);
        assert_eq!(found.len(), 1, "{found:#?}");
        let diagnostic = &found[0];
        assert_eq!(
            diagnostic.file, "/tmp/app/library/components/actors/include/scheduler.hpp",
            "the file the compiler blamed, as it printed it"
        );
        assert_eq!(
            diagnostic.related,
            vec![
                "/tmp/app/main/main.cpp".to_string(),
                "/tmp/app/main/sampler.hpp".to_string(),
            ],
            "the include chain and the note's file — the two places the fix could go:\n{diagnostic:#?}"
        );
        assert!(
            diagnostic
                .lines
                .iter()
                .any(|line| line.contains("no known conversion for argument 2")),
            "the note that says what the compiler expected is the repair:\n{diagnostic:#?}"
        );
        assert_eq!(
            diagnostic.lines.len(),
            3,
            "the error and both `note:` lines under it — the notes are the compiler saying what it \
             expected, which is the repair"
        );
        assert!(
            !diagnostic
                .lines
                .iter()
                .any(|line| line.contains("unused variable")),
            "a warning is not a broken build, and a repair for one is a repair nobody asked for"
        );
        assert!(
            diagnostic
                .lines
                .iter()
                .all(|line| !line.contains("^~~~~~~")),
            "a caret diagram is a continuation, not a diagnostic:\n{diagnostic:#?}"
        );
        // **The blamed file first, the call site second.** A syntax error is in the file the compiler
        // named and nowhere else; the chain only decides it when the blamed file is one a repair may not
        // touch — which is what the guard test below is about.
        assert_eq!(
            diagnostic.candidates()[0],
            "/tmp/app/library/components/actors/include/scheduler.hpp"
        );
        assert_eq!(diagnostic.candidates()[1], "/tmp/app/main/main.cpp");
    }

    /// Which of those files a repair may touch: the scaffold's **fill roots**, and nothing else.
    ///
    /// The same rule the fill writes under, and what keeps a repair from rewriting the manifest that
    /// defines the build — or the library the application only consumes. A diagnostic whose every
    /// candidate is outside is dropped here, and that is a report to a person, not a repair.
    #[test]
    fn a_repair_may_only_touch_the_files_the_scaffold_left_open() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("main")).unwrap();
        std::fs::write(root.join("main/main.cpp"), "// the composition\n").unwrap();
        std::fs::write(root.join("main/sampler.hpp"), "// an actor\n").unwrap();
        std::fs::write(root.join("CMakeLists.txt"), "project(x)\n").unwrap();

        let fill_roots = vec!["main".to_string()];
        let found = group_diagnostics(&format!(
            "{}/main/main.cpp:4:9: error: 'Foo' was not declared in this scope\n\
             {}/CMakeLists.txt:2:1: error: Parse error. Expected \"(\"\n\
             {}/library/components/sps30/include/sps30.hpp:9:2: error: no such file\n",
            root.display(),
            root.display(),
            root.display(),
        ));
        let repairable = repairable_sources(root, &fill_roots, &found);
        assert_eq!(repairable.len(), 1, "{repairable:#?}");
        assert_eq!(repairable[0].0, root.join("main/main.cpp"));
        assert!(
            repairable[0].1[0].lines[0].contains("was not declared"),
            "{repairable:#?}"
        );

        // A diagnostic the compiler *blamed* on a locked file, whose include chain reaches a fillable
        // one: the call site is repaired, and the locked file is not touched.
        let found = group_diagnostics(&format!(
            "In file included from {}/main/sampler.hpp:3:\n\
             {}/library/components/actors/include/scheduler.hpp:63:23: error: no matching function\n",
            root.display(),
            root.display(),
        ));
        let repairable = repairable_sources(root, &fill_roots, &found);
        assert_eq!(
            repairable.len(),
            1,
            "the chain carries it back into `main/`:\n{repairable:#?}"
        );
        assert_eq!(repairable[0].0, root.join("main/sampler.hpp"));

        // Nothing in `main/` to blame: the library owns the mistake, and a person decides.
        let found = group_diagnostics(&format!(
            "{}/library/components/actors/include/actor.hpp:9:2: error: no such file\n",
            root.display()
        ));
        assert!(repairable_sources(root, &fill_roots, &found).is_empty());
    }

    /// The `pm25-reader` repair loop, in one test: an overload-resolution failure whose two files both
    /// live in `main/`.
    ///
    /// A real generated application produced exactly this — `main.cpp` spawned `Power` with a `view`
    /// that is an `ActorRef<Report>`, while `Power`'s constructor wanted `ActorRef<Battery>` — and the
    /// repair rewrote `main.cpp` on every pass, because `repairable_sources` stopped at the first
    /// fillable candidate (the call site, which the include chain reaches before the note). The call
    /// site cannot be made to match a declaration that is itself wrong, so those passes never
    /// converged: 6 compiler lines, then 3, then the same 3 again.
    const PM25_READER_OVERLOAD_MISMATCH: &str = "\
In file included from /tmp/app/main/main.cpp:2:
/tmp/app/library/components/actors/include/scheduler.hpp: In instantiation of 'app::Power* spire::Scheduler::spawn<app::Tick, app::Power>(...)':
/tmp/app/main/main.cpp:31:46:   required from here
/tmp/app/library/components/actors/include/scheduler.hpp:74:23: error: no matching function for call to 'app::Power::Power(spire::ActorRef<app::Report>&)'
In file included from /tmp/app/main/main.cpp:8:
/tmp/app/main/power.hpp:14:14: note: candidate: 'app::Power::Power(spire::ActorRef<app::Battery>)'
/tmp/app/main/power.hpp:14:45: note:   no known conversion for argument 1 from 'ActorRef<app::Report>' to 'ActorRef<app::Battery>'
ninja: build stopped: subcommand failed.";

    #[test]
    fn an_overload_mismatch_repairs_the_call_site_and_the_declaration_both() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("main")).unwrap();
        std::fs::write(root.join("main/main.cpp"), "// the composition\n").unwrap();
        std::fs::write(root.join("main/power.hpp"), "// an actor\n").unwrap();

        let fill_roots = vec!["main".to_string()];
        let found = group_diagnostics(
            &PM25_READER_OVERLOAD_MISMATCH.replace("/tmp/app", &root.display().to_string()),
        );
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(
            found[0].is_overload_mismatch(),
            "the shape is what makes this repair reach past the call site:\n{found:#?}"
        );

        let repairable = repairable_sources(root, &fill_roots, &found);
        let paths: Vec<_> = repairable.iter().map(|(path, _)| path.clone()).collect();
        assert!(
            paths.contains(&root.join("main/main.cpp")),
            "the call site is repaired:\n{repairable:#?}"
        );
        assert!(
            paths.contains(&root.join("main/power.hpp")),
            "and so is the declaration the compiler offered — the file the loop never reached:\n{repairable:#?}"
        );
        assert_eq!(
            paths.len(),
            2,
            "and the locked scheduler.hpp does not sneak in:\n{repairable:#?}"
        );
    }

    /// The repair prompt carries the *other* file the compiler named, read-only, so a mismatch that
    /// spans two files can be lined up in one pass.
    #[test]
    fn a_repair_reads_the_other_fillable_file_it_has_to_line_up() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("main")).unwrap();
        std::fs::write(root.join("main/main.cpp"), "// the composition\n").unwrap();
        std::fs::write(root.join("main/power.hpp"), "// an actor\n").unwrap();

        let fill_roots = vec!["main".to_string()];
        let found = group_diagnostics(
            &PM25_READER_OVERLOAD_MISMATCH.replace("/tmp/app", &root.display().to_string()),
        );

        // Repairing `power.hpp` shows `main.cpp`; repairing `main.cpp` shows `power.hpp`. Never the
        // locked `scheduler.hpp`, which is not fillable at all.
        let for_power = related_sources(root, &fill_roots, &found, &root.join("main/power.hpp"));
        assert_eq!(for_power.len(), 1, "{for_power:#?}");
        assert_eq!(for_power[0].0, "main/main.cpp");
        assert_eq!(for_power[0].1, "// the composition\n");

        let for_main = related_sources(root, &fill_roots, &found, &root.join("main/main.cpp"));
        assert_eq!(for_main.len(), 1, "{for_main:#?}");
        assert_eq!(for_main[0].0, "main/power.hpp");
    }

    #[test]
    fn the_repair_prompt_carries_related_files_as_read_only_context() {
        let prompt = compile_fix_prompt(
            "/tmp/app/main/power.hpp",
            "// an actor\n",
            &["power.hpp:14:14: note:   no known conversion for argument 1".to_string()],
            &[(
                "main/main.cpp".to_string(),
                "// the composition\n".to_string(),
            )],
        );
        assert!(prompt.contains("main/main.cpp"), "{prompt}");
        assert!(prompt.contains("// the composition"), "{prompt}");
        assert!(prompt.contains("Do NOT rewrite these"), "{prompt}");
        // The corrected file is still the only thing asked for.
        assert!(
            prompt.contains("Return ONLY the complete corrected file"),
            "{prompt}"
        );

        // With no related files the prompt is exactly the single-file shape the dashboard flow uses.
        let bare = compile_fix_prompt("/tmp/app/main/power.hpp", "// an actor\n", &[], &[]);
        assert!(!bare.contains("Do NOT rewrite these"), "{bare}");
    }
}
