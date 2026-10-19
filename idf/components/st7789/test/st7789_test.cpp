// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// st7789's **host test** — the protocol exercised on this machine, with no panel and no chip.
//
// `st7789_bus.hpp` resolves to the fake in this directory, because the seam is included with angle
// brackets and this directory is ahead of the component's `include/` on the include path. So what the
// driver *sent* comes back out, already split into commands and data. See the fake's own comments.
//
// The cases below are this panel's own:
//
//   * the command sequence `init()` sends, asserted against `fake_commands()` — the exact words, in
//     order, because a display left in the wrong frame is a fault the hardware will not report;
//   * the waits `init()` asks for, asserted against `fake_delays()` — a panel's power-on is a
//     sequence *with timing*, and a dropped sleep-out delay comes up on the bench and not in a cold
//     room;
//   * the addressing window `flush()` sets, with its two corner parameters;
//   * the pixels, and the byte order they went out in — the "everything is green" bug lives here;
//   * and the failures: a bus that refuses, a reversed window, no pixels.

#include <st7789_bus.hpp>
#include "st7789.hpp"

#include <cstdio>
#include <vector>

namespace {

int failures = 0;

void expect(bool ok, const char* what) {
    if (!ok) {
        std::printf("FAIL %s\n", what);
        ++failures;
    }
}

// The command words `init()` must send, in order. The tuning commands carry parameters, which go out
// as data and so do not appear here — only the command words do.
const std::vector<std::uint8_t> kInitCommands = {
    0x10,  // SLPIN
    0x01,  // SWRESET
    0x11,  // SLPOUT
    0x13,  // NORON
    0xB7,  // GCTRL
    0xBB,  // VCOMS
    0xC3,  // VRHS
    0xD0,  // PWCTRL1
    0xB0,  // RAMCTRL
    0xE0,  // PVGAMCTRL
    0xE1,  // NVGAMCTRL
    0x26,  // GAMMA_CURVE
    0x36,  // MADCTL
    0x3A,  // COLMOD
    0x20,  // INVOFF
    0x29,  // DISPON
};

// The waits `init()` must ask for, in order: after SLPIN, after SWRESET, after SLPOUT.
const std::vector<std::uint32_t> kInitDelays = {10, 200, 300};

// The data bytes `init()` must send, in order — the parameters of the tuning commands, then MADCTL,
// COLMOD. Transcribed from the ST7789's own tuning table, not computed from the driver.
const std::vector<std::uint8_t> kInitData = {
    0x44,                                            // GCTRL
    0x24,                                            // VCOMS
    0x13,                                            // VRHS
    0xA4, 0xA1,                                      // PWCTRL1
    0x00, 0xC0,                                      // RAMCTRL
    0xD0, 0x00, 0x02, 0x07, 0x0A, 0x28, 0x32,        // PVGAMCTRL
    0x44, 0x42, 0x06, 0x0E, 0x12, 0x14, 0x17,
    0xD0, 0x00, 0x02, 0x07, 0x0A, 0x28, 0x31,        // NVGAMCTRL
    0x54, 0x47, 0x0E, 0x1C, 0x17, 0x1B, 0x1E,
    0x01,                                            // GAMMA_CURVE
    0x00,                                            // MADCTL: RGB order, no mirroring, no exchange
    0x55,                                            // COLMOD: 16 bits per pixel, RGB565
};

void test_init_sequence() {
    st7789::fake_reset();
    st7789::St7789 panel(nullptr);

    expect(panel.init(), "init() succeeds when the bus accepts every transfer");
    expect(st7789::fake_commands() == kInitCommands,
           "the power-on sequence went out as commands, in order");
    expect(st7789::fake_delays() == kInitDelays,
           "the power-on sequence asked for its waits, in order");
    expect(st7789::fake_data() == kInitData,
           "the power-on sequence sent its parameters, in order");
}

void test_init_refused() {
    st7789::fake_reset();
    st7789::fake_refuse();
    st7789::St7789 panel(nullptr);

    expect(!panel.init(), "init() returns false when the bus refuses every transfer");
}

void test_init_refused_partway() {
    st7789::fake_reset();
    st7789::fake_refuse_after(3);
    st7789::St7789 panel(nullptr);

    expect(!panel.init(), "init() returns false when the bus stops answering partway through");
}

void test_flush_window_and_pixels() {
    st7789::fake_reset();
    st7789::St7789 panel(nullptr);

    // A 2x2 rectangle at (10, 20): four pixels, two bytes each, big-endian RGB565.
    const std::uint8_t pixels[] = {
        0xF8, 0x00,  // red
        0x07, 0xE0,  // green
        0x00, 0x1F,  // blue
        0xFF, 0xFF,  // white
    };

    expect(panel.flush(10, 20, 11, 21, pixels), "flush() succeeds when the bus accepts");

    // The commands: CASET, PASET, RAMWR.
    const std::vector<std::uint8_t> expected_commands = {0x2A, 0x2B, 0x2C};
    expect(st7789::fake_commands() == expected_commands,
           "flush() set the window and started a RAM write, in order");

    // The data: CASET's two corners, PASET's two corners, then the pixels — all in one stream.
    const std::vector<std::uint8_t> expected_data = {
        0x00, 0x0A, 0x00, 0x0B,  // CASET: x0=10, x1=11, high byte first
        0x00, 0x14, 0x00, 0x15,  // PASET: y0=20, y1=21, high byte first
        0xF8, 0x00, 0x07, 0xE0, 0x00, 0x1F, 0xFF, 0xFF,  // the pixels, big-endian
    };
    expect(st7789::fake_data() == expected_data,
           "flush() sent the window corners and the pixels, big-endian, in order");
}

void test_flush_reversed_window() {
    st7789::fake_reset();
    st7789::St7789 panel(nullptr);

    const std::uint8_t pixels[] = {0x00, 0x00};

    expect(!panel.flush(11, 20, 10, 21, pixels),
           "flush() refuses a reversed x window");
    expect(!panel.flush(10, 21, 11, 20, pixels),
           "flush() refuses a reversed y window");
    expect(st7789::fake_commands().empty(),
           "a refused flush() sent nothing to the panel");
}

void test_flush_no_pixels() {
    st7789::fake_reset();
    st7789::St7789 panel(nullptr);

    expect(!panel.flush(0, 0, 0, 0, nullptr), "flush() refuses a null pixel buffer");
    expect(st7789::fake_commands().empty(),
           "a refused flush() sent nothing to the panel");
}

void test_flush_refused() {
    st7789::fake_reset();
    st7789::fake_refuse();
    st7789::St7789 panel(nullptr);

    const std::uint8_t pixels[] = {0x00, 0x00};
    expect(!panel.flush(0, 0, 0, 0, pixels),
           "flush() returns false when the bus refuses");
}

}  // namespace

int main() {
    test_init_sequence();
    test_init_refused();
    test_init_refused_partway();
    test_flush_window_and_pixels();
    test_flush_reversed_window();
    test_flush_no_pixels();
    test_flush_refused();

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
