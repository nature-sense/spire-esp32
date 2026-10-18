# spire-esp32

The ESP32/C++ development domain for Spire: a Rust core that plans, scaffolds
and fills ESP-IDF projects, a SwiftUI app that drives it, and the component
library those projects are built against.

This repository was forked from `spire-code` with the `spire-idf` library folded
in. [PROVENANCE.md](PROVENANCE.md) records the exact source commits, what was
copied, what was removed, and the acceptance gates run in this tree.

## Layout

| Path | What it is |
| --- | --- |
| `crates/spire-esp32/` | Rust core: the `idf.py` build module, project/component scaffolders, the application spec, RAG manifests, the coordinator and the C ABI |
| `idf/` | The ESP-IDF **component library** — `SPIRE.md` is its contract: the framework (`actors`, `ramen`, `toolkit`), the drivers, `lvgl_display`, and vendored LVGL |
| `ui/swift/` | The macOS SwiftUI app; it loads the core as `libspire_esp32.dylib` |
| `build/` | App assembly and ESP-IDF environment helpers |
| `docs/`, `schema/`, `tools/` | Architecture notes, the board-capability vocabulary, helper scripts |

## Prerequisites

- Rust (stable) and Cargo
- Xcode / Swift 5.10+
- ESP-IDF **v5.5.5** at `~/.espressif/esp-idf/v5.5.5` (override with `IDF_DIR`)

## Build and test

```sh
make test          # rustfmt check + Rust tests + Swift tests
make app           # release dylib + Swift app → build/Spire.app
make run           # assemble and launch
make run-idf       # launch with an exported ESP-IDF environment
```

The library builds on its own, and that is the fastest proof the vendored tree is
complete:

```sh
cd idf
./tools/idf.sh set-target esp32s3 build          # firmware image
./tools/vendor-lvgl.sh --status                  # 9.6.0

# host tests for one component
(cd components/actors/test && cmake -S . -B build && cmake --build build && ctest --test-dir build)
```

## How a project is made

1. **Create.** The app scaffolds an **ESP32 Components** library or an **ESP32
   Application**. An application names the component library it builds against;
   the default is this repository's `idf/`.
2. **Design and review.** The design is written as a composition
   (`composition.spire`), reviewed as what it will become, and only then filled
   into C++ components.
3. **Build.** The application is built with `idf.py build` through the core's
   build module, which resolves an ESP-IDF environment itself when the process
   has none.

`idf/SPIRE.md` is the library's own statement of how a component is composed and
what a build requires. It reaches the model two ways: verbatim as *library hints*
when an application names the library, and semantically through the `spire-idf`
RAG corpus, whose documents ship with the binary.

## License

GPL-3.0-or-later. Vendored LVGL is MIT — see
`idf/components/lvgl/upstream/LICENCE.txt`.
