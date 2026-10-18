// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <st7789_bus.hpp>

#include <cstddef>
#include <cstdint>

namespace st7789 {

/// TODO: what this panel is, in the terms a caller thinks in — and what it is not.
///
/// **This is a stub, and the init sequence is the part no scaffold can write.** The command words, the
/// parameter each one carries, the delay between them, and the frame the panel is left in are the
/// panel's own; they come off its datasheet rather than out of a guess.
///
/// What is fixed here is the shape, and the shape is worth keeping:
///
///  * it owns its **bus handle** and takes no board facts — the reset line, the data/command line, the
///    backlight and the clock speed are the *board's*, and it is the application that opens the bus and
///    supplies them. It takes the **handle**, not the pins: constructing the SPI device is the
///    application's job, and this constructor is handed what comes back from that;
///  * it holds the **panel's** facts — geometry and colour order are properties of the ST7789, the same
///    on every board that fits one — and so live in this header rather than in a `board.hpp`;
///  * it names **no actor, no task and no framework type** — a display is a device, and which single
///    task is allowed to call it is the product's business;
///  * it reaches the panel through `bus_command` / `bus_data` and nothing else, so it is testable on a
///    host (`test/`) as well as on the chip;
///  * it reports failure by **returning false**, never by logging, so the caller can tell a startup
///    refusal from a screen worth trying again.
class St7789 {
public:
    /// The **panel's** dimensions, in pixels, portrait — the ST7789 is the same panel on every board.
    /// A board's rotation is the board's, applied by whoever draws, not a second panel.
    static constexpr std::uint16_t kWidth = 240;
    static constexpr std::uint16_t kHeight = 320;

    explicit St7789(BusHandle device);

    St7789(const St7789&) = delete;
    St7789& operator=(const St7789&) = delete;

    /// TODO: bring the panel up — the power-on sequence, the pixel format, the addressing window — and
    /// leave it in a state where `flush()` means something. Something that answers only if the panel is
    /// really there, rather than trusting that a bus handle exists.
    bool init();

    /// TODO: push one rectangle of pixels — `(x1 - x0 + 1) * (y1 - y0 + 1)` of them, **two bytes
    /// each**, big-endian RGB565, which is the panel's format and the reason this signature names a
    /// colour order rather than choosing one. The addressing window is the driver's to set; the caller
    /// hands pixels and a rectangle and nothing else.
    bool flush(std::uint16_t x0, std::uint16_t y0, std::uint16_t x1, std::uint16_t y1,
               const std::uint8_t* pixels);

private:
    BusHandle device_;
};

}  // namespace st7789
