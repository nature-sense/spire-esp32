// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **Retrospective component-spec backfill** — give the components that predate the graph the spec
//! their own tree implies, and render each one's `SPIRE.component.yaml`.
//!
//! The graph is the source of truth and the YAML is its rendered copy, so a file written by hand
//! would be decorative: the first edit would overwrite it from a graph that still held only a name
//! and a kind. This example therefore goes through the same three calls the app's own door uses —
//! `store_component_spec`, `decide_component_spec` and `write_component_spec_file` — so the graph
//! and the file say the same thing afterwards.
//!
//! Everything a row states is read off the tree itself: the kind from the component's
//! `CMakeLists.txt` (re-checked with `check_kind`), the bus from its bus header, the roles from
//! `schema/capabilities.yaml`, the description from the header's own module doc, and the facts from
//! the `device-facts` corpus. The one exception is `st7789`, whose header still carries the scaffold's
//! fill prompt: its description is taken from the part's facts document, which says the same thing as
//! the constants in that header — a spec with no prose asks a model to write from invariants alone.
//!
//! The device states a **part and a bus, never an address**: every driver's header says the bus,
//! the address and the pins are the board's — the constructor is handed a handle, and the address
//! belongs to the application's composition, not to the component library.
//!
//! # Running it
//!
//! **The Spire app must be quit first.** The library's graph lives at `idf/.spire/data` (a SeleneDB
//! snapshot plus WAL), and a running app would checkpoint its own in-memory copy over these writes.
//!
//! ```text
//! cargo run -p spire-esp32 --example backfill_component_specs
//! ```
//!
//! `SPIRE_IDF_ROOT` overrides the project root when it is not the repository's own `idf/`.
//!
//! Re-running is safe: an unchanged spec re-ratifies its record instead of bumping the version, and
//! the rendered file is rewritten from the same graph.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use spire_core::actors::{Actor, MemoryGraphActor, MemoryGraphMessage};
use spire_esp32::build::edit_history::library_name;
use spire_esp32::build::idf_projects::ComponentKind;
use spire_esp32::subsystems::project::component_persist::{
    check_kind, decide_component_spec, load_component_spec, store_component_spec,
    write_component_spec_file,
};
use spire_esp32::subsystems::project::component_spec::{
    ComponentDevice, ComponentSource, ComponentSpec, FactRef,
};
use tokio::sync::{mpsc, oneshot};

/// The corpus the `facts` references point into.
const DEVICE_FACTS: &str = "device-facts";

/// One component, as the tree states it.
struct Row {
    /// The directory name and the identifier the spec keys on.
    name: &'static str,
    kind: ComponentKind,
    /// The header's own statement of what the part is, lifted verbatim (minus its emphasis).
    description: &'static str,
    /// `Some` only for a driver: the bus its own `_bus.hpp` includes.
    bus: Option<&'static str>,
    /// The capability paths its job names, from `schema/capabilities.yaml`.
    provides: &'static [&'static str],
    /// The properties that must hold regardless of how it is written (the spec's `invariants`).
    invariants: &'static [&'static str],
    /// The `device-facts` document its code is written from, when there is one.
    fact: Option<&'static str>,
}

