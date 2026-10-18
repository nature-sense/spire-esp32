// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// sht20's **host test** — the protocol exercised on this machine, with no board and no chip.
//
// `sht20_bus.hpp` resolves to the fake in this directory, because the seam is included with
// angle brackets and this directory is ahead of the component's `include/` on the include path. So
// this sensor's real frames go in as *bytes* and what the protocol wrote comes back out. See the
// fake's own comments for the two things it keeps.
//
// The cases below are this sensor's own, and they are the ones a bench teaches you to keep:
//
//   * the exchange that proves presence — a device answering the user register;
//   * the two measurements, with the values the datasheet's conversions give;
//   * a frame whose data is right and whose checksum is not — which is no reading, not a reading;
//   * and the malformed ones: a reply too short, no reply at all. Those matter most, because they are
//     the ones that only ever happen on a bench.

#include <sht20_bus.hpp>
#include "sht20.hpp"

#include <cmath>
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

/// The datasheet's conversions are real arithmetic, in `float`, so a value is "the right one" within
/// a tolerance rather than to the last bit.
bool close_to(float value, float expected) {
    return std::fabs(value - expected) < 1e-3f;
}

}  // namespace

int main() {
    sht20::Sht20 sensor(nullptr);

    // probe(): the SHT2x has no chip id, so presence is "a device answered the user register". What
    // the byte *says* is not checked — only that one came back. (Unlike a panel controller, there is
    // no id here that is this part's and no other's.)
    sht20::fake_reset();
    sht20::fake_answer({0x00});
    expect(sensor.probe(), "probe() accepts a device that answers the user register");
    expect(sht20::fake_written() == std::vector<uint8_t>{0xE7}, "it asked the user register");

    // probe(): a silent bus is no device at all.
    sht20::fake_reset();  // the device stays silent
    expect(!sensor.probe(), "probe() fails rather than claiming a device that did not answer");

    // temperature(): the hold-master command, and the datasheet's conversion of the reply.
    //
    // The raw word is 0x8000 — half scale — which puts the arithmetic on round numbers:
    //   41.01 °C = -46.85 + 175.72 * 0.5        (the frame's checksum byte is 0x23)
    sht20::fake_reset();
    sht20::fake_answer({0x80, 0x00, 0x23});
    float celsius = 0.0f;
    expect(sensor.temperature(celsius), "a temperature measurement reads true");
    expect(close_to(celsius, 41.01f), "raw 0x8000 is 41.01 °C");
    expect(sht20::fake_written() == std::vector<uint8_t>{0xE3}, "it sent the temperature command");

    // humidity(): the same word, read through the humidity command, is a different quantity:
    //   56.5 %RH = -6.00 + 125.00 * 0.5
    sht20::fake_reset();
    sht20::fake_answer({0x80, 0x00, 0x23});
    float relative_humidity = 0.0f;
    expect(sensor.humidity(relative_humidity), "a humidity measurement reads true");
    expect(close_to(relative_humidity, 56.5f), "raw 0x8000 is 56.5 %RH");
    expect(sht20::fake_written() == std::vector<uint8_t>{0xE5}, "it sent the humidity command");

    // The checksum: the same data, with a corrupted checksum byte, is *no reading*.
    sht20::fake_reset();
    sht20::fake_answer({0x80, 0x00, 0x00});  // the word, but not its checksum
    expect(!sensor.temperature(celsius), "a corrupt checksum reads false");

    // A reply too short to hold both the data and its checksum...
    sht20::fake_reset();
    sht20::fake_answer({0x80});  // one byte, where three were asked for
    expect(!sensor.temperature(celsius), "a reply too short to hold a measurement reads false");

    // ...and no reply at all.
    sht20::fake_reset();  // the device stays silent
    expect(!sensor.humidity(relative_humidity), "an exchange that failed reads false");

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
