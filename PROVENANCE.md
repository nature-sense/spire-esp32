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
  `~/naturesense/spire/spire-esp32/idf` *(Phase 7d: through `SpireCorePaths.libraryRoot`,
  which asks the core — see below)*.
- `resources/rag-ingest/spire-idf.ingest.yaml` is a `local` source
  (`path: docs`) instead of `github_repo`; nothing is fetched at install time.
  *(Phase 7d: `path` is the token `__SPIRE_LIBRARY_ROOT__`, replaced with the live
  `idf/` root as the manifest is installed.)*
- `rag_bundle::IDF_DOCS` lists the documents shipped into
  `<store>/spire-idf/docs/` (`SPIRE.md`, `README.md`, `API.md`,
  `components/lvgl/README.md`), and `install_into` writes them.
  *(Phase 7d: `IDF_DOCS` is gone; the corpus globs the live tree instead of
  shipping copies.)*
- The manifest now claims `**/API.md`. The new
  `the_spire_idf_corpus_ships_the_library_documents_it_installs` gate caught
  that `API.md` was shipped but unclaimed — a corpus silently missing the
  function reference. *(Phase 7d: replaced by
  `the_spire_idf_corpus_is_generated_from_the_embedded_library`, which pins the
  resolved path and the include/exclude globs against the real tree.)*

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

## Phase 3b — the Rust-on-ESP32 shape removed

The fork keeps only the C++/ESP-IDF half, so the shape that existed for the Rust
path was removed rather than left disabled:

| Area | What went |
| --- | --- |
| RAG corpora | `esp-rs-book`, `esp-idf-hal` and `rust` removed from `BUNDLE` (`src/actors/rag_bundle.rs`) and their manifests deleted. Remaining corpora are C/C++: `esp-idf`, `esp-idf-lib`, `esp-bsp`, `m5unified`, `waveshare`, `lvgl`, `esp-dl`, the Spire corpora and `device-facts`. |
| AST parsing | `src/build/ast_parser.rs` — the generic Rust/JavaScript/Python tree-sitter walker — deleted. It had no callers left: the C++ path is `generic_helpers::cpp_parser`, which uses `tree-sitter-cpp` directly. `tree-sitter-rust`, `tree-sitter-javascript` and `tree-sitter-python` dropped. AST *storage* (`AstParseResult`, `AstNodeData`, `AstEdgeData`, the regex parser) is unchanged. |
| Platform model | `Platform.rust`/`PlatformRust`, `Platform.hal`/`PlatformHal`, `cargo_config()`, `meson_cross_file()`, `CrossSpec`, and the graph keys `rust_target`/`rust_idf_target`/`rust_flash`/`hal_crate`/`hal_version`/`hal_features` removed from `src/platform.rs` and `src/actors/platform_codec.rs`. `hal_add_platform` no longer writes a Meson cross file. |
| Swift mirror | `Models/Platform.swift`'s `rust`/`hal` fields and both structs; `ConfigurationViewerView`'s HAL and Rust-toolchain cards; `ActorAssignment.boardJSON` no longer writes `hal`. |
| Swift UI | `ProjectAnalysisView`'s HAL/Meson mode (`isHalProject`, `isSpireAppProject`, the platform tree, the add-platform sheet) and `Models/ProjectLayout.swift`; `SyntaxHighlighter` is C/C++ only. |

`ApplicationSpec.board.hal` is deliberately **kept**: it names the board
*abstraction* (`m5unified`), which is a C++ library, not a Rust crate.

Remaining after Phase 7 was the legacy HAL wizard — one feature, whose Rust
tools (`hal_add_target`, `hal_add_platform`, `hal_fill_*`, the HAL viewer
endpoints) and Swift screens called each other, so it belonged to a pass that
retired it whole. **Phase 7b is that pass** (see below): the 22 `hal_*` tools,
`modify_contract`, and every Swift HAL screen are gone, while `board.hal` and the
`spire-core` `ProjectStructure::Hal` vocabulary stay.

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

   Phase 6 pins the dependency (`BSP_VERSIONS`), and the pin **resolves**:
   `idf.py set-target esp32s3` fetches `espressif/m5stack_core_s3 (4.1.0)` and
   its 18 transitive dependencies. Configuration then stops on a *different*
   issue: the library's vendored LVGL (an `EXTRA_COMPONENT_DIRS` component)
   overrides the managed `lvgl/lvgl`, so the BSP's optional decoder components
   (`freetype`, `libjpeg-turbo`, `libpng`, `lz4`) ask about `LV_USE_*` options
   whose Kconfig is no longer in the build, and the component manager reports
   `Missing required kconfig option after retry`. The BSP-less build below
   remains the end-to-end proof; reconciling the vendored LVGL with a BSP that
   pulls the managed one is the next step, not a scaffold defect.

