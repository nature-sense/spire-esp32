# Provenance

`spire-esp32` is a **fork**, not a rewrite: it was created by copying the
ESP32/ESP-IDF subset of `spire-code` (and, in Phase 2, the `spire-idf`
component library) into a new repository so the ESP32 pipeline can evolve
without carrying the polyglot build layer.

The two source repositories are left untouched and remain the reference
implementations.

## Source state at copy time

| Repo | HEAD | Working tree |
| --- | --- | --- |
| `../spire-code` | `ef2ae64` | 37 modified/untracked paths (the copy includes this working-tree state) |
| `../spire-idf` | `39c1616` | 2 modified/untracked paths (to be folded in during Phase 2) |

Copied on 2026-10-18.

Because the copy was taken from the working tree, the device-facts work
(`ft6336.md`, `gt911.md`, `cores3.md`, their `FACTS_DOCS` registration and the
retrieval-manifest gates) is included even though it was not committed in
`spire-code`.

## What was copied

- `crates/spire-esp32/` — the whole crate (src, tests, examples, resources,
  `templates/esp-idf`, `templates/container`) as the starting point. The
  non-ESP32 modules are being removed in place; see the trim log below.
- `ui/swift/` — SwiftUI sources and `Package.swift` (no `.build/`).
- `docs/`, `schema/`, `tools/`, `build/`, `Makefile`, `Cargo.lock`,
  `.gitignore`.

Not copied: `target/`, `ui/swift/.build/`, `.spire/`, the original Git
history, and `spire-code`'s root `templates/` (the Rust `spire-app` scaffold,
which has no ESP32 counterpart).

## Renames

| Before | After |
| --- | --- |
| package `spire-code` | package `spire-esp32` |
| lib crate `spire_code` | lib crate `spire_esp32` |
| binary `spire-core` | binary `spire-esp32` |
| `libspire_code.dylib` | `libspire_esp32.dylib` |

The C ABI symbol names (`spire_send_json`, `spire_wait_for_event`, …) are
unchanged, so the Swift bridge only needed the dylib filename updated.

## Trim log

Phase 1 keeps the crate compiling under its new name first; the non-ESP32
build modules are then deleted with the compiler as the guide.

Removed in Phase 1 (with every registration and test that enumerated them):

| Removed | Reason |
| --- | --- |
| `build/cargo.rs`, `node.rs`, `swift.rs`, `python.rs`, `go.rs`, `maven.rs`, `gradle.rs`, `make.rs`, `ruby.rs`, `meson.rs` | language/build modules outside the ESP32/C++ domain |
| `build/hal_migration.rs`, `hal_rust_contract.rs` | the Rust-on-ESP32 HAL path (locked decision: C++ only) |
| `build/spire_app_scaffold.rs` | Rust `spire-app` workspace scaffolder |
| `templates/container/` | Rust actor/executor container template |
| `examples/spire_app_e2e.rs` | Rust app end-to-end example |
| `tests/build_config_routing_tests.rs` | Cargo + Make + SwiftPM multi-config routing |
| `tests/system_flow_tests.rs` | every fixture shells out to `meson setup`; Phase 4 rebuilds these flows on the ESP-IDF/CMake fixtures |
| `parse_source_file_routes_to_cargo_module` | Rust tree-sitter parse path |

Changed in the same pass:

- `cpp_compile_gate` now drives the host C++ build with **CMake**
  (`cmake -S . -B build-<platform>`, `cmake --build build-<platform>`) instead
  of Meson, including its "not configured" setup message and the test that
  pins it.
- `analyze_routes_to_cargo_module` → `analyze_routes_to_cmake_module`.
- FFI/standalone bootstrap register **cmake** as the config-file module and
  **esp-idf** as the platform module; the module-operation capability test
  pins both.
- `build_default_registry`/tool tests no longer enumerate the dropped modules.

Still to trim (Phase 3): the generic `ProjectStructure` branches in
`project_creation.rs`, capability vocabulary entries, and the Swift UI's
language/target pickers.

## Phase 1 acceptance

Run in the new tree on 2026-10-18:

| Gate | Result |
| --- | --- |
| `cargo check --workspace --all-targets` | pass |
| `cargo fmt -p spire-esp32 --check` | pass |
| `cargo test -p spire-esp32` | pass (exit 0; 0 failures) |

Suites that ran: 483 lib tests, 62 actor tests, 1 build-tool registration,
1 capability profile, 4 IDF component-edit, 4 IDF project-routing,
16 black-box integration (the real `spire-esp32` binary over TCP),
4 modify-code LLM, 11 plan-orchestrator, 6 RAG manifests, 1 RAG fill,
28 tool-orchestrator.

## Phase 2 — the component library, folded in

`../spire-idf@39c1616` (working tree, including its 2 uncommitted paths) is now
`idf/` at the repository root: `CMakeLists.txt`, `sdkconfig.defaults`, `main/`
(the build harness), `components/` (the framework, drivers and `lvgl_display`),
`tools/`, `SPIRE.md`, `API.md`, `README.md`.

Copied without `.git`, `.spire/`, `build/`, `sdkconfig`, and the per-component
`test/build/` directories. LVGL (`components/lvgl/upstream`, a **submodule** in
the source repo) is now **vendored in place**: its files are committed,
`.gitmodules` is removed, and `tools/vendor-lvgl.sh --status` reports the
vendored version (`9.6.0`) instead of moving a gitlink.

Coupling re-pointed:

- `CreateProjectSheet.defaultLibrary` now points at
  `~/naturesense/spire/spire-esp32/idf`.
