// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "axp2101.hpp"
#include <axp2101_bus.hpp>

#include <cstddef>
#include <cstdint>

namespace axp2101 {

// The part's protocol, off the datasheet — and it is only the **reading** half of it. Every register
// below is fetched and decoded; nothing here is written, because what a board's power tree looks like is
// the board's decision and not a driver's (see the header):
//
//   0x00  STATUS1            bit 3 = battery present  (bit 4 = batfet, 2 = active mode, 1 = thermal,
//                                                      0 = input current limited)
//   0x01  STATUS2            bits 7-5 = current direction: 0x00 standby, 0x01 charging, 0x02 discharging
//   0x03  IC type            the part's identity: **0x4A** is an AXP2101 and nothing else is
//   0x30  ADC channel ctrl   bit 0 = battery-voltage measurement enabled (not read here)
//   0x34  battery voltage    **high** byte, bits 4-0 meaningful (0x35 is the low byte)  → millivolts
//   0xA4  battery percent    the part's own gauge, 0..100
//
// The ADC hands its measurements over as a **high byte and a low byte in adjacent registers**, and only
// the low five bits of the high byte are the value's top bits: `battery voltage = (high & 0x1F) << 8 |
// low`. The upper bits of that high byte belong to the part, not to the number, so they are masked — a
// driver that read them as part of the value would report a battery several volts too high, which is
// exactly the kind of bug that looks like a wiring fault on a bench.
//
// The two bytes are adjacent and the address pointer auto-increments, so one `bus_write_read` takes both
// halves of the **same** conversion. Two separate reads could straddle one.
namespace {

constexpr std::uint8_t kRegStatus1 = 0x00;
constexpr std::uint8_t kRegStatus2 = 0x01;
constexpr std::uint8_t kRegIcType = 0x03;
constexpr std::uint8_t kRegBatteryVoltage = 0x34;
constexpr std::uint8_t kRegBatteryPercent = 0xA4;

constexpr std::uint8_t kChipId = 0x4A;

constexpr int kBatteryPresentBit = 3;
constexpr int kCurrentDirectionShift = 5;
constexpr std::uint8_t kCurrentDirectionMask = 0x07;
constexpr std::uint8_t kCurrentDirectionCharging = 0x01;
constexpr std::uint8_t kHighByteMask = 0x1F;

/// Read one register, as one exchange: the pointer goes out and the value comes back.
bool read_byte(BusHandle device, std::uint8_t reg, std::uint8_t& value) {
    return bus_write_read(device, &reg, 1, &value, 1);
}

}  // namespace

Axp2101::Axp2101(BusHandle device) : device_(device) {}

bool Axp2101::probe() {
    // The part names itself, so this is identity and not presence: a different XPowers part answering at
    // the address reads false rather than being taken for an AXP2101.
    std::uint8_t type = 0;
    if (!read_byte(device_, kRegIcType, type)) {
        return false;
    }
    return type == kChipId;
}

bool Axp2101::battery_connected(bool& connected) {
    std::uint8_t status = 0;
    if (!read_byte(device_, kRegStatus1, status)) {
        return false;
    }
    connected = ((status >> kBatteryPresentBit) & 0x01) != 0;
    return true;
}

bool Axp2101::charging(bool& charging) {
    std::uint8_t status = 0;
    if (!read_byte(device_, kRegStatus2, status)) {
        return false;
    }
    charging = ((status >> kCurrentDirectionShift) & kCurrentDirectionMask) == kCurrentDirectionCharging;
    return true;
}

bool Axp2101::battery_voltage_mv(std::uint16_t& millivolts) {
    // The high byte and the low byte beside it, in one exchange: the pointer auto-increments and the two
    // halves belong to the same conversion. Only the low five bits of the high byte are the value's.
    const std::uint8_t reg = kRegBatteryVoltage;
    std::uint8_t pair[2] = {};
    if (!bus_write_read(device_, &reg, 1, pair, sizeof(pair))) {
        return false;
    }
    millivolts = static_cast<std::uint16_t>((pair[0] & kHighByteMask) << 8 | pair[1]);
    return true;
}

bool Axp2101::battery_percent(int& percent) {
    // The part's own gauge, handed over as a percentage. The driver neither smooths nor corrects it; what
    // the percentage *means* is a policy above this layer.
    std::uint8_t gauge = 0;
    if (!read_byte(device_, kRegBatteryPercent, gauge)) {
        return false;
    }
    percent = static_cast<int>(gauge);
    return true;
}

}  // namespace axp2101