## Phase 5 — tidy

- `README.md` describes the merged tree and its build/test entry points.
- `docs/*.md` now resolve under `crates/spire-esp32/`; the `rag_bundle`
  `file:line` anchors were re-pointed after `IDF_DOCS` (since retired in Phase 7d)
  moved `install_into`, and the "only corpus with shipped documents" claim then
  covered both `device-facts` and `spire-idf`.
- `.gitignore` covers the vendored library's build output, `sdkconfig` and the
  per-component host-test `build/` directories.
- Fresh history: four commits from the initial import to the Phase 4 gates;
  this file is the provenance record.

## Phase 6 — de-Rusting, the BSP pin, and IDF/CMake system flow

| Gate | Result |
| --- | --- |
| `cargo fmt --check` | clean |
| `cargo test --workspace` | 628 passed, 0 failed, 12 ignored (16 test binaries) |
| `swift test` (`ui/swift`) | 59/59 |
| host `ctest` in all 12 `idf/components/*/test` suites | 12/12 |
| `cargo test -p spire-esp32 --test system_flow_tests` | 9 passed, 1 ignored (live model) |
| BSP-less `PM25_METER` scaffold → `idf.py set-target esp32s3 build` | exit 0; `build/pm25-meter.bin` (190,416 bytes) |
| `PM25_METER` scaffold with the BSP → `idf.py set-target esp32s3` | pin resolves (`espressif/m5stack_core_s3 (4.1.0)` + 18 deps); configure stops on the vendored-LVGL/managed-LVGL conflict (Phase 4, finding 2) |
| device-facts margin (`cores3` vs `ft6336`) | 0.1338 vs 0.1124 — unchanged (re-measured) |

The system-flow suite (`tests/system_flow_tests.rs`) was ported from the
pre-fork tree. Its fixture is **CMake/Ninja** rather than Meson, and it covers
the tool registry, a real build-manager refusal, `modify_code` apply and
rollback against a real compiler, a declared device reaching the application
(and writing no phantom component for a board part), and the
`platforms/config` device facts. Phase 7b removed its `modify_contract` case
with the tool. The live `createProject → fill → build` run stays `#[ignore]`d
and reads its library from `SPIRE_LIVE_LIBRARY`, so it can point at the
vendored `idf/`.

The fixture uses **Ninja** deliberately: a flow writes a rewrite and rebuilds
in the same second, and Make's second-granularity timestamps then treat the
object as current — a deliberately broken file would look like a passing build.

The two Fix & Verify system tests are in the port as of Phase 7 (see below). The
deterministic one found a real defect in the stale-diagnostic sweep before it
could pass — which is the reason the coverage belonged with the artifact fix.

### The BSP pin

`main/idf_component.yml` now writes `m5stack_core_s3: "4.1.0"` instead of
`"*"` (`BSP_VERSIONS` in `src/build/idf_projects.rs`). A `published` component
and a BSP with no table entry still get `"*"`, so a newly added board still
scaffolds. The registry resolution is proven by the dependency list in Phase 4
finding 2; the configure failure there is the LVGL conflict, not the version.


## Phase 7 — the vendored-LVGL/BSP reconciliation, and the Meson-era sweep

| Gate | Result |
| --- | --- |
| `cargo fmt --check` | clean |
| `cargo check -p spire-esp32 --all-targets` | clean |
| `cargo test -p spire-esp32 --lib` | 483 passed, 0 failed, 7 ignored |
| `cargo test -p spire-esp32 --test system_flow_tests` | 10 passed, 0 failed, 2 ignored (live model) |
| `cargo test --workspace` | 632 passed, 0 failed, 13 ignored; exit 0 |
| `swift test` (`ui/swift`) | 59/59 |
| host `ctest` in all 12 `idf/components/*/test` suites | 12/12 |
| BSP-less `PM25_METER` → `idf.py set-target esp32s3 build` | not re-run here (no ESP-IDF on this machine); this phase does not touch that path |
| `PM25_METER` with the BSP → `idf.py set-target esp32s3` | not run here (no ESP-IDF on this machine); the change is configure-time and needs a real BSP build to prove |