const ROWS: &[Row] = &[
    // ── drivers ───────────────────────────────────────────────────────────────────────────────
    Row {
        name: "axp2101",
        kind: ComponentKind::Driver,
        description: "An AXP2101 power-management IC: the rails, the charging and the battery gauge, over I2C.",
        bus: Some("i2c"),
        provides: &["power.pmu"],
        fact: Some("axp2101.md"),
        invariants: &["it owns its bus handle and takes no board facts — pins, ports and the address are the application's", "it reaches the bus through bus_write / bus_read / bus_write_read and nothing else", "it reports failure by returning false, never by logging", "it reads only: the power tree — what gets written — is the board's"],
    },
    Row {
        name: "bm8563",
        kind: ComponentKind::Driver,
        description: "A BM8563 real-time clock: a battery-backed wall clock, over I2C.",
        bus: Some("i2c"),
        provides: &["timekeeping.rtc"],
        fact: Some("bm8563.md"),
        invariants: &["it owns its bus handle and takes no board facts — pins, ports and the address are the application's", "it reaches the bus through bus_write / bus_read / bus_write_read and nothing else", "it reports failure by returning false, never by logging", "a time that cannot be trusted is not a time: the voltage-low flag makes now() return false", "the clock is read and written as one seven-register frame, not register by register"],
    },
    Row {
        name: "bmi270",
        kind: ComponentKind::Driver,
        description: "A BMI270 inertial measurement unit: three axes of acceleration, three of rotation and its own temperature, over I2C.",
        bus: Some("i2c"),
        provides: &["sensing.imu"],
        fact: Some("bmi270.md"),
        invariants: &["it owns its bus handle and takes no board facts — pins, ports and the address are the application's", "it reaches the bus through bus_write / bus_read / bus_write_read and nothing else", "it reports failure by returning false, never by logging", "it reads the range registers rather than assuming the reset default; an unknown range is a failure", "the counts are signed, two's-complement words"],
    },
    Row {
        name: "ft6336",
        kind: ComponentKind::Driver,
        description: "A FocalTech FT6336 capacitive touch controller: one touch point, read register by register over I2C.",
        bus: Some("i2c"),
        provides: &["media.touch"],
        fact: Some("ft6336.md"),
        invariants: &["it owns its bus handle and takes no board facts — pins, ports and the address are the application's", "it reaches the bus through bus_write / bus_read / bus_write_read and nothing else", "it reports failure by returning false, never by logging", "status 0 is a successful read with no finger, not a failure", "a coordinate is twelve bits, high nibble first"],
    },
    Row {
        name: "ili9342",
        kind: ComponentKind::Driver,
        description: "An ILI9342 panel controller: the CoreS3's 320x240 IPS, driven over SPI command by command.",
        bus: Some("spi"),
        provides: &["media.display"],
        fact: Some("ili9342.md"),
        invariants: &["it owns its bus handle and takes no board facts — the reset line, the data/command line, the backlight and the clock are the board's", "it reaches the panel through bus_command / bus_data / bus_delay_ms and nothing else", "it reports failure by returning false, never by logging", "the panel's facts — geometry, revision, colour order, the frame it is left in — live in the header"],
    },
    Row {
        name: "sht20",
        kind: ComponentKind::Driver,
        description: "A Sensirion SHT20 humidity-and-temperature sensor, read by command over I2C.",
        bus: Some("i2c"),
        provides: &["sensing.temperature", "sensing.humidity"],
        fact: Some("sht20.md"),
        invariants: &["it owns its bus handle and takes no board facts — pins, ports and the address are the application's", "it reaches the bus through bus_write / bus_read / bus_write_read and nothing else", "it reports failure by returning false, never by logging", "a reply whose CRC-8 disagrees is a failed reading, not a value", "a measurement is the sensor's hold-master mode — one bus_write_read (0xE3 / 0xE5) per reading, and nothing in this driver waits"],
    },
    Row {
        name: "sps30",
        kind: ComponentKind::Driver,
        description: "A Sensirion SPS30 particulate-matter sensor: PM1.0, PM2.5, PM4.0 and PM10, read by command over I2C.",
        bus: Some("i2c"),
        provides: &["sensing.particulate"],
        fact: Some("sps30.md"),
        invariants: &["it owns its bus handle and takes no board facts — pins, ports and the address are the application's", "it reaches the bus through bus_write / bus_read / bus_write_read and nothing else", "it reports failure by returning false, never by logging", "a sample is produced on the part's clock, not the caller's", "a reply whose CRC-8 disagrees is a failed reading, not a value"],
    },
    Row {
        name: "st7789",
        kind: ComponentKind::Driver,
        // The header's own module doc is still the scaffold's fill prompt, so this is taken from the
        // part's facts document rather than invented — and the header's `kWidth`/`kHeight` say the
        // same thing. A spec with no description asks the model to work from invariants alone.
        description: "An ST7789 panel controller: a 240x320 portrait IPS, driven over SPI command by command.",
        bus: Some("spi"),
        provides: &["media.display"],
        fact: Some("st7789.md"),
        invariants: &["it owns its bus handle and takes no board facts — the reset line, the data/command line, the backlight and the clock are the board's", "it reaches the panel through bus_command / bus_data / bus_delay_ms and nothing else", "it reports failure by returning false, never by logging", "the panel's facts — geometry and colour order — live in the header"],
    },
    // ── libraries ─────────────────────────────────────────────────────────────────────────────
    Row {
        name: "actors",
        kind: ComponentKind::Library,
        description: "A classical actor: a mailbox, a task of its own and on_message.",
        bus: None,
        provides: &[],
        fact: None,
        invariants: &[],
    },
    Row {
        name: "lvgl_display",
        kind: ComponentKind::Library,
        description: "The LVGL surface: one lv_display, the draw buffer LVGL renders into, and the flush callback that hands each changed rectangle to a pixel sink.",
        bus: None,
        provides: &[],
        fact: None,
        invariants: &[],
    },
    Row {
        name: "ramen",
        kind: ComponentKind::Library,
        description: "RAMEN: a compact single-header C++20 message-passing framework for hard real-time embedded systems.",
        bus: None,
        provides: &[],
        fact: None,
        invariants: &[],
    },
    Row {
        name: "rolling_average",
        kind: ComponentKind::Library,
        description: "A sliding-window average of float samples, over storage the caller owns.",
        bus: None,
        provides: &[],
        fact: None,
        invariants: &[],
    },
    Row {
        name: "toolkit",
        kind: ComponentKind::Library,
        description: "One loop on a FreeRTOS task of its own: the seam both application frameworks stand on.",
        bus: None,
        provides: &[],
        fact: None,
        invariants: &[],
    },
];

