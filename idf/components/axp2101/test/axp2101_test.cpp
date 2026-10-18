// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// axp2101's **host test** — the protocol exercised on this machine, with no board and no chip.
//
// `axp2101_bus.hpp` resolves to the fake in this directory, because the seam is included with angle
// brackets and this directory is ahead of the component's `include/` on the include path. So the PMIC's
// real registers go in as *bytes* and the register the protocol asked for comes back out. See the
// fake's own comments for the two things it keeps.
//
// The cases below are this device's own, and they are the ones a bench teaches you to keep:
//
//   * the exchange that proves **identity** — an ID register that reads 0x4A and one that does not;
//   * the battery states: bit 3 of STATUS1, and the current-direction field of STATUS2 in all three of
//     the values that matter (standing by, charging, discharging);
//   * the voltage, including the part of the high byte that is *not* the number — the case a driver that
//     forgot to mask it would fail silently, reporting a battery several volts too high;
//   * the gauge;
//   * and the malformed one: no reply at all. That matters most, because it is the one that only ever
//     happens on a bench.

#include <axp2101_bus.hpp>
#include "axp2101.hpp"

#include <cstdint>
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
    axp2101::Axp2101 pmu(nullptr);

    // probe(): this part names itself, so the value is checked — identity, not presence. A different
    // XPowers part answering at the address must read false rather than be taken for an AXP2101.
    axp2101::fake_reset();
    axp2101::fake_answer({0x4A});
    expect(pmu.probe(), "probe() accepts a part whose ID register reads 0x4A");
    expect(axp2101::fake_written() == std::vector<uint8_t>{0x03}, "it asked the IC-type register");

    axp2101::fake_reset();
    axp2101::fake_answer({0x00});  // a device, but not this device
    expect(!pmu.probe(), "probe() refuses an ID that is not an AXP2101");

    axp2101::fake_reset();  // the device stays silent
    expect(!pmu.probe(), "probe() fails rather than claiming a device that did not answer");

    // battery_connected(): STATUS1 bit 3. An absent battery is a successful reading of *no battery*,
    // not a failure — the two mean different things to a caller deciding whether to run on the cell.
    axp2101::fake_reset();
    axp2101::fake_answer({0x08});
    bool connected = false;
    expect(pmu.battery_connected(connected), "a status register that answers reads true");
    expect(connected, "bit 3 set is a battery connected");
    expect(axp2101::fake_written() == std::vector<uint8_t>{0x00}, "it asked STATUS1");

    axp2101::fake_reset();
    axp2101::fake_answer({0x00});
    expect(pmu.battery_connected(connected), "no battery is a reading, not a failure");
    expect(!connected, "bit 3 clear is no battery");

    axp2101::fake_reset();  // the device stays silent
    expect(!pmu.battery_connected(connected), "an exchange that failed reads false");

    // charging(): STATUS2 bits 7-5, where 0x01 is charging and the rest are not.
    axp2101::fake_reset();
    axp2101::fake_answer({0x20});
    bool charging = false;
    expect(pmu.charging(charging), "a status register that answers reads true");
    expect(charging, "bits 7-5 = 001 is charging");
    expect(axp2101::fake_written() == std::vector<uint8_t>{0x01}, "it asked STATUS2");

    axp2101::fake_reset();
    axp2101::fake_answer({0x00});
    expect(pmu.charging(charging) && !charging, "bits 7-5 = 000 is standing by, not charging");

    axp2101::fake_reset();
    axp2101::fake_answer({0x40});
    expect(pmu.charging(charging) && !charging, "bits 7-5 = 010 is discharging, not charging");

    // battery_voltage_mv(): 0x34 and 0x35, the high byte's low five bits and all of the low byte.
    axp2101::fake_reset();
    axp2101::fake_answer({0x0F, 0x3C});
    std::uint16_t millivolts = 0;
    expect(pmu.battery_voltage_mv(millivolts), "a voltage measurement reads true");
    expect(millivolts == 3900, "0x0F 0x3C is 3900 mV");
    expect(axp2101::fake_written() == std::vector<uint8_t>{0x34}, "it asked the voltage high byte");

    axp2101::fake_reset();
    axp2101::fake_answer({0x2F, 0x3C});  // bits 5-7 of the high byte are the part's, not the number's
    expect(pmu.battery_voltage_mv(millivolts) && millivolts == 3900,
           "the high byte's upper bits are masked out of the value");

    axp2101::fake_reset();  // the device stays silent
    expect(!pmu.battery_voltage_mv(millivolts), "an exchange that failed reads false");

    // battery_percent(): the part's own gauge.
    axp2101::fake_reset();
    axp2101::fake_answer({0x5A});
    int percent = -1;
    expect(pmu.battery_percent(percent), "a gauge reading reads true");
    expect(percent == 90, "0x5A is ninety percent");
    expect(axp2101::fake_written() == std::vector<uint8_t>{0xA4}, "it asked the gauge register");

    axp2101::fake_reset();  // the device stays silent
    expect(!pmu.battery_percent(percent), "a gauge that will not answer reads false");

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