### The LVGL conflict: the symbols a BSP tests for, not LVGL's whole menu

The Phase 4 finding was exact. The vendoring wrapper registers a component named
`lvgl` with **no Kconfig**, so the BSP's optional decoder components
(`freetype`, `libjpeg-turbo`, `libpng`, `lz4`) have rules on `CONFIG_LV_USE_*`
symbols that no loaded Kconfig defines, and the component manager stops with
`Missing required kconfig option after retry`.

`components/lvgl/Kconfig` now defines those symbols — the nine external
decoders LVGL itself knows (`LZ4`, `LIBPNG`, `LIBJPEG_TURBO`, `FREETYPE`,
`FFMPEG`, `LIBWEBP`, `RLOTTIE`, `THORVG`, `TINY_TTF`), all `n`, plus the two
`_INTERNAL` toggles (`LV_USE_LZ4_INTERNAL`, `LV_USE_THORVG_INTERNAL`) that
default `y`, exactly as upstream's do. It is
**deliberately not** `upstream/Kconfig`: that is a generated ~3,500-line menu,
and `upstream/` is a submodule that may not be checked out, so a Kconfig that
sourced it would fail the configure step on a clone that never ran
`tools/vendor-lvgl.sh` — the one outcome the wrapper exists to prevent. The
names, types and defaults mirror upstream, so a rule written against the
managed component resolves against this one.

`lv_conf.h` remains the authority for what is compiled. The Kconfig answers the
component manager; the header answers the compiler. The wrapper's `README.md`
and `SPIRE.md` state that division, and the `CMakeLists.txt` comments no longer
claim the wrapper has no Kconfig.

### The Meson-era sweep

- **Fix & Verify's artifact** is now discovered, not guessed:
  `autofix::built_artifact` reads the build directory a run used (`build/`, or
  `build-<platform>`) and returns the newest top-level `.bin` (falling back to
  `.elf`) — ESP-IDF's `<project>.bin`. A nested image (the bootloader) is never
  named, and a host CMake executable (no extension) is deliberately not named.
  The old code built `build-<platform>/<target>`, a path no CMake or IDF build
  writes.
- **The two unported system tests are ported** onto the CMake/Ninja fixture:
  `fix_and_verify_fixes_a_real_defect` and the `#[ignore]`d live-model
  `a_real_model_fixes_a_real_defect`.
- **The port found a real defect.** The stale-diagnostic sweep compared an
  absolute compiler path with the project root by string prefix. On macOS the
  compiler reports `/private/var/...` while the caller holds `/var/...`, so the
  old errors were never superseded: the rebuild succeeded, the graph still said
  three errors, and the loop reverted a correct fix as "3 → 3". The comparison
  now canonicalises both paths (and falls back to a boundary-aware lexical
  check), which also stops a sibling `/p/app-2` being mistaken for `/p/app`.
- `Platform::is_embedded` is ESP-IDF only; the retired `esp-hal`/`rp2040`
  spellings no longer offer a project type nothing builds.
- The plan prompt, the `ffi.rs` config default and project-root marker list, and
  the autofix comments no longer name Meson.

### The legacy HAL wizard retired (Phase 7b)

The HAL wizard was the last cross-language feature left over from the pre-fork
tree. It was removed whole rather than disabled, because its Swift half had
already lost its entry point — `SpireBridge.selectedDomain` was never assigned a
non-nil value, so `halCard`, `verifyHAL()`, `MigrateHalCard` and both HAL
portals were unreachable — while the Rust half still advertised tools that
either had no handler (`hal_sanity_check`, `hal_migrate_*`) or answered a caller
that no longer existed.

