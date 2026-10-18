// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// bmi270's **host test** — the protocol exercised on this machine, with no board and no chip.
//
// `bmi270_bus.hpp` resolves to the fake in this directory, because the seam is included with angle
// brackets and this directory is ahead of the component's `include/` on the include path. So the IMU's
// real registers go in as *bytes* and the registers the protocol asked for come back out. See the
// fake's own comments for the two things it keeps.
//
// The cases below are this device's own, and they are the ones a bench teaches you to keep:
//
//   * the exchange that proves **identity** — a chip ID that reads 0x24 and one that does not;
//   * a sample, with the datasheet's counting: a positive axis, a zero one and a **negative** one, so
//     the sign handling is proven rather than assumed;
//   * the same counts on a **different range**, which is the case a driver that hardcoded one range's
//     scale would fail silently — the numbers move by a factor of eight, and nothing else changes;
//   * a range register holding a value the driver does not know, which is *no reading* and not a guess;
//   * the temperature, on both sides of its 23 °C zero — the second is what proves the word is signed;
//   * and the malformed ones: a reply too short, no reply at all. Those matter most, because they are
//     the ones that only ever happen on a bench.

#include <bmi270_bus.hpp>
#include "bmi270.hpp"

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

/// The datasheet's scale factors are real arithmetic, in `float`, so a value is "the right one" within
/// a tolerance rather than to the last bit.
bool close_to(float value, float expected) {
    return std::fabs(value - expected) < 1e-3f;
}

}  // namespace

int main() {
    bmi270::Bmi270 imu(nullptr);

    // probe(): the part names itself, so the value is checked — identity, not presence. A different
    // Bosch part answering at the address must read false rather than be taken for a BMI270.
    bmi270::fake_reset();
    bmi270::fake_answer({0x24});
    expect(imu.probe(), "probe() accepts a part whose chip ID reads 0x24");
    expect(bmi270::fake_written() == std::vector<uint8_t>{0x00}, "it asked the chip-ID register");

    bmi270::fake_reset();
    bmi270::fake_answer({0x00});  // a device, but not this device
    expect(!imu.probe(), "probe() refuses a chip ID that is not a BMI270");

    bmi270::fake_reset();  // the device stays silent
    expect(!imu.probe(), "probe() fails rather than claiming a device that did not answer");

    // read(): the ranges first, then the twelve bytes of the six axes — and the datasheet's counting.
    //
    // The rounded counts put the arithmetic on clean numbers: at ±2 g one count is 1/16384 g, so 0x4000
    // is 1.0 g and 0xC000 (two's complement) is −1.0 g; at ±2000 °/s one count is 1/16.384 °/s, so
    // 0x4000 is 1000 °/s and 0xE000 is −500 °/s. The middle axis of each is zero, so an axis the driver
    // read the wrong way round would show up as a non-zero where none was sent.
    bmi270::fake_reset();
    bmi270::fake_answer({0x00});  // ACC_RANGE: ±2 g
    bmi270::fake_answer({0x00});  // GYR_RANGE: ±2000 °/s
    bmi270::fake_answer({0x00, 0x40, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x40, 0x00, 0xE0, 0x00, 0x00});
    bmi270::Bmi270::Sample sample{};
    expect(imu.read(sample), "a sample reads true");
    expect(close_to(sample.accel_x, 1.0f) && close_to(sample.accel_y, 0.0f) &&
               close_to(sample.accel_z, -1.0f),
           "0x4000/0x0000/0xC000 is +1 g, 0 g, -1 g");
    expect(close_to(sample.gyro_x, 1000.0f) && close_to(sample.gyro_y, -500.0f) &&
               close_to(sample.gyro_z, 0.0f),
           "0x4000/0xE000/0x0000 is 1000, -500 and 0 degrees per second");
    expect(bmi270::fake_written() == std::vector<uint8_t>{0x41, 0x43, 0x0C},
           "it asked the accel range, the gyro range, then the data");

    // The same counts, a different range: the full scale is the part's *setting*, so the driver reads it
    // rather than assuming. At ±16 g one count is 1/2048 g and at ±125 °/s one is 1/262.144 °/s, so
    // 0x4000 is 8.0 g and 62.5 °/s — eight times the acceleration the same word meant a moment ago.
    bmi270::fake_reset();
    bmi270::fake_answer({0x03});  // ACC_RANGE: ±16 g
    bmi270::fake_answer({0x04});  // GYR_RANGE: ±125 °/s
    bmi270::fake_answer({0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00});
    expect(imu.read(sample), "a sample at a wider range reads true");
    expect(close_to(sample.accel_x, 8.0f) && close_to(sample.gyro_x, 62.5f),
           "the same 0x4000 is 8 g and 62.5 degrees per second at the wider range");

    // A range register holding a value this driver does not know is *no reading*: a count it cannot
    // scale is not a number it may hand over.
    bmi270::fake_reset();
    bmi270::fake_answer({0x07});  // not a range this part has
    bmi270::fake_answer({0x00});
    bmi270::fake_answer({0x00, 0x40, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x40, 0x00, 0xE0, 0x00, 0x00});
    expect(!imu.read(sample), "an unknown range reads false rather than guessing a scale");

    // A data reply too short to hold the twelve bytes it asked for...
    bmi270::fake_reset();
    bmi270::fake_answer({0x00});
    bmi270::fake_answer({0x00});
    bmi270::fake_answer({0x00, 0x40, 0x00, 0x00, 0x00, 0xC0});  // six bytes, where twelve were asked for
    expect(!imu.read(sample), "a reply too short to hold a sample reads false");

    // ...and no reply at all.
    bmi270::fake_reset();  // the device stays silent
    expect(!imu.read(sample), "an exchange that failed reads false");

    // temperature(): the die temperature, on both sides of its 23 °C zero.
    //   raw 0x0400 =  1024 counts →  1024 / 512 + 23 = 25 °C
    //   raw 0xEC00 = -5120 counts → -5120 / 512 + 23 = 13 °C   (the word is signed)
    bmi270::fake_reset();
    bmi270::fake_answer({0x00, 0x04});
    float celsius = 0.0f;
    expect(imu.temperature(celsius), "a temperature measurement reads true");
    expect(close_to(celsius, 25.0f), "raw 0x0400 is 25 degrees Celsius");
    expect(bmi270::fake_written() == std::vector<uint8_t>{0x22}, "it asked the temperature's low byte");

    bmi270::fake_reset();
    bmi270::fake_answer({0x00, 0xEC});
    expect(imu.temperature(celsius) && close_to(celsius, 13.0f),
           "a negative count reads below 23 degrees rather than as a huge positive one");

    bmi270::fake_reset();  // the device stays silent
    expect(!imu.temperature(celsius), "a temperature that will not answer reads false");

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
