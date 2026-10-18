# spire-idf

An **ESP-IDF component library**: a project whose product is `components/*`.

It starts with a **framework** — three components that are already here — and then grows. The tool
that made this project has no opinion about what belongs *beyond* that: a driver, an algorithm, a
device — each is decided later, and one at a time. A **board** is not one of them: a board is a
composition, and a composition belongs to the application that names the board, never to the library.
`SPIRE.md` is where those decisions are written down.

## Layout

```
CMakeLists.txt        the idf.py project root, and `set(SPIRE_PROJECT_STRUCTURE idf_library)`
sdkconfig.defaults    what the project itself decides — nearly empty, deliberately
main/                 the BUILD HARNESS, not an application: it exists so `idf.py build` compiles
                      every component. It starts nothing and names no device.
components/           the product: the framework, and everything added to it
  toolkit/            spire::Task — the task seam both frameworks stand on
  ramen/              the dataflow framework (upstream Zubax/ramen, MIT) and its host test
  actors/             the classical actor framework: a scheduler, typed refs, on_message, and a name
                      registry (`*scheduler.registry()`) for a peer you were not handed a ref to
                      — and a host test that runs it, on real threads, against a fake FreeRTOS
SPIRE.md              HOW THIS LIBRARY IS MEANT TO BE USED — the architecture lives here
API.md                the function reference: every public type and function, component by component
```

## The framework is here; using it is optional

`ramen` and `actors` are two **application frameworks**, and they are alternatives rather than
layers: streaming work (a pipeline of frames, samples, readings) suits RAMEN, and a system of many
interacting agents suits mailboxes. They share one thing — `spire::Task` — and neither requires the
other. A library that adopts neither is a perfectly good library; what the framework removes is the
*gap*, so that "we need an idiom for this" is answered by what is already in the tree instead of by
an architecture invented mid-project.

## SPIRE.md is the point

An architecture — a framework, a lifecycle convention, a wiring idiom — is not something the tool
knows about. It is something a *library* provides and describes. When someone adds a component here,
or builds an application on this library, the model reads `SPIRE.md` and follows it.

So keep it true as the components arrive: what each one is, how they are composed, what belongs in a
component versus an application, and anything the build needs that a component cannot state itself
(a compiler flag, a config symbol). Nothing else in the tree carries that.

## Adding a component

A protocol component is generated as a **stub** and then filled in. The command framing, the
checksums, the byte order and the field order are the device's own; they come off its datasheet, and
none of them can be guessed.

A component is **framework-agnostic**: it holds a device or an algorithm, and it names no pins, no
board, no `Task` and no framework. The actor — the few lines that give a component ports or a mailbox
and own its loop — belongs to the application, because it names the product.

A component that has just **arrived** is a directory CMake has never seen. The component list is
resolved when CMake configures, so `idf.py build` on its own reports success — *without having
compiled it*. The build that adds a component is a re-configure:

```sh
tools/idf.sh reconfigure                # re-resolve components/ — a new one is invisible until this
tools/idf.sh set-target esp32s3 build   # compile-checks every component
```

ESP-IDF **v5.5.5** is installed at `~/.espressif/esp-idf/v5.5.5`, and no shell rc sources its
`export.sh` — so `idf.py` is not on `PATH` until something does. `tools/idf.sh` is that something:
it sources `export.sh` and then runs `idf.py` with the arguments it was given, so it works in a
non-login shell (a build run by a tool or a CI step) as well as in a terminal. In a shell you have
already activated with `. ~/.espressif/esp-idf/v5.5.5/export.sh`, `idf.py` works directly.

## What does not go here

An application. It is a separate project that depends on this one, and the first `app_main` that
lands in `main/` is the signal that a library has quietly become an application.