- `resources/rag-ingest/spire-idf.ingest.yaml` is a `local` source
  (`path: docs`) instead of `github_repo`; nothing is fetched at install time.
- `rag_bundle::IDF_DOCS` lists the documents shipped into
  `<store>/spire-idf/docs/` (`SPIRE.md`, `README.md`, `API.md`,
  `components/lvgl/README.md`), and `install_into` writes them.
- The manifest now claims `**/API.md`. The new
  `the_spire_idf_corpus_ships_the_library_documents_it_installs` gate caught
  that `API.md` was shipped but unclaimed — a corpus silently missing the
  function reference.

Acceptance:

| Gate | Result |
| --- | --- |
| `idf.py set-target esp32s3 build` (in `idf/`) | exit 0; `idf/build/spire-idf.bin` produced |
| host `ctest` in every `components/*/test` | 12/12 suites pass |
| `cargo test -p spire-esp32 --test rag_bundle_manifests` | 7/7 |

## Phase 3 — de-specialize

Done so far:

- The **New project** catalogue (`ui/swift/.../Models/ProjectType.swift`) is the
  ESP32 pair only: *ESP32 Components* (`idf_library`) and *ESP32 Application*
  (`idf_application`). The Rust+SwiftUI row and both Linux SBC stubs are gone,
  and the Swift gates in `SpireUITests.swift` now pin the two rows.
- `schema/capabilities.yaml` and `src/capability_vocabulary.rs` were inspected:
  they are **hardware** vocabulary (radio, bus, pins, roles), not language
  vocabulary. There is nothing language-specific to remove there.
- The binary rename (`spire-core` → `spire-esp32`), the Swift dylib name, and the
  standalone bootstrap (cmake + esp-idf modules only) landed in Phase 1.

Still to de-specialize (Phase 3b) — the platform model keeps the Rust-on-ESP32
path alive as dead shape:

| Location | Rust-only remnant |
| --- | --- |
| `src/platform.rs:162,204` | `Platform.rust: Option<PlatformRust>`, `struct PlatformRust` |
| `src/platform.rs:145,73` | `Platform.hal: Option<PlatformHal>` (an `esp-hal` crate pin) |
| `src/actors/platform_codec.rs:92-100,385-392` | persists/reads `rust_target`, `rust_idf_target`, `rust_flash` |
| `src/actors/platform_codec.rs:114-121,372-379` | persists/reads `hal_crate`, `hal_version`, `hal_features` |
| `ui/swift/.../Models/Platform.swift:97` | `PlatformRust` mirror (the `rust` key) |
| `ui/swift/.../ProjectAnalysisView.swift:265-270` | `spire_app` / Meson HAL branches, inert for IDF projects |

Removing those is a self-contained change (platform struct + codec + Swift
mirror + their round-trip tests). It was deliberately kept separate from this
checkpoint so the platform schema change is one reviewable commit.

## Phase 4 — acceptance ladder

| Gate | Result |
| --- | --- |
| `cargo test -p spire-esp32 --lib` | 483 passed (7 ignored), exit 0 |
| `cargo test -p spire-esp32 --test rag_bundle_manifests` | 7/7 |
| `idf_project_routing_tests` / `idf_component_edit_tests` | 4 + 4 pass |
| host `ctest` in all 12 `components/*/test` suites | 12/12 |
| `SPIRE_FILL_CORPUS=device-facts … rag_fill_tests -- --ignored` | 6 docs ingested into the store |
| `a_device_facts_query_answers_with_that_part_and_not_its_neighbour` | pass; `cores3` 0.1338 vs `ft6336` 0.1124 — the margins measured before the fork are unchanged |
| scaffold of `PM25_METER` (minus the BSP, see below) → `idf.py set-target esp32s3 build` | exit 0; `build/pm25-meter.bin` produced |

### Two findings from the end-to-end scaffold

1. **The canonical example named a library component that does not exist.**
   `examples::PM25_METER` (and `docs/example-composition.spire`,
   `docs/composition-format.md`) said `moving_average`; the library's component
   is `rolling_average` (`RollingAverage`). A scaffold of the example therefore
   emitted `REQUIRES … moving_average` and CMake refused the build with
   `Failed to resolve component 'moving_average' required by component
   'air_quality'`. The example and the assertions derived from it now say
   `rolling_average`. Synthetic tests that use `moving_average` as an arbitrary
   component name are unchanged, as is the deliberate "design claims a library
   component the library does not have" mismatch test.

2. **A board BSP is resolved by the ESP-IDF component manager at build time.**
   The example's board names `bsp: m5stack_core_s3`, so the scaffold writes
   `main/idf_component.yml` with `m5stack_core_s3: "*"`. On this machine the
   manager reported `no versions of espressif/m5stack_core_s3 match *` even
   though the registry serves the component — a component-manager/registry
   resolution problem, not the scaffold (the dependency is correct and the
   component exists). The end-to-end build was therefore run with the same
   design **minus the BSP** (`board: { chip: esp32s3 }`), which exercises the
   whole scaffold → CMake → compile path without a managed dependency.

## Phase 5 — tidy

- `README.md` describes the merged tree and its build/test entry points.
- `docs/*.md` now resolve under `crates/spire-esp32/`; the `rag_bundle`
  `file:line` anchors were re-pointed after `IDF_DOCS` moved `install_into`,
  and the "only corpus with shipped documents" claim now covers both
  `device-facts` and `spire-idf`.
- `.gitignore` covers the vendored library's build output, `sdkconfig` and the
  per-component host-test `build/` directories.
- Fresh history: four commits from the initial import to the Phase 4 gates;
  this file is the provenance record.






