// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <bmi270_bus.hpp>

#include <cstdint>

namespace bmi270 {

/// A **BMI270** inertial measurement unit — the six-axis part and its own temperature, over I²C.
///
/// It is the part that reports how the thing it is screwed to is *moving*: three axes of acceleration
/// and three of rotation, each a signed sixteen-bit count the part produces from its own MEMS elements.
/// This driver carries the part's **measurements**, and what they mean — a tilt, a tap, a step — is the
/// product's.
///
/// What is fixed here is the shape, and the shape is worth keeping:
///
///  * it owns its **bus handle** and takes no board facts — the bus, the address and the pins are the
///    *board's*, supplied by the application. It takes the **handle**, not the address: opening the
///    bus and adding the device at its address is the application's job, and this constructor is handed
///    what comes back from that. A call site passing a bare `0x69` where a handle is wanted does not
///    compile, which is the compiler saying this sentence again;
///  * it names **no actor, no task and no framework type** — a protocol is a library, and what drives
///    it is the product's business;
///  * it reaches the bus through `bus_write` / `bus_read` / `bus_write_read` and nothing else, so it is
///    testable on a host (`test/`) as well as on the chip;
///  * it reports failure by **returning false**, never by logging, so the caller can tell a startup
///    refusal from a reading worth retrying.
///
/// It **reads** and it does not **configure**. Bringing the part up — taking it out of suspend, its
/// power mode, the full-scale ranges, and the configuration file Bosch ships for it — is the board's,
/// and a driver that silently rearranged a board's part would be one no board could adopt. A part left
/// in its reset state still *answers*, so `probe()` tells you it is there and `read()` still returns
/// counts; what a part that was never brought up will not do is *change* them, because its sensors are
/// not running. That distinction is the caller's to make, and the driver does not paper over it.
///
/// What it is **not**: a motion model. It reports the counts the part produced; whether that is level,
/// shaking, or falling is arithmetic and a decision, and both belong above this layer, where the policy
/// lives.
class Bmi270 {
public:
    /// One reading of all six axes, in the units the datasheet's conversions give.
    ///
    /// Acceleration in **g** and rotation in **degrees per second**, each scaled by the full-scale range
    /// the part is actually set to — read from the part rather than assumed — so the numbers mean the
    /// same thing whichever range the board chose.
    struct Sample {
        float accel_x;
        float accel_y;
        float accel_z;
        float gyro_x;
        float gyro_y;
        float gyro_z;
    };

    explicit Bmi270(BusHandle device);

    Bmi270(const Bmi270&) = delete;
    Bmi270& operator=(const Bmi270&) = delete;

    /// Whether this really is a BMI270: reads its chip-ID register and checks the value.
    ///
    /// The part **identifies itself** — register `0x00` carries a value that is the BMI270's and no
    /// other's — so this reports **identity**, not mere presence: a different Bosch part answering at
    /// the address reads false rather than being mistaken for this one.
    bool probe();

    /// The six axes, written to `sample`.
    ///
    /// The accelerometer's and gyroscope's ranges are read from the part first, because the full scale
    /// is a *setting* — the board's, not the driver's — and the same count is a different number of g
    /// per range. A driver that assumed the reset default would report a plausible-looking value wrong
    /// by a factor of eight the moment a board chose a wider range, which is exactly the kind of bug
    /// that looks like a loose mounting screw on a bench. The counts themselves come out of the twelve
    /// consecutive data registers in a single exchange, so the six axes are one instant rather than six
    /// that could straddle a sample.
    ///
    /// Returns **false** when an exchange failed, and also when a range register holds a value this
    /// driver does not know: a reading it cannot scale is not a reading.
    bool read(Sample& sample);

    /// The part's own temperature, in degrees Celsius, written to `celsius`.
    ///
    /// This is the *die* temperature the part measures for itself — a fine check that the part is
    /// answering and warmed up, and **not** an ambient reading: the part warms itself, so the number
    /// runs above the room. What it is *for* is the caller's decision.
    bool temperature(float& celsius);

private:
    BusHandle device_;
};

}  // namespace bmi270
