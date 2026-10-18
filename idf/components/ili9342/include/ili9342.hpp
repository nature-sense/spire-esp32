// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <ili9342_bus.hpp>

#include <cstdint>

namespace ili9342 {

/// Which **revision** of the panel this is.
///
/// The same glass — the CoreS3's 320x240 IPS — ships with an ILI9342C or an ILI9342E behind it, and
/// the two want different power-on tables: the C is happy with the vendor's stock sequence, and the E
/// is not. That is a fact about the *panel*, so the sequence for each lives here, in this driver.
///
/// *Which* of them this board carries is a fact about the *board*, and it is the application's to
/// state: on the CoreS3 it is recorded nowhere but the touch controller's firmware-id register, which
/// is what the BSP reads to decide. So this driver is told, rather than guessing — an init that picked
/// a table by trying one and seeing is an init that cannot tell a wrong table from a dead panel.
enum class Revision {
    C,  ///< the vendor's stock sequence
    E,  ///< the vendor's sequence plus the E revision's extra tuning
};

/// A **ILI9342** panel controller — the CoreS3's own, driven over SPI, command by command.
///
/// It is a *device*, not a screen: it knows the panel's geometry, the frame the panel must be left in
/// and the addressing window a rectangle of pixels goes into. It does not know what is drawn, whose
/// buffer the pixels came from, or that anything above it exists.
///
/// What is fixed here is the shape, and the shape is worth keeping:
///
///  * it owns its **bus handle** and takes no board facts — the reset line, the data/command line, the
///    backlight, the panel-enable line and the clock speed are the *board's*, and it is the application
///    that opens the bus and supplies them. It takes the **handle**, not the pins: constructing the SPI
///    device is the application's job, and this constructor is handed what comes back from that;
///  * it holds the **panel's** facts — geometry, revision, colour order, the frame it is left in — and
///    so they live in this component rather than in a `board.hpp`. It does **not** scale or rotate:
///    a caller that wants landscape draws in landscape, and the driver takes the rectangle it is given;
///  * it names **no actor, no task and no framework type** — a display is a device, and which single
///    task is allowed to call it is the product's business;
///  * it reaches the panel through `bus_command` / `bus_data` / `bus_delay_ms` and nothing else, so it
///    is testable on a host (`test/`) as well as on the chip;
///  * it reports failure by **returning false**, never by logging, so the caller can tell a startup
///    refusal from a screen worth trying again.
class Ili9342 {
public:
    /// The **panel's** dimensions, in pixels, in the orientation the panel's own `MADCTL` leaves it:
    /// 320 wide, 240 tall — the same panel on every board. A board's rotation is the board's, applied
    /// by whoever draws, not a second panel.
    static constexpr std::uint16_t kWidth = 320;
    static constexpr std::uint16_t kHeight = 240;

    /// Wrap the bus handle a board has already opened. `revision` is the board's, and required — see
    /// [`Revision`].
    explicit Ili9342(BusHandle device, Revision revision);

    Ili9342(const Ili9342&) = delete;
    Ili9342& operator=(const Ili9342&) = delete;

    /// Bring the panel up — the power-on sequence, the pixel format, and the frame the panel is left
    /// in — so that `flush()` means something.
    ///
    /// Returns **false** the moment the bus refuses a transfer. A display's seam is write-only, so
    /// there is no id to read back and no honest way to distinguish "not fitted" from "not answering":
    /// both are reported the same way, and the caller decides what an unresponsive screen is worth.
    /// Safe to call twice — a second bring-up is another reset-and-init, which the panel tolerates.
    bool init();

    /// Push one rectangle of pixels — `(x1 - x0 + 1) * (y1 - y0 + 1)` of them, **two bytes each**,
    /// big-endian RGB565, which is the panel's format and the reason this signature names a colour
    /// order rather than choosing one.
    ///
    /// The corners are **inclusive**, and the addressing window is the driver's to set: the caller
    /// hands pixels and a rectangle and nothing else. `pixels` is not copied and must outlive the
    /// call — the transfer is synchronous, so "outlive" means until this returns.
    ///
    /// Returns **false** on a rectangle that is inside out, a null buffer, or a bus that refuses.
    bool flush(std::uint16_t x0, std::uint16_t y0, std::uint16_t x1, std::uint16_t y1,
               const std::uint8_t* pixels);

private:
    BusHandle device_;
    Revision revision_;
};

}  // namespace ili9342
