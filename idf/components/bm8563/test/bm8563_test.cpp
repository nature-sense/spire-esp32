// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// bm8563's **host test** — the protocol exercised on this machine, with no board and no chip.
//
// `bm8563_bus.hpp` resolves to the fake in this directory, because the seam is included with angle
// brackets and this directory is ahead of the component's `include/` on the include path. So the
// clock's real frames go in as *bytes* and what the protocol wrote comes back out. See the fake's own
// comments for the three things it keeps.
//
// The cases below are this device's own, and they are the ones a bench teaches you to keep:
//
//   * the exchange that proves presence — a device answering the control register;
//   * the whole clock, seven BCD registers, read in one exchange and decoded to a calendar;
//   * the **century bit**, which is a bit and not a number, both ways: a frame read and a frame written;
//   * the voltage-low flag — the part saying its count is wrong — which is no reading, not a wrong one;
//   * the frame `set()` writes, byte for byte, including that a 19xx year carries no century bit;
//   * a write the bus refuses;
//   * and the malformed ones: a reply too short, no reply at all. Those matter most, because they are
//     the ones that only ever happen on a bench.

#include <bm8563_bus.hpp>
#include "bm8563.hpp"

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
    bm8563::Bm8563 clock(nullptr);

    // probe(): the part has no chip id, so presence is "a device answered the control register". What
    // the byte *says* is not checked — only that one came back.
    bm8563::fake_reset();
    bm8563::fake_answer({0x00});
    expect(clock.probe(), "probe() accepts a device that answers the control register");
    expect(bm8563::fake_written() == std::vector<uint8_t>{0x00}, "it asked the control register");

    // probe(): a silent bus is no device at all.
    bm8563::fake_reset();  // the device stays silent
    expect(!clock.probe(), "probe() fails rather than claiming a device that did not answer");

    // now(): seven BCD registers, taken in one exchange, decoded to a calendar.
    //   the frame is 2026-10-16, weekday 5, 13:45:30 — with the century bit set in the months byte
    //   (0x90 = 0x10 and the century), and every value BCD: 0x30 is thirty, 0x45 is forty-five.
    bm8563::fake_reset();
    bm8563::fake_answer({0x30, 0x45, 0x13, 0x16, 0x05, 0x90, 0x26});
    bm8563::DateTime when;
    expect(clock.now(when), "a clock that is running reads true");
    expect(bm8563::fake_written() == std::vector<uint8_t>{0x02}, "it asked the seconds register");
    expect(when.year == 2026 && when.month == 10 && when.day == 16 && when.weekday == 5 &&
               when.hour == 13 && when.minute == 45 && when.second == 30,
           "the seven bytes decode to 2026-10-16 13:45:30");

    // The century is a bit, not a number: the same fields with it clear are a 19xx year.
    bm8563::fake_reset();
    bm8563::fake_answer({0x30, 0x45, 0x13, 0x16, 0x05, 0x10, 0x26});
    expect(clock.now(when) && when.year == 1926 && when.month == 10,
           "the century bit clear means 19xx, not 20xx");

    // VL: the oscillator stopped, so the fields are readable and wrong. No reading, not a wrong one —
    // the same rule the sensors follow with their checksum, and the caller's cue to set the clock.
    bm8563::fake_reset();
    bm8563::fake_answer({0xB0, 0x45, 0x13, 0x16, 0x05, 0x90, 0x26});  // 0x30 with the VL flag set
    expect(!clock.now(when), "a voltage-low flag reads false rather than a time that is wrong");

    // A reply too short to hold the clock...
    bm8563::fake_reset();
    bm8563::fake_answer({0x30});  // one byte, where seven were asked for
    expect(!clock.now(when), "a reply too short to hold seven registers reads false");

    // ...and no reply at all.
    bm8563::fake_reset();  // the device stays silent
    expect(!clock.now(when), "an exchange that failed reads false");

    // set(): the same seven registers written as one frame, in the part's order and in BCD.
    bm8563::fake_reset();
    const bm8563::DateTime start{2026, 10, 16, 5, 13, 45, 30};
    expect(clock.set(start), "setting the clock writes true");
    expect(bm8563::fake_written() ==
               std::vector<uint8_t>{0x02, 0x30, 0x45, 0x13, 0x16, 0x05, 0x90, 0x26},
           "it wrote the pointer and the seven clock registers, in order");

    // A 19xx year sets no century bit — and the frame it writes is the frame it reads back, so the
    // encoding proves itself against the decoding.
    bm8563::fake_reset();
    const bm8563::DateTime millennium{1999, 12, 31, 0, 23, 59, 58};
    expect(clock.set(millennium), "setting a 19xx clock writes true");
    const std::vector<uint8_t> written = bm8563::fake_written();
    expect(written == std::vector<uint8_t>{0x02, 0x58, 0x59, 0x23, 0x31, 0x00, 0x12, 0x99},
           "a 19xx year carries no century bit");

    bm8563::fake_reset();
    bm8563::fake_answer(
        {written[1], written[2], written[3], written[4], written[5], written[6], written[7]});
    bm8563::DateTime read_back;
    expect(clock.now(read_back), "the frame it wrote reads back");
    expect(read_back.year == 1999 && read_back.month == 12 && read_back.day == 31 &&
               read_back.weekday == 0 && read_back.hour == 23 && read_back.minute == 59 &&
               read_back.second == 58,
           "what went out is what comes back — the century bit and the BCD agree both ways");

    // A write the bus refuses is a clock that did not get set.
    bm8563::fake_reset();
    bm8563::fake_writes_fail(true);
    expect(!clock.set(start), "a refused write reads false");
    expect(bm8563::fake_written().empty(), "a refused write put nothing on the wire");

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
