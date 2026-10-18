# spire-esp32 monorepo — Rust core (crates/spire-esp32) + Swift UI (ui/swift).
#
# Targets:
#   make rust      — build the Rust crate (libspire_esp32.dylib + spire-esp32 bin)
#   make swift     — build the Swift UI executable
#   make app       — build everything + assemble build/Spire.app (double-clickable)
#   make run       — assemble + launch the app
#   make run-idf   — assemble + launch it **with an exported ESP-IDF environment** — the app resolves one
#                    itself when it has none, so this is for *choosing* one
#                    (`build/run-with-idf.sh --check` proves the environment;
#                    `build/run-with-idf.sh -- <command>` runs a command in it)
#   make test      — run Rust + Swift tests (formatting is checked first)
#   make fmt       — format this repo's Rust crate
#   make fmt-check — fail if anything in this repo is unformatted
#   make clean     — remove build artifacts
#
# spire-core sits next to this repo as a path dependency (see Cargo.toml). It is
# a separate repository and is deliberately NOT formatted or checked from here.

.PHONY: rust swift app run run-idf test fmt fmt-check clean

rust:
	cargo build --release -p spire-esp32

swift:
	cd ui/swift && swift build

app:
	@./build/assemble-app.sh

run: app
	@open ./build/Spire.app

# The same app, launched with an ESP-IDF environment resolved *first*, so that a chip build inside it runs
# in an environment a person chose rather than the one the app resolves for itself when it has none. This
# execs the binary instead of `open`, so the app inherits the environment.
run-idf: app
	@./build/run-with-idf.sh

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

test: fmt-check
	cargo test -p spire-esp32
	cd ui/swift && swift test

clean:
	cargo clean
	rm -rf build/Spire.app
	cd ui/swift && swift package clean || true
