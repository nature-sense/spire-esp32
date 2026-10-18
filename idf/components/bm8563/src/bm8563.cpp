// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "bm8563.hpp"
#include <bm8563_bus.hpp>

#include <cstddef>
#include <cstdint>

namespace bm8563 {

// The clock's protocol, off the datasheet. The BM8563's register map is the PCF8563's — the part is
// that family's, and the two are read and written identically:
//
//   0x00  Control_status_1   the register `probe()` reads: presence, and nothing that names the part
//   0x01  Control_status_2
//   0x02  VL_seconds         bit 7 = VL (voltage low), bits 6-0 = seconds, BCD
//   0x03  minutes                                        bits 6-0, BCD
//   0x04  hours                                          bits 5-0, BCD
//   0x05  days                                           bits 5-0, BCD
//   0x06  weekdays                                       bits 2-0, a day number, not a date
//   0x07  century_months     bit 7 = century,  bits 4-0 = months, BCD
//   0x08  years                                          bits 7-0, BCD
//
// Every value is **BCD**: two decimal digits, one per nibble, so `0x26` is the number twenty-six and
// not thirty-eight. The high bits the datasheet reserves for flags — VL in the seconds register, the
// century in the months register, and the unused high bits of the rest — are masked off before a value
// is decoded, so a reserved bit that reads back set cannot leak into a field. The seconds' own bit 7 is
// the one flag that is not discarded: it is what makes `now()` report *false* rather than a number.
//
// The seven clock registers are consecutive and the part's address pointer auto-increments, so `now()`
// asks once and takes all seven as one instant, and `set()` writes the same seven as one frame. There
// is no start bit to set and no stop bit to clear: a BM8563 comes out of reset running, and the only
// thing that ever stops the count is a power loss — which is exactly what the VL flag records.
namespace {

constexpr std::uint8_t kRegControl1 = 0x00;
constexpr std::uint8_t kRegSeconds = 0x02;

constexpr std::uint8_t kVoltageLow = 0x80;  // 0x02 bit 7: the oscillator stopped — the time is wrong
constexpr std::uint8_t kCentury = 0x80;     // 0x07 bit 7: 1 = 20xx, 0 = 19xx

constexpr std::uint8_t kSecondsMask = 0x7F;
constexpr std::uint8_t kMinutesMask = 0x7F;
constexpr std::uint8_t kHoursMask = 0x3F;
constexpr std::uint8_t kDaysMask = 0x3F;
constexpr std::uint8_t kWeekdaysMask = 0x07;
constexpr std::uint8_t kMonthsMask = 0x1F;

/// The number a BCD byte holds: `0x26` is twenty-six.
int from_bcd(std::uint8_t byte) {
    return static_cast<int>(byte >> 4) * 10 + static_cast<int>(byte & 0x0F);
}

/// The BCD byte that holds a number: twenty-six is `0x26`.
std::uint8_t to_bcd(int value) {
    return static_cast<std::uint8_t>(((value / 10) << 4) | (value % 10));
}

}  // namespace

Bm8563::Bm8563(BusHandle device) : device_(device) {}

bool Bm8563::probe() {
    // Control_status_1, read as one exchange. The part has no chip-id register, so what this proves is
    // that *something* at the address answers — presence, not identity. The channel that proves the
    // part is a working clock is a reading, and that is where the voltage-low flag earns its keep.
    std::uint8_t control = 0;
    const std::uint8_t reg = kRegControl1;
    return bus_write_read(device_, &reg, 1, &control, 1);
}

bool Bm8563::now(DateTime& when) {
    // Seconds through years, in one exchange: the address pointer walks the part's own register order,
    // so the clock is read as a single instant rather than a sequence that could straddle a tick.
    std::uint8_t reg = kRegSeconds;
    std::uint8_t block[7] = {};
    if (!bus_write_read(device_, &reg, 1, block, sizeof(block))) {
        return false;  // the bus refused, or the device answered with fewer than seven bytes
    }

    // VL: the oscillator stopped since the clock was last set, so every field below is a number the part
    // is no longer entitled to. A reading that cannot be trusted is not a reading — the same rule the
    // sensors follow with their checksum — and it is the caller's signal to set the clock again.
    if ((block[0] & kVoltageLow) != 0) {
        return false;
    }

    when.second = from_bcd(block[0] & kSecondsMask);
    when.minute = from_bcd(block[1] & kMinutesMask);
    when.hour = from_bcd(block[2] & kHoursMask);
    when.day = from_bcd(block[3] & kDaysMask);
    when.weekday = static_cast<int>(block[4] & kWeekdaysMask);
    when.month = from_bcd(block[5] & kMonthsMask);

    // The century is a bit rather than a number, so the year is read from it: 1 = 20xx, 0 = 19xx.
    const bool twenty_first = (block[5] & kCentury) != 0;
    when.year = from_bcd(block[6]) + (twenty_first ? 2000 : 1900);

    return true;
}

bool Bm8563::set(const DateTime& when) {
    // The same seven registers, written as one frame: the address pointer first, then the clock.
    //
    // The seconds byte goes out with bit 7 clear, which is also what clears the part's voltage-low flag:
    // writing the clock declares the time trustworthy again, because the caller has just said what the
    // time is. The century bit rides in the months byte, set for a 20xx year.
    const bool twenty_first = when.year >= 2000;
    const int year_of_century = twenty_first ? when.year - 2000 : when.year - 1900;

    const std::uint8_t frame[8] = {
        kRegSeconds,
        to_bcd(when.second),
        to_bcd(when.minute),
        to_bcd(when.hour),
        to_bcd(when.day),
        to_bcd(when.weekday),
        static_cast<std::uint8_t>(to_bcd(when.month) | (twenty_first ? kCentury : 0x00)),
        to_bcd(year_of_century),
    };
    return bus_write(device_, frame, sizeof(frame));
}

}  // namespace bm8563
