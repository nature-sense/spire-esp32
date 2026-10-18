// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <ft6336_bus.hpp>

#include <cstdint>

namespace ft6336 {

/// One report from the panel: where a finger is, and whether there is one at all.
///
/// The coordinates are the **controller's** — the panel's own grid, the same on every board that fits
/// this module — not the display's. Mapping a point onto a widget is a different job, done above this
/// layer; a touch driver that knew about screens would be one that could not be tested without one.
struct Touch {
    std::uint16_t x = 0;
    std::uint16_t y = 0;
    bool down = false;  ///< whether a finger is on the panel. `x`/`y` mean nothing when this is false.
};

/// A FocalTech **FT6336** capacitive touch controller — the FT6x36 family's part, as fitted to the
/// CoreS3's panel — read register by register over I²C.
///
/// It reports **one point**: whether a finger is down and, when it is, its coordinate. It recognises
/// no gestures on its own — a tap, a swipe or a long press is a shape in a *stream* of these reports,
/// and the timing that tells one from another lives above this layer, where a clock does.
///
/// What is fixed here is the shape, and the shape is worth keeping:
///
///  * it owns its **bus handle** and takes no board facts — the bus, the address, the interrupt line
///    and the reset line are the *board's*, supplied by the application. It takes the **handle**, not
///    the address: adding the device at its address is the application's job;
///  * it holds the **panel's** facts — its grid — because those travel with the module;
///  * it names **no actor, no task and no framework type** — polling the controller, or waking on its
///    interrupt, is the product's business;
///  * it reaches the controller through `bus_write` / `bus_read` / `bus_write_read` and nothing else,
///    so it is testable on a host (`test/`) as well as on the chip;
///  * it reports failure by **returning false**, never by logging. A read that finds *no finger* is not
///    a failure — `read()` returns true with `Touch::down` false — and the two cases must stay apart.
class Ft6336 {
public:
    /// The module's grid, in the panel's own units — the same module on every board.
    ///
    /// `read()` reports the controller's own coordinate, which comes out of twelve-bit registers
    /// (`0x000`–`0xFFF`); these are the panel dimensions that grid is laid over, so a caller that needs
    /// panel units scales `x * kWidth / 4096`. The driver does not scale for you: a full-scale is an
    /// assumption about the panel, and a driver that assumed one could only be checked against it.
    static constexpr std::uint16_t kWidth = 240;
    static constexpr std::uint16_t kHeight = 320;

    explicit Ft6336(BusHandle device);

    Ft6336(const Ft6336&) = delete;
    Ft6336& operator=(const Ft6336&) = delete;

    /// Whether the controller is really there: reads its chip id and compares it with the FT6336U's.
    ///
    /// A probe that returned true because a bus handle was passed would report success on a board where
    /// the controller is not fitted. A bus that will not answer reads **false** too — a part that will
    /// not speak is no more usable than one that is absent.
    bool probe();

    /// Reads the controller's touch registers and fills `touch` — the first point, when a finger is
    /// down.
    ///
    /// Returns **false** only when an exchange failed; returns **true** with `Touch::down == false`
    /// when the panel was read successfully and simply has no finger on it. Collapsing those two into
    /// one false is how a dropped I²C transaction becomes a stuck cursor.
    bool read(Touch& touch);

private:
    BusHandle device_;
};

}  // namespace ft6336
