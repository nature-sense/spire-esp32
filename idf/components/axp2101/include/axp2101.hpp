// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <axp2101_bus.hpp>

#include <cstdint>

namespace axp2101 {

/// An **AXP2101** power-management IC — the PMIC that runs a battery board, over I²C.
///
/// It is the part that stands between the cell, the charger and the rails: it decides what is powered,
/// it charges the battery when the cable is in, and it *measures* — battery voltage, the charge it is
/// putting back, the battery's own gauge. This driver carries the part's **measurements and states**,
/// and the state machine that makes them mean something is the product's.
///
/// What is fixed here is the shape, and the shape is worth keeping:
///
///  * it owns its **bus handle** and takes no board facts — the bus, the address and the pins are the
///    *board's*, supplied by the application. It takes the **handle**, not the address: opening the
///    bus and adding the device at its address is the application's job, and this constructor is handed
///    what comes back from that. A call site passing a bare `0x34` where a handle is wanted does not
///    compile, which is the compiler saying this sentence again;
///  * it names **no actor, no task and no framework type** — a protocol is a library, and what drives
///    it is the product's business;
///  * it reaches the bus through `bus_write` / `bus_read` / `bus_write_read` and nothing else, so it is
///    testable on a host (`test/`) as well as on the chip;
///  * it reports failure by **returning false**, never by logging, so the caller can tell a startup
///    refusal from a reading worth retrying.
///
/// It **reads** and it does not **configure**: bring-up — which rails come up at what voltage, the
/// charge current, the watchdog — is the board's, and a driver that silently rearranged a board's
/// power tree would be one no board could adopt. So there is nothing here that turns a rail on.
///
/// What it is **not**: a battery model. It reports the voltage and the gauge the part measures; whether
/// that is *healthy*, *nearly flat*, or *enough to sleep* is arithmetic and a decision, and both belong
/// above this layer, where the policy lives.
class Axp2101 {
public:
    explicit Axp2101(BusHandle device);

    Axp2101(const Axp2101&) = delete;
    Axp2101& operator=(const Axp2101&) = delete;

    /// Whether this really is an AXP2101: reads its ID register and checks the value.
    ///
    /// Unlike the sensor and clock drivers beside this one, this part **identifies itself** — register
    /// `0x03` carries a type value that is the AXP2101's and no other's — so this reports **identity**,
    /// not mere presence: a different XPowers part answering at the address reads false rather than
    /// being mistaken for this one.
    bool probe();

    /// Whether a battery is connected, written to `connected`.
    ///
    /// Returns **false** only when the exchange failed — a battery that is absent is a successful
    /// reading of *no battery*, not a failure, and the two read differently on purpose: a caller
    /// deciding whether to run on the cell wants the answer, not an error.
    bool battery_connected(bool& connected);

    /// Whether the part is charging the battery, written to `charging`.
    ///
    /// False (`charging == false`) covers everything the part reports instead — standing by with the
    /// cable in, and discharging with the cable out — because they call for the same response from a
    /// thin client: wait, or run on the cell.
    bool charging(bool& charging);

    /// The battery voltage, in millivolts, written to `millivolts`.
    ///
    /// The part's ADC measures it in millivolts, so there is no scale to apply and no unit to guess:
    /// the number is what the part reports. With no battery fitted the measurement is meaningless —
    /// the caller checks `battery_connected()` first rather than the driver inventing a value.
    bool battery_voltage_mv(std::uint16_t& millivolts);

    /// The battery's own gauge, in percent, written to `percent`.
    ///
    /// The part computes this itself and hands it over as a percentage; the driver neither smooths nor
    /// corrects it. It is only meaningful while a battery is fitted and the gauge has learned one, which
    /// is why it is a reading beside the voltage rather than a derived *state of charge*.
    bool battery_percent(int& percent);

private:
    BusHandle device_;
};

}  // namespace axp2101