| Area | What went |
| --- | --- |
| Build tool surface | `hal_tool_definitions()` and its 22 `hal_*` definitions; `modify_contract`; the HAL arms of `call_tool`; `hal_generate_plan`/`_apply`, `verify_written_placeholders`, `cpp_compile_gate`, `cpp_gate_status`, `HalImplContext`, `hal_impl_generation_context`, `hal_llm_generate_source`, `CppStubArtifact`. The ESP-IDF definitions that shared the block (`idf_add_component`, `idf_remove_component`, `cpp_syntax_check`) moved to `component_tool_definitions()`. |
| Build helpers | `actors/hal_fill.rs`, `build/modify_contract.rs`, and the HAL half of `build/generic_helpers.rs` (contract parsing, placeholder generation, coverage, doc graph/lint, state/report/verify, prompts, meson wiring). `spire_framework_hints`, `cpp_syntax_check`, `compile_fix_prompt`, `strip_code_fences` and the parse/repair helpers stay. `verify_spine` went with its only consumer. |
| Project creation | `CreationStepType::WriteHalContract` / `WriteHalImplementation` and their executor, parser and description branches. |
| Coordinator | `propose_hal_fix` (`hal/fixPropose`), `handle_modify_contract` + `CoordinatorContractModify` (`modify-contract` and `tools/call` `modify_contract`). |
| AST | `AstParseResult.docs` (the `HalDoc` vector) — nothing else read it. |
| Swift | `HalDocReport.swift`, `HalGeneration.swift`, `HalMigrationPlan.swift`, `ModifyContractReport.swift`, `HALViewerPortal.swift`, `HALVerificationPortal.swift`; the HAL properties/refresh/methods on `SpireBridge`; the HAL card, fill/generate flows and contract gate in `MainView`; `HALLintStatusView`, `HALFixVerifyPanel`, `MigrateHalCard` and `FixWindows` in `ContextActionPane`; `HalFillItem`/`HalMethodSig`. `HalFixProposeResult` became `FixProposeResult` in `Models/FixProposeResult.swift`, because the ordinary `build/fixPropose` compile-fix loop still uses it. The orphaned wrappers went too: `SpireBridge.cppSyntaxCheck` and its `CppSyntaxReport`/`CppSyntaxError` types (only the deleted verification portal called them), and `MainView.findTreeDir`/`treeHasFile` (only the deleted HAL domain tree used them). |

Kept deliberately: `board.hal` (`m5unified`/`bsp`) in `ApplicationSpec`, the
`spire-core` `ProjectStructure::Hal` / `ProjectDomain` vocabulary (shared with
`spire-code`), the `esp-hal` fixtures and RAG corpus names (device facts), and
the `cpp_syntax_check` tool (a structural C++ check with no HAL involvement; it
stays on the Rust side, where `idf_add_component` also lives).

### Dead Swift UI removed after the HAL wizard (cleanup pass)

With the HAL entry points gone, a symbol sweep over the Swift package found a
second layer of code with no callers. None of it was HAL code — it was
pre-existing fork/upstream leftover — and it was removed with the compiler as
the guide:

