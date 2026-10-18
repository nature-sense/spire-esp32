// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "ili9342.hpp"
#include <ili9342_bus.hpp>

#include <cstddef>
#include <iterator>

namespace ili9342 {
namespace {

// The ILI9342's protocol, off its datasheet and the two vendor sequences that are known to work on
// this glass. Like every driver here, the whole of it is reachable through the seam — a command, its
// parameters, and the interval the panel needs afterwards.
//
// The **command set** this driver uses, and no more:
//
//   0x01  SWRESET   software reset
//   0x11  SLPOUT    leave sleep mode (the panel wakes up asleep)
//   0x21  INVON     colour inversion *on* — this panel's own requirement, not an option. Without it
//                   every colour comes out as its complement.
//   0x29  DISPON    display on
//   0x2A  CASET     column address set  \  the addressing window, two corners each, inclusive
//   0x2B  PASET     page address set    /
//   0x2C  RAMWR     memory write       — consumes the pixels the window points at
//   0x36  MADCTL    memory access control: the frame's orientation and colour order
//   0x3A  COLMOD    pixel format
//
// The rest — power control, timing, gamma — is the panel's tuning, and it is the *vendors'*, not this
// project's: transcribed from Espressif's `esp_lcd_ili9341` defaults and, for the E revision, from
// Espressif's own board support package for the CoreS3 (`bsp/m5stack_core_s3`, whose
// `ili9342e_init_cmds.h` is Apache-2.0). Nothing below is guessed, and nothing below is tuned here.

/// One line of a power-on sequence: a command, the parameters it carries, and the interval the panel
/// needs before it will take the next one.
///
/// `data` is a fixed buffer rather than a pointer because these tables are `constexpr`: the values
/// belong to the table, and a pointer would invite them to be changed somewhere the table cannot see.
struct Command {
    std::uint8_t command;
    std::uint8_t data[15];  ///< the longest block any of them carries is a 15-byte gamma curve
    std::uint8_t length;    ///< 0 for the commands that take no parameters
    std::uint16_t delay_ms;
};

/// The power-on block **both revisions share**.
///
/// Espressif's default sequence for this controller, and the first block of the BSP's E table
/// verbatim: the two authorities agree line for line, which is why there is one copy of it here.
constexpr Command kCommonInit[] = {
    {0xCF, {0x00, 0xAA, 0xE0}, 3, 0},                              // power control B
    {0xED, {0x67, 0x03, 0x12, 0x81}, 4, 0},                        // power-on sequence control
    {0xE8, {0x8A, 0x01, 0x78}, 3, 0},                              // driver timing control A
    {0xCB, {0x39, 0x2C, 0x00, 0x34, 0x02}, 5, 0},                  // power control A
    {0xF7, {0x20}, 1, 0},                                          // pump ratio control
    {0xF7, {0x20}, 1, 0},                                          // ...twice, as the vendors have it
    {0xEA, {0x00, 0x00}, 2, 0},                                    // driver timing control
    {0xC0, {0x23}, 1, 0},                                          // power control 1
    {0xC1, {0x11}, 1, 0},                                          // power control 2
    {0xC5, {0x43, 0x4C}, 2, 0},                                    // VCOM control 1
    {0xC7, {0xA0}, 1, 0},                                          // VCOM control 2
    {0xB1, {0x00, 0x1B}, 2, 0},                                    // frame rate control
    {0xF2, {0x00}, 1, 0},                                          // 3G enable (off)
    {0x26, {0x01}, 1, 0},                                          // gamma set, curve 1
    {0xE0, {0x1F, 0x36, 0x36, 0x3A, 0x0C, 0x05, 0x4F, 0x87,        // positive gamma correction
            0x3C, 0x08, 0x11, 0x35, 0x19, 0x13, 0x00}, 15, 0},
    {0xE1, {0x00, 0x09, 0x09, 0x05, 0x13, 0x0A, 0x30, 0x78,        // negative gamma correction
            0x43, 0x07, 0x0E, 0x0A, 0x26, 0x2C, 0x1F}, 15, 0},
    {0xB7, {0x07}, 1, 0},                                          // entry mode set
    {0xB6, {0x08, 0x82, 0x27}, 3, 0},                              // display function control
};

/// The **E revision's** extra tuning — the second block of the BSP's table, and the whole of what
/// separates it from the C.
///
/// Three of the BSP's lines are deliberately *not* here: its `0x3A`, `0x36` and `0x21` are the same
/// pixel format, the same `MADCTL` and the same inversion the C revision is set to, so they are stated
/// once, below, as the frame both revisions are left in, rather than twice — once per table — where
/// the two copies could drift apart. What remains is what is genuinely E-specific: the tuning of the
/// panel's charge pump, its timing and its gamma, including a second `0xB1`, `0xB7` and `0xCB` that
/// deliberately overwrite the shared block's.
constexpr Command kEInit[] = {
    {0xDD, {0x01}, 1, 0},                              // (E-specific; the vendor names no other use)
    {0xD5, {0x00}, 1, 0},                              // (E-specific)
    {0xB1, {0x22}, 1, 0},                              // frame rate control, overridden for the E
    {0xC8, {0x38}, 1, 0},                              // (E-specific)
    {0xCB, {0x1C}, 1, 0},                              // power control A, overridden for the E
    {0xC9, {0x1A}, 1, 0},                              // (E-specific)
    {0xCA, {0x1A}, 1, 0},                              // (E-specific)
    {0xB7, {0x5A, 0x41, 0x11, 0x19}, 4, 0},            // entry mode set, overridden for the E
    {0xE4, {0x04, 0x08, 0x11, 0x06, 0x12, 0x07, 0x3A,  // positive gamma, the E revision's curve
            0x76, 0x47, 0x07, 0x0F, 0x0A, 0x11, 0x19, 0x05}, 15, 0},
    {0xE5, {0x02, 0x03, 0x07, 0x06, 0x12, 0x07, 0x36,  // negative gamma, the E revision's curve
            0x5F, 0x48, 0x06, 0x10, 0x0C, 0x16, 0x14, 0x09}, 15, 0},
};

constexpr std::uint8_t kSoftwareReset = 0x01;
constexpr std::uint8_t kSleepOut = 0x11;
constexpr std::uint8_t kInvertOn = 0x21;
constexpr std::uint8_t kDisplayOn = 0x29;
constexpr std::uint8_t kColumnAddress = 0x2A;
constexpr std::uint8_t kPageAddress = 0x2B;
constexpr std::uint8_t kMemoryWrite = 0x2C;
constexpr std::uint8_t kMemoryAccessControl = 0x36;
constexpr std::uint8_t kPixelFormat = 0x3A;

/// `MADCTL`: RGB element order (bit 3 — the panel is wired BGR) with no row/column exchange (bit 5) and
/// no mirroring (bits 7:6). It is not a choice: it is the frame the panel's own glass is mounted in,
/// and a rotated display is a different order of drawing, not a different panel.
constexpr std::uint8_t kMemoryAccessControlValue = 0x08;

/// `COLMOD`: 16 bits per pixel, RGB565 — the format `flush()` names in its signature.
constexpr std::uint8_t kPixelFormatRgb565 = 0x55;

/// The datasheet's minimum after a software reset, rounded up as the vendors do.
constexpr std::uint16_t kResetDelayMs = 20;

/// The interval the vendors give the panel after sleep-out, and again after display-on. Both are
/// minimums rather than rates: the panel has a charge pump to bring up, and a frame sent into it before
/// this has elapsed is a frame it is not listening for yet.
constexpr std::uint16_t kPowerOnDelayMs = 120;

constexpr std::uint8_t high_byte(std::uint16_t value) {
    return static_cast<std::uint8_t>(value >> 8);
}

constexpr std::uint8_t low_byte(std::uint16_t value) {
    return static_cast<std::uint8_t>(value & 0xFF);
}

/// Send one command and, when it has any, its parameters; then wait the interval the panel asks for.
bool send(BusHandle device, const Command& command) {
    if (!bus_command(device, &command.command, 1)) {
        return false;
    }
    if (command.length != 0 && !bus_data(device, command.data, command.length)) {
        return false;
    }
    if (command.delay_ms != 0) {
        bus_delay_ms(command.delay_ms);
    }
    return true;
}

/// Send a table, in order, stopping at the first refusal.
bool send_all(BusHandle device, const Command* commands, std::size_t count) {
    for (std::size_t i = 0; i < count; ++i) {
        if (!send(device, commands[i])) {
            return false;
        }
    }
    return true;
}

}  // namespace

Ili9342::Ili9342(BusHandle device, Revision revision) : device_(device), revision_(revision) {}

bool Ili9342::init() {
    // The **software** reset, and not the reset *line*: a reset pin is the board's, and a board that
    // has one has already pulsed it, while this command is part of the panel's own protocol.
    if (!send(device_, {kSoftwareReset, {}, 0, kResetDelayMs})) {
        return false;
    }

    // A panel comes up asleep, and will not take a frame until it is woken.
    if (!send(device_, {kSleepOut, {}, 0, kPowerOnDelayMs})) {
        return false;
    }

    if (!send_all(device_, kCommonInit, std::size(kCommonInit))) {
        return false;
    }
    if (revision_ == Revision::E && !send_all(device_, kEInit, std::size(kEInit))) {
        return false;
    }

    // And the frame, last, so nothing in either table above can leave the panel somewhere else: the
    // access control, the pixel format, the panel's own inversion, and on — with the interval it wants
    // before it shows anything.
    const Command frame[] = {
        {kMemoryAccessControl, {kMemoryAccessControlValue}, 1, 0},
        {kPixelFormat, {kPixelFormatRgb565}, 1, 0},
        {kInvertOn, {}, 0, 0},
        {kDisplayOn, {}, 0, kPowerOnDelayMs},
    };
    return send_all(device_, frame, std::size(frame));
}

bool Ili9342::flush(std::uint16_t x0, std::uint16_t y0, std::uint16_t x1, std::uint16_t y1,
                    const std::uint8_t* pixels) {
    // A rectangle inside out, or no pixels to put in it. The panel would take either without
    // complaining — `CASET` with its corners reversed is a window it declares and never fills — so the
    // refusal has to be here, where the caller can still do something about it.
    if (pixels == nullptr || x1 < x0 || y1 < y0) {
        return false;
    }

    // **The addressing window**, as two commands with two corners each, high byte first. Both are
    // *inclusive*: the window is `[x0, x1]`, exactly the rectangle the caller drew in, which is why
    // neither corner is nudged here. (`esp_lcd_panel_draw_bitmap()` takes an exclusive end and does its
    // own `-1`; this driver's callers hand it corners, and `lvgl_display`'s sink does too.)
    const std::uint8_t columns[] = {high_byte(x0), low_byte(x0), high_byte(x1), low_byte(x1)};
    if (!bus_command(device_, &kColumnAddress, 1) || !bus_data(device_, columns, sizeof(columns))) {
        return false;
    }

    const std::uint8_t pages[] = {high_byte(y0), low_byte(y0), high_byte(y1), low_byte(y1)};
    if (!bus_command(device_, &kPageAddress, 1) || !bus_data(device_, pages, sizeof(pages))) {
        return false;
    }

    // **The pixels.** Two bytes each, big-endian, and the count is the window's own area — the panel
    // walks the window itself, so a count that disagrees with it is a band that wraps onto the row
    // below. The bytes are handed over untouched: this is where a driver that swapped them would turn
    // a red screen green, and the whole of the colour order it promises is `kPixelFormatRgb565` above.
    const std::size_t bytes =
        static_cast<std::size_t>(x1 - x0 + 1) * static_cast<std::size_t>(y1 - y0 + 1) * 2;
    return bus_command(device_, &kMemoryWrite, 1) && bus_data(device_, pixels, bytes);
}

}  // namespace ili9342
