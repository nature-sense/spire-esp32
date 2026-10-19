// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "st7789.hpp"
#include <st7789_bus.hpp>

namespace st7789 {

namespace {

// The ST7789's command words, off its datasheet (MIPI DCS). Named here rather than spelled as
// literals below so the sequence reads as the sequence it is.
constexpr std::uint8_t kSwReset = 0x01;
constexpr std::uint8_t kSlpIn = 0x10;
constexpr std::uint8_t kSlpOut = 0x11;
constexpr std::uint8_t kNorOn = 0x13;
constexpr std::uint8_t kInvOff = 0x20;
constexpr std::uint8_t kDispOn = 0x29;
constexpr std::uint8_t kCaset = 0x2A;
constexpr std::uint8_t kPaset = 0x2B;
constexpr std::uint8_t kRamWr = 0x2C;
constexpr std::uint8_t kMadCtl = 0x36;
constexpr std::uint8_t kColMod = 0x3A;
constexpr std::uint8_t kGctrl = 0xB7;
constexpr std::uint8_t kVcoms = 0xBB;
constexpr std::uint8_t kVrhs = 0xC3;
constexpr std::uint8_t kPwCtrl1 = 0xD0;
constexpr std::uint8_t kRamCtrl = 0xB0;
constexpr std::uint8_t kPvGamCtrl = 0xE0;
constexpr std::uint8_t kNvGamCtrl = 0xE1;
constexpr std::uint8_t kGammaCurve = 0x26;

// Send one command with no parameters.
bool command(BusHandle device, std::uint8_t word) {
    return bus_command(device, &word, 1);
}

// Send one command followed by its parameters, as one command transfer and one data transfer.
bool command_with(BusHandle device, std::uint8_t word, const std::uint8_t* params,
                  std::size_t count) {
    if (!command(device, word)) {
        return false;
    }
    if (count == 0) {
        return true;
    }
    return bus_data(device, params, count);
}

// Send one command with a single parameter byte.
bool command_with(BusHandle device, std::uint8_t word, std::uint8_t param) {
    return command_with(device, word, &param, 1);
}

}  // namespace

St7789::St7789(BusHandle device) : device_(device) {}

bool St7789::init() {
    // The power-on sequence, in the vendored order: the pump needs the waits, so they are part of the
    // sequence rather than an optimisation.
    if (!command(device_, kSlpIn)) {
        return false;
    }
    bus_delay_ms(10);

    if (!command(device_, kSwReset)) {
        return false;
    }
    bus_delay_ms(200);

    if (!command(device_, kSlpOut)) {
        return false;
    }
    bus_delay_ms(300);

    if (!command(device_, kNorOn)) {
        return false;
    }

    // The ST7789's tuning, which no scaffold can invent.
    if (!command_with(device_, kGctrl, 0x44)) {
        return false;
    }
    if (!command_with(device_, kVcoms, 0x24)) {
        return false;
    }
    if (!command_with(device_, kVrhs, 0x13)) {
        return false;
    }

    {
        const std::uint8_t params[] = {0xA4, 0xA1};
        if (!command_with(device_, kPwCtrl1, params, sizeof(params))) {
            return false;
        }
    }
    {
        const std::uint8_t params[] = {0x00, 0xC0};
        if (!command_with(device_, kRamCtrl, params, sizeof(params))) {
            return false;
        }
    }
    {
        const std::uint8_t params[] = {0xD0, 0x00, 0x02, 0x07, 0x0A, 0x28, 0x32,
                                       0x44, 0x42, 0x06, 0x0E, 0x12, 0x14, 0x17};
        if (!command_with(device_, kPvGamCtrl, params, sizeof(params))) {
            return false;
        }
    }
    {
        const std::uint8_t params[] = {0xD0, 0x00, 0x02, 0x07, 0x0A, 0x28, 0x31,
                                       0x54, 0x47, 0x0E, 0x1C, 0x17, 0x1B, 0x1E};
        if (!command_with(device_, kNvGamCtrl, params, sizeof(params))) {
            return false;
        }
    }
    if (!command_with(device_, kGammaCurve, 0x01)) {
        return false;
    }

    // Address mode: RGB order, no mirroring, no row/column exchange.
    if (!command_with(device_, kMadCtl, 0x00)) {
        return false;
    }

    // Pixel format: 16 bits per pixel, RGB565 — the format `flush()` names.
    if (!command_with(device_, kColMod, 0x55)) {
        return false;
    }

    // Colour inversion off, then the display on.
    if (!command(device_, kInvOff)) {
        return false;
    }
    if (!command(device_, kDispOn)) {
        return false;
    }

    return true;
}

bool St7789::flush(std::uint16_t x0, std::uint16_t y0, std::uint16_t x1, std::uint16_t y1,
                   const std::uint8_t* pixels) {
    // A reversed window or no pixels is a refusal here: the ST7789 accepts a reversed window and
    // never fills it, so the caller has to be told rather than left with a blank rectangle.
    if (x1 < x0 || y1 < y0 || pixels == nullptr) {
        return false;
    }

    // The addressing window: two corners each, high byte first, both inclusive.
    {
        const std::uint8_t params[] = {
            static_cast<std::uint8_t>(x0 >> 8), static_cast<std::uint8_t>(x0 & 0xFF),
            static_cast<std::uint8_t>(x1 >> 8), static_cast<std::uint8_t>(x1 & 0xFF),
        };
        if (!command_with(device_, kCaset, params, sizeof(params))) {
            return false;
        }
    }
    {
        const std::uint8_t params[] = {
            static_cast<std::uint8_t>(y0 >> 8), static_cast<std::uint8_t>(y0 & 0xFF),
            static_cast<std::uint8_t>(y1 >> 8), static_cast<std::uint8_t>(y1 & 0xFF),
        };
        if (!command_with(device_, kPaset, params, sizeof(params))) {
            return false;
        }
    }

    // RAMWR, then the pixels: two bytes each, big-endian RGB565, the count the window's own area.
    if (!command(device_, kRamWr)) {
        return false;
    }

    const std::size_t width = static_cast<std::size_t>(x1 - x0 + 1);
    const std::size_t height = static_cast<std::size_t>(y1 - y0 + 1);
    const std::size_t bytes = width * height * 2;
    return bus_data(device_, pixels, bytes);
}

}  // namespace st7789