| Area | What went |
| --- | --- |
| Layout | `MainView.middlePane` and the members only it used (`contextTitle`, `contextSubtitle`, `leftWidth`, `isDraggingBottom`, `dragHorizontal`, `dragGesture`); the unreachable `TargetScopedPanel` (~225 lines) and `Views/SubprojectDetailCard.swift` (489 lines, including `BuildDetailView`). The live body (analysis / source tree / dependencies) is unchanged. |
| Bridge old flows | `makeLLMService` and `Services/LLMService.swift` (settings go through `fetchLlmConfig`/`saveLlmConfig`); `planScaffold` and `PlanScaffoldResult`; `appendBuildEvent`, `startBuildEventConsumer`, `stopBuildEventConsumer` and `buildEventDrainTask` (the live consumer is `BuildService`'s); `deviceRun`, `generateAppSpec`, `scaffoldProjectLegacy` and its only helper `writeFile`, `fetchTools` and `mcpTools`/`mcpToolsLoading`, `specDesignState`, `fetchDependencyDocs`, and `AppSpecSummary` (`Models/AppSpec.swift`). |
| Smaller orphans | `ProjectTree.buildProjectTree`/`buildNodes`/`insertSubproject`/`ProjectTreeNode` (the file keeps `badgeLabel`/`badgeColor`), `ContextActionPane.actionCard`, `ToolsOverviewView.coreServer`, `BuildService.dependencyDocs`, and the unused `str`/`err` bindings left by commented-out debug prints in `SpireBridge.readFile`/`ProjectService.readFile`. |

Gates after the cleanup pass: `swift build` complete with no warnings from the
removed surface (the remaining Swift warnings are pre-existing and unrelated),
and `swift test` 58/58.

Gates at Phase 7b:

| Gate | Result |
| --- | --- |
| `cargo check -p spire-esp32 --all-targets` | clean (no warnings from the removed surface) |
| `cargo test -p spire-esp32 --lib` | 431 passed, 7 ignored |
| `cargo test -p spire-esp32 --test actor_tests` | 62 passed, 1 ignored — includes `test_build_registry_drops_hal_tools_and_keeps_idf_tools` |
| `cargo test -p spire-esp32 --test system_flow_tests` | 8 passed, 2 ignored (the removed `modify_contract` case is gone) |
| `cargo test -p spire-esp32` (all targets) | exit 0 |
| `swift build --package-path ui/swift` | Build complete |
| `swift test --package-path ui/swift` | 58/58 (the removed `ModifyContractReport` decode test accounts for the one-test delta) |

### The non-ESP32 RAG corpora removed (Phase 7c)

The bundle still carried two generic Spire corpora — `spire-core` (the core library's
architecture, RAG and spatial docs) and `spire-actor` (the actor-model library's docs).
Neither describes anything specific to ESP32, and this application's knowledge store is
meant to hold only ESP32 knowledge, so both were removed from `BUNDLE`, their manifests
deleted from `resources/rag-ingest/`, and the already-installed `knowledge/spire-core`
and `knowledge/spire-actor` directories removed. The `spire-idf` corpus stays: it is the
component library an ESP-IDF application is built on.

The comments in `esp-idf-lib.ingest.yaml`, `esp-dl.ingest.yaml` and `spire-idf.ingest.yaml`
that pointed at the deleted `spire-core` manifest for the `**/`-prefix rule now state the
rule directly, and the Swift install hint and the architecture doc name only `spire-idf`.

Gates at Phase 7c:

| Gate | Result |
| --- | --- |
| `cargo test -p spire-esp32` (all targets) | 16 suites, 578 passed, 0 failed, 13 ignored |
| `cargo test -p spire-esp32 --test rag_bundle_manifests` | 7 passed |
| `swift build --package-path ui/swift` | Build complete |
| `./build/assemble-app.sh` | `build/Spire.app` rebuilt |
| `install_rag_bundles` | 9 corpora installed; no `spire-core`/`spire-actor` in `~/.spire/spire-esp32/knowledge` |

### The component library as a first-class project, and a live `spire-idf` corpus (Phase 7d)

The embedded `idf/` tree was a library an application was built against, but not a project the app
could open: its components were reachable only through a dependent. It is now the first-class
**Component Library** — a `shippingbox` icon in the icon bar that calls `project/open` on it, the same
route an application opens by, so its components become subprojects and its build/test/lint/modify
flows are available without scaffolding anything. A missing tree is reported in an alert rather than
silently creating an empty directory (`project/open` creates what it is given).

The path is resolved once, in `library::library_root()`: `$SPIRE_LIBRARY_ROOT` when set, else
`<repo>/idf/` anchored on `CARGO_MANIFEST_DIR` (not a hard-coded home directory), canonicalised when it
exists. Three surfaces consume it, and they can no longer disagree:

- the host's icon, through the new FFI entry point `spire_library_root()` and
  `SpireCorePaths.libraryRoot`;
- `CreateProjectSheet.defaultLibrary`, which now asks the core instead of spelling
  `~/naturesense/...`;
- `rag_bundle::install_into`, which writes the `spire-idf` manifest with the resolved root substituted
  for `__SPIRE_LIBRARY_ROOT__`.

The `spire-idf` corpus no longer ships four copied documents. It globs the live tree
(`**/SPIRE.md`, `**/README.md`, `**/API.md`) and excludes the vendored LVGL checkout, build output, the
build harness, host tooling and component tests. `IDF_DOCS` is gone; a document added to a component is
in the next ingest with no list to update. The installer also removes the superseded
`<store>/spire-idf/docs/` copy, so a store filled by an older binary converges on re-install. The corpus
still tracks the tree at **ingest** time, not at every save — automatic re-embedding on file change
remains a deliberate, separate step.

Gates at Phase 7d:

| Gate | Result |
| --- | --- |
| `cargo check -p spire-esp32 --all-targets` | clean |
| `cargo test -p spire-esp32 --lib` | 431 passed, 7 ignored |
| `cargo test -p spire-esp32 --test rag_bundle_manifests` | 8 passed |
| `swift build --package-path ui/swift` | Build complete |
| `swift test --package-path ui/swift` | 58/58 |
| `cargo run -p spire-esp32 --example install_rag_bundles` | 9 corpora installed; `spire-idf/ingest.yaml` names `/Users/steve/naturesense/spire/spire-esp32/idf`, and the legacy `spire-idf/docs/` is gone |
| `nm -gU target/release/libspire_esp32.dylib` | exports `_spire_library_root` |

### A component's contract in the graph, and a test that can fail (Phase 8)

The edit flow had a gap where the contract should be. `idf_component_edit` was given a component's
header, its `SPIRE.md` and one sentence of fill prose, and nothing stated what the component *promised*
— which capabilities it provides, what device it drives, or which document its protocol came from. Two
failures ran out of that gap, and neither is visible by reading the code that results:

- **A spec that cited a neighbour.** `device-facts` is keyed by part, but the retrieval ranked the whole
  corpus by description: a live batch answered `axp2101` with `bmi270.md` and `ili9342` with
  `gt911.md`, and both were recorded as the component's own protocol. A citation is the kind of value
  nothing downstream re-derives — every later edit stands on it.
- **A test that agreed with its code.** `sps30`'s first test had been written from the same reading of
  the protocol as the implementation, so the Sensirion CRC seed was wrong in both and every case
  passed. Reading the test again cannot find that; only something the source did not produce can.

**The spec.** `subsystems/project/component_spec.rs` decomposes a component into five typed nodes —
anchor (name, kind, fill prose, invariants), device (part, bus, address), roles (capability paths),
facts (corpus documents) and the design record — and is pure: `reconstruct(decompose(spec)) == spec` is
a test, not a convention. `ComponentSpec::validate` refuses what the rest of the system cannot honour: a
role the capability vocabulary does not know, a device on a library, a blank name/part/bus, a fact that
names no document, and a `device-facts` fact that is **not this part's own**. `merge_facts` unions by
corpus and document, so an edit can add a precedent without losing the protocol.

**The store.** `component_persist.rs` writes that decomposition the way an application's composition is
written — one `AttrNode` per node, `Custom(<predicate>)` edges, scoped `{library}::{component}` because a
component's name is only unique within its library — and a store **replaces**: children the spec no
longer states are pruned, so a removed role cannot come back as a stale `role.2`. The rendered
`SPIRE.component.yaml` sits on the anchor as a copy; the graph is what a flow reads. `decision.rs` now
holds the versioned decision both spec kinds share (source, version, fingerprint, freshness), and
`graph_io.rs` is the one codec for the property envelope and scoped names — `spec_persist`,
`composition_persist` and `edit_history` had three copies of it.

**The doors.** `project/componentSpec` answers a component from the graph, or from the tree when the graph
has never seen it (marked implied, and undecided — looking is not a leg that writes, so a read never
backfills). The first **edit** of a component that predates the graph backfills one from what its own
`CMakeLists.txt` states, recorded as a stub nobody has decided; an edit merges the facts it retrieved and
records the decision when the gate passes, so a spec is ratified by the run that kept it; and
`idf_add_component` validates the spec **before** it writes the skeleton, because refusing a contract
after the files exist is refusing too late.

**The retrieval.** The ranking is no longer trusted with the key: `component_device_lookups` asks for the
whole corpus, `promote_part_document` puts the document named for the part in front, and only a document
that passes `document_is_the_parts_own` is recorded as a fact. `store_component_spec` runs `validate` on
every record it writes — backfill, merge or hand edit — so the same rule holds at the store.

**The test as an oracle, not an echo.** Both edit requests now say where an expectation comes from. A
driver's expected frames are *transcribed* — command word, register, checksum byte, decoded value — from
the part's own protocol section or the user's words, never computed from the code being written, with the
self-check that changing the command word, CRC seed or framing must not leave the test green. A library's
expected values are worked out by hand from the definition the user gave, with the same check on a
constant, an index or a sign. And the host build is now `cmake --build --clean-first`: make compares
timestamps at whole-second resolution, so an edit applied in the same second as the baseline build was
being verified by the **previous** object file.

**The evidence.** Three suites, each able to fail:

- `component_mutation_tests` — copies a component to a temp tree, asserts the control (the unmutated copy
  builds and its test passes), then plants compiling bugs one at a time: a checksum seed, a command word,
  a decoded byte, a settling delay. A mutant that survives is a test and a source that still agree about
  something they both got from one place; a build failure counts as a survivor, not as a catch.
- `component_rollback_tests` — the real modify spine and the real gate against `sps30`, with a planted
  CRC seed that compiles and is caught by the test; asserts `files_reverted` **and** all three files
  byte-identical afterwards. `CodeDriver::revert` now reads its own restore back, because a batch run had
  reported a revert whose bytes were still on disk.
- `component_spec_tests` — every shipped `SPIRE.component.yaml` resolves to its own part: name matches its
  directory, kind agrees with the component's `CMakeLists.txt`, a device only on a driver, and each
  `device-facts` source is this part's own and shipped in `FACTS_DOCS`. A planted set (neighbour document,
  unshipped document, no document, library with a device, wrong kind, blank fact) proves the check refuses,
  beside an accepted control.

**What ships.** Retrospective specs for all thirteen components, including the three framework ones;
`device-facts` documents for `axp2101`, `bm8563`, `bmi270`, `ili9342` and `st7789`, which had none; and
two harnesses — `backfill_component_specs`, which writes the specs from what the tree already says, and
`regenerate_component`, which is the audit: an **empty** instruction, the app's own coordinator/RAG/LLM/gate,
ten components in one batch, a tar backup of `components/` on top of the per-run rollback, and a non-zero
exit iff any spec failed to carry its component.

**The pane.** A selected component now opens in the right pane (`ComponentDetailView`) instead of a sheet:
write, history and remove hang off one box that starts from the spec's own words, the navigator
(`ProjectNavigator`) is one tree rather than two overlapping ones, and a framework component offers none of
the three. `ComponentSpec.swift` decodes leniently — a pane that refuses to open is worse than one with a
gap — and requires only `spec`.

**The audit.** `regenerate_component -- all` is the phase's live gate: each of the ten editable
components goes through the app's own `idf_component_edit` with an **empty** instruction, so the only
thing asked for is the spec — description, invariants, facts, header, `SPIRE.md` — retrieved by the RAG
actor, written by the LLM actor, and kept only if the component's own host test passes. All ten passed,
twice. Because this is the edit door, each file is shown to the model as it rewrites it, so the audit
measures **agreement with the spec** rather than reconstruction from it: on the second batch five came
back **up to date** — every file read against its spec and returned unchanged — and four of the five
rewrites differed from the file they replaced by nothing but whitespace. The fifth, `st7789`, gained an
assertion that the panel's tuning parameters go out in order — transcribed from the datasheet, not read
back from the driver — which is exactly the oracle this phase asked for. The audit also caught the last
harness defect: every file it rewrote came back one byte shorter — its final newline — because the fence
stripper trims the block. `strip_code_block_as_file` now puts exactly one back, with a test, so a
rewrite that changes one line reads as a one-line diff.

Gates at Phase 8:

| Gate | Result |
| --- | --- |
| `cargo check -p spire-esp32 --all-targets` | clean |
| `cargo test -p spire-esp32` | 16 suites, 631 passed, 0 failed, 13 ignored |
| `cargo test -p spire-esp32 --lib` | 477 passed, 7 ignored |
| `cargo test -p spire-esp32 --test component_mutation_tests` | 1 passed — control green, five planted bugs caught |
| `cargo test -p spire-esp32 --test component_rollback_tests` | 1 passed — `files_reverted`, bytes identical |
| `cargo test -p spire-esp32 --test component_spec_tests` | 2 passed — thirteen specs resolve, six planted records refused |
| `cargo run -p spire-esp32 --example regenerate_component -- all` | 10/10 verified on both batches; second batch 5 up to date, no content drift but `st7789`'s new init-data assertion |
| `cargo fmt --check` | clean |
| `swift build --package-path ui/swift` | Build complete |
| `swift test --package-path ui/swift` | 58/58 |