/// Turn a row into the spec the graph stores. A driver's `part` is its own name: every driver here
/// is looked up in `device-facts` by exactly the word its directory uses.
fn spec_from(row: &Row) -> ComponentSpec {
    let mut spec = ComponentSpec::new(row.name, row.kind);
    spec.description = row.description.to_string();
    spec.device = row.bus.map(|bus| ComponentDevice {
        part: row.name.to_string(),
        bus: bus.to_string(),
        address: None,
    });
    spec.provides = row.provides.iter().map(|path| path.to_string()).collect();
    spec.invariants = row.invariants.iter().map(|i| i.to_string()).collect();
    spec.facts = row
        .fact
        .map(|source| FactRef {
            corpus: DEVICE_FACTS.to_string(),
            source: source.to_string(),
        })
        .into_iter()
        .collect();
    spec
}

#[tokio::main]
async fn main() -> Result<()> {
    let idf = idf_root()?;
    let data_dir = idf.join(".spire").join("data");
    let library = library_name(&idf.to_string_lossy());

    println!("library  {library}  ({})", idf.display());
    println!("graph    {}", data_dir.display());
    println!("(the Spire app must not be running against this store)");
    println!();

    let (tx, rx) = mpsc::channel(64);
    let _graph = MemoryGraphActor::new().spawn(rx);
    let (reply, reply_rx) = oneshot::channel();
    tx.send(MemoryGraphMessage::Initialize {
        data_dir: data_dir.clone(),
        reply_to: reply,
    })
    .await
    .context("send Initialize to the graph")?;
    reply_rx
        .await
        .context("Initialize reply")?
        .context("the graph did not initialize")?;

    let mut failures = 0usize;
    for row in ROWS {
        match backfill(&tx, &idf, &library, row).await {
            Ok(line) => println!("{line}"),
            Err(e) => {
                failures += 1;
                eprintln!("  FAILED {:<15} {e}", row.name);
            }
        }
    }

    // Make the writes durable now rather than at the debounced checkpoint: a reader that starts
    // straight after this process exits must see them.
    let (reply, reply_rx) = oneshot::channel();
    tx.send(MemoryGraphMessage::Sync { reply_to: reply })
        .await
        .context("send Sync to the graph")?;
    reply_rx
        .await
        .context("Sync reply")?
        .context("the graph did not checkpoint")?;

    // Read every stored spec back: the rendered file is only honest if the graph it was rendered
    // from reconstructs to the same thing.
    let mut mismatches = 0usize;
    if failures == 0 {
        for row in ROWS {
            let expected = spec_from(row);
            match load_component_spec(&tx, &library, row.name).await {
                Ok(Some(stored)) if stored == expected => {}
                Ok(Some(stored)) => {
                    mismatches += 1;
                    eprintln!("  MISMATCH {:<15} {stored:?}", row.name);
                }
                Ok(None) => {
                    mismatches += 1;
                    eprintln!(
                        "  MISSING  {:<15} the graph has no spec after the write",
                        row.name
                    );
                }
                Err(e) => {
                    mismatches += 1;
                    eprintln!("  UNREADABLE {:<13} {e}", row.name);
                }
            }
        }
    }

    println!();
    if failures == 0 && mismatches == 0 {
        println!(
            "{} component specs stored, rendered and read back intact.",
            ROWS.len()
        );
        Ok(())
    } else {
        anyhow::bail!(
            "{failures} of {} components failed, {mismatches} did not read back",
            ROWS.len()
        )
    }
}

async fn backfill(
    mg_tx: &mpsc::Sender<MemoryGraphMessage>,
    idf: &Path,
    library: &str,
    row: &Row,
) -> Result<String> {
    let spec = spec_from(row);

    // The manifest is authoritative: a row that disagrees with the component's own CMakeLists.txt is
    // a mistake in this table, and `check_kind` names both sides of the disagreement.
    check_kind(idf, &spec).map_err(anyhow::Error::msg)?;

    let existed = load_component_spec(mg_tx, library, &spec.name)
        .await
        .map_err(anyhow::Error::msg)?
        .is_some();

    store_component_spec(mg_tx, library, &spec)
        .await
        .map_err(anyhow::Error::msg)?;
    let record = decide_component_spec(mg_tx, library, &spec, ComponentSource::Stated)
        .await
        .map_err(anyhow::Error::msg)?;
    let path = write_component_spec_file(idf, &spec.name, &spec).map_err(anyhow::Error::msg)?;

    Ok(format!(
        "  {:<15} {:<8} {:<8} v{}  {}",
        spec.name,
        spec.kind.as_str(),
        if existed { "updated" } else { "created" },
        record.decision.version,
        path.strip_prefix(idf).unwrap_or(&path).display()
    ))
}

/// The project root: `SPIRE_IDF_ROOT`, or the repository's own `idf/`.
fn idf_root() -> Result<PathBuf> {
    if let Ok(root) = std::env::var("SPIRE_IDF_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let idf = repo.join("idf");
    if !idf.join("components").is_dir() {
        anyhow::bail!(
            "{} has no components directory — set SPIRE_IDF_ROOT to the IDF project root",
            idf.display()
        );
    }
    Ok(idf)
}
