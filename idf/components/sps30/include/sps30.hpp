// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <sps30_bus.hpp>

namespace sps30 {

/// A Sensirion **SPS30** particulate-matter sensor — the SPS generation's I²C part, read by command.
///
/// While it is measuring it reports a **sample** once a second: four mass concentrations (PM1.0,
/// PM2.5, PM4.0, PM10) in µg/m³, five particle-number concentrations in #/cm³, and a typical particle
/// size in µm. Unlike a one-shot sensor it is **stateful** — a sample is produced on *its* clock, not
/// on the caller's, so reading one is a loop: start measuring once, watch for a sample to be ready,
/// then take it. Everything above this layer (a rolling mean, an air-quality index, a decision) is
/// arithmetic on those numbers, done where the clock is.
///
/// What is fixed here is the shape, and the shape is worth keeping:
///
///  * it owns its **bus handle** and takes no board facts — the bus, the address and the pins are the
///    *board's*, supplied by the application. It takes the **handle**, not the address: opening the
///    bus and adding the device at its address is the application's job, and this constructor is
///    handed what comes back from that. A call site passing a bare `0x69` where a handle is wanted
///    does not compile, which is the compiler saying this sentence again;
///  * it names **no actor, no task and no framework type** — a protocol is a library, and what drives
///    it is the product's business;
///  * it reaches the bus through `bus_write` / `bus_read` / `bus_write_read` — plus the device's own
///    `bus_delay_ms` — and nothing else, so it is testable on a host (`test/`) as well as on the chip;
///  * it reports failure by **returning false**, never by logging, so the caller can tell a startup
///    refusal from a reading worth retrying.
///
/// A sample is a **single exchanged frame** of ten values, each a big-endian IEEE754 float carried as
/// two checksummed sixteen-bit words; the datasheet fixes their order. Every word is checksummed, and
/// a frame whose checksum does not match is reported as *no sample*, not as a sample to average. A
/// read is two exchanges rather than one (this device's own framing — see `sps30_bus.hpp`), and the
/// device needs a short settling interval after start and stop, which this driver waits out through
/// the seam rather than by sleeping on its own.
///
/// The **measurement loop** is a state machine, and it is deliberately the caller's to drive: this
/// driver does not hide it behind a single call, because *when* to sample is the product's decision —
/// a one-second poll, a burst on a trigger, a sample per display refresh — and a driver that chose for
/// the product would be one that cannot be asked a different question.
class Sps30 {
public:
    explicit Sps30(BusHandle device);

    Sps30(const Sps30&) = delete;
    Sps30& operator=(const Sps30&) = delete;

    /// One sample: the ten values the device reports, in the datasheet's order and units.
    struct Measurement {
        float mass_pm1 = 0.0f;               ///< Mass concentration, PM1.0   [µg/m³]
        float mass_pm2_5 = 0.0f;             ///< Mass concentration, PM2.5   [µg/m³]
        float mass_pm4 = 0.0f;               ///< Mass concentration, PM4.0   [µg/m³]
        float mass_pm10 = 0.0f;              ///< Mass concentration, PM10    [µg/m³]
        float number_pm0_5 = 0.0f;           ///< Number concentration, PM0.5 [#/cm³]
        float number_pm1 = 0.0f;             ///< Number concentration, PM1.0 [#/cm³]
        float number_pm2_5 = 0.0f;           ///< Number concentration, PM2.5 [#/cm³]
        float number_pm4 = 0.0f;             ///< Number concentration, PM4.0 [#/cm³]
        float number_pm10 = 0.0f;            ///< Number concentration, PM10  [#/cm³]
        float typical_particle_size = 0.0f;  ///< Typical particle size        [µm]
    };

    /// Whether the sensor is really there: reads its serial number and checks the reply's checksums.
    ///
    /// Unlike the SHT2x, this part **has** an identity — a per-unit serial number — so a valid reply is
    /// evidence that the device at the address is an SPS30 and not merely *something*. A silent bus, a
    /// short reply and a corrupt one all read **false**: a sensor that will not speak is no more usable
    /// than one that is not fitted.
    bool probe();

    /// Begin measuring, and wait out the device's own settling interval before returning.
    ///
    /// The device then produces a sample every second, and `data_ready()` reports when one is waiting.
    /// Returns **false** when the command was refused. Starting an already-measuring device is
    /// harmless — the command simply restarts the loop.
    bool start_measurement();

    /// Stop measuring, and wait out the device's own settling interval before returning.
    ///
    /// Returns **false** when the command was refused.
    bool stop_measurement();

    /// Whether a fresh sample is waiting to be read.
    ///
    /// A sample arrives once a second while the device is measuring, so a caller polls this and reads
    /// when it reports one. **False** means "nothing to read this time" — either the device has not
    /// produced its next sample yet or the bus would not answer — and the caller's response to both is
    /// the same: do not read, try again next tick. (A caller that must tell "not ready" from "gone"
    /// apart has `read()`, which fails only on a broken or untrustworthy exchange.)
    bool data_ready();

    /// Take the ready sample into `measurement`, its checksums verified.
    ///
    /// All ten values are decoded — the frame carries them together, with a checksum after each of its
    /// twenty words. Returns **false** when the exchange failed, when the reply was too short, or when
    /// any word's checksum did not match: a sample that cannot be trusted is not a sample. On a failed
    /// read the caller's `measurement` is left as it was, never half-written. Read only when
    /// `data_ready()` has said so; reading early returns the previous sample, which the device keeps
    /// until the next one replaces it.
    bool read(Measurement& measurement);

private:
    BusHandle device_;
};

}  // namespace sps30
