// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// ft6336's **host test** — the protocol exercised on this machine, with no panel and no chip.
//
// `ft6336_bus.hpp` resolves to the fake in this directory, because the seam is included with angle
// brackets and this directory is ahead of the component's `include/` on the include path. So this
// controller's real frames go in as *bytes* and what the protocol wrote comes back out. See the fake's
// own comments for the two things it keeps.
//
// The cases below are this controller's own, and they are the four a bench teaches you to keep:
//
//   * the reply that proves presence — a chip id with the value the datasheet says;
//   * the register reads `read()` performs, with their expected coordinates;
//   * the report for **no finger** — a successful read with `down == false`, which is not a failure;
//   * and the malformed ones: a short reply, a truncated one, no reply at all. Those matter most,
//     because they are the ones that only ever happen on a bench.

#include <ft6336_bus.hpp>
#include "ft6336.hpp"

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

}  // namespace

int main() {
    ft6336::Ft6336 touch(nullptr);

    // probe(): the id the datasheet gives, read from the chip-id register.
    ft6336::fake_reset();
    ft6336::fake_answer({0x64});  // the FT6336U's own id
    expect(touch.probe(), "probe() accepts the controller's own id");
    expect(ft6336::fake_written() == std::vector<uint8_t>{0xA3}, "it asked the chip-id register");

    // probe(): a sibling part's id is a different part, and a silent bus is no part at all.
    ft6336::fake_reset();
    ft6336::fake_answer({0x36});  // an FT6236 — register-compatible, but not this board's part
    expect(!touch.probe(), "probe() rejects another part's id");

    ft6336::fake_reset();  // the device stays silent
    expect(!touch.probe(), "probe() fails rather than claiming a part that did not answer");

    // read(): no finger is a *successful* read with an empty report, not a failure.
    ft6336::fake_reset();
    ft6336::fake_answer({0x00});  // TD_STATUS: no points down
    ft6336::Touch point;
    expect(touch.read(point) && !point.down, "a panel with nothing on it reads true, down false");
    expect(ft6336::fake_written() == std::vector<uint8_t>{0x02}, "it asked the touch-status register");

    // read(): one finger, twelve-bit coordinates, high nibble first.
    ft6336::fake_reset();
    ft6336::fake_answer({0x01});                    // TD_STATUS: one point down
    ft6336::fake_answer({0x01, 0x23, 0x04, 0x56});  // P1: XH XL YH YL
    expect(touch.read(point) && point.down, "a finger reads true, down true");
    expect(point.x == 0x0123 && point.y == 0x0456, "the twelve-bit coordinates decode");
    expect(ft6336::fake_written() == std::vector<uint8_t>{0x02, 0x03},
           "it asked the status register, then the first point");

    // read(): the failures a bench is where you meet — a reply too short to hold the status...
    ft6336::fake_reset();
    ft6336::fake_answer({});  // an answer with no bytes in it at all
    expect(!touch.read(point), "a reply too short to hold the status reads false");

    // ...a truncated point reply...
    ft6336::fake_reset();
    ft6336::fake_answer({0x01});        // one point down...
    ft6336::fake_answer({0x01, 0x23});  // ...but only half of its four bytes arrive
    expect(!touch.read(point), "a truncated point reply reads false");

    // ...and no reply at all.
    ft6336::fake_reset();  // the device stays silent
    expect(!touch.read(point), "an exchange that failed reads false");

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
