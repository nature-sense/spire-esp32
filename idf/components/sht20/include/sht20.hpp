// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <sht20_bus.hpp>

#include <cstdint>

namespace sht20 {

/// A Sensirion **SHT20** humidity-and-temperature sensor — the SHT2x family's part, read by command
/// over I²C.
///
/// It reports two measurements, each triggered by its own command and answered by the sensor: a
/// **temperature** in degrees Celsius and a **relative humidity** in percent. It recognises nothing on
/// its own — no averaging, no derived value — and everything above this layer (a rolling mean, a dew
/// point, a decision) is arithmetic on those two numbers, done where the clock is.
///
/// What is fixed here is the shape, and the shape is worth keeping:
///
///  * it owns its **bus handle** and takes no board facts — the bus, the address and the pins are the
///    *board's*, supplied by the application. It takes the **handle**, not the address: opening the
///    bus and adding the device at its address is the application's job, and this constructor is
///    handed what comes back from that. A call site passing a bare `0x40` where a handle is wanted
///    does not compile, which is the compiler saying this sentence again;
///  * it names **no actor, no task and no framework type** — a protocol is a library, and what drives
///    it is the product's business;
///  * it reaches the bus through `bus_write` / `bus_read` / `bus_write_read` and nothing else, so it
///    is testable on a host (`test/`) as well as on the chip;
///  * it reports failure by **returning false**, never by logging, so the caller can tell a startup
///    refusal from a reading worth retrying.
///
/// The measurement is the sensor's own **hold-master** mode: the command and the reply are one
/// exchange, and the sensor stretches the clock while it measures — so there is nothing for this
/// driver to wait on, and no delay it must be trusted to get right. Every measurement reply is
/// **checksummed**, and a reply whose checksum does not match is reported as *no reading*, not as a
/// reading to average.
class Sht20 {
public:
    explicit Sht20(BusHandle device);

    Sht20(const Sht20&) = delete;
    Sht20& operator=(const Sht20&) = delete;

    /// Whether the sensor is really there: reads its user register and sees whether the device answers.
    ///
    /// The SHT2x family exposes **no chip-id register** — unlike a panel controller or a flash part,
    /// there is no value here that is this part's and no other's. So this reports **presence**, not
    /// identity: a device answered at the address. What proves the part is what it claims is a
    /// measurement, which is why `temperature()` and `humidity()` check the datasheet's own checksum.
    ///
    /// An absent part and a bus that will not answer both read **false**: a sensor that will not speak
    /// is no more usable than one that is not fitted.
    bool probe();

    /// Measures the temperature, in degrees Celsius, and writes it to `celsius`.
    ///
    /// Returns **false** when the exchange failed or the reply's checksum did not match — a reading
    /// that cannot be trusted is not a reading. The sensor's own hold-master timing is inside this
    /// call: it blocks for the measurement, so the value is complete when this returns.
    bool temperature(float& celsius);

    /// Measures the relative humidity, in percent, and writes it to `relative_humidity`.
    ///
    /// Returns **false** on the same terms as `temperature()`: a failed exchange, or a reply whose
    /// checksum did not match.
    bool humidity(float& relative_humidity);

private:
    BusHandle device_;
};

}  // namespace sht20
