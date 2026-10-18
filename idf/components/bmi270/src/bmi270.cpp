// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "bmi270.hpp"
#include <bmi270_bus.hpp>

#include <cstddef>
#include <cstdint>

namespace bmi270 {

// The part's protocol, off the datasheet — and it is only the **reading** half of it. Every register
// below is fetched and decoded; nothing here is written, because how a board brings its IMU up is the
// board's decision and not a driver's (see the header):
//
//   0x00  CHIP_ID       the part's identity: **0x24** is a BMI270 and nothing else is
//   0x0C  ACC_X_LSB     twelve bytes of data, LSB first, each axis a signed 16-bit little-endian count:
//         … 0x17        accel x, y, z then gyro x, y, z — adjacent, and the pointer auto-increments
//         GYR_Z_MSB
//   0x22  TEMP_LSB      the die temperature, the same little-endian word: 0x0000 is 23 °C and 512
//        0x23 TEMP_MSB  counts is one degree, so temp = raw / 512 + 23
//   0x41  ACC_RANGE     the accelerometer's full scale: 0x00 ±2 g, 0x01 ±4 g, 0x02 ±8 g, 0x03 ±16 g
//   0x43  GYR_RANGE     the gyroscope's full scale: 0x00 ±2000 °/s, 0x01 ±1000, 0x02 ±500, 0x03 ±250,
//                       0x04 ±125 — each a different number of counts per degree per second
//
// The count-to-unit scale is the part's own and differs per range, so it is a **table** and not a
// constant: the same raw 16384 is 1.0 g at ±2 g and 8.0 g at ±16 g. That is why `read()` asks the part
// which range it is on rather than assuming — and why an unknown range is a failure rather than a
// guess.
//
// The counts are **two's complement**, so the high byte is read as the signed word it is rather than as
// an unsigned magnitude: a part lying upside down reports negative g, and a driver that forgot the sign
// would report the same number for right-way-up and upside-down.
namespace {

constexpr std::uint8_t kRegChipId = 0x00;
constexpr std::uint8_t kRegAccelData = 0x0C;
constexpr std::uint8_t kRegTemperature = 0x22;
constexpr std::uint8_t kRegAccelRange = 0x41;
constexpr std::uint8_t kRegGyroRange = 0x43;

constexpr std::uint8_t kChipId = 0x24;

/// Counts per g, indexed by the accelerometer's range register — ±2, ±4, ±8, ±16 g.
constexpr float kAccelCountsPerG[] = {16384.0f, 8192.0f, 4096.0f, 2048.0f};

/// Counts per degree per second, indexed by the gyroscope's range register — ±2000 … ±125 °/s.
constexpr float kGyroCountsPerDps[] = {16.384f, 32.768f, 65.536f, 131.072f, 262.144f};

/// The die temperature's word: one degree is 512 counts and 0x0000 is 23 °C.
constexpr float kTemperatureCountsPerDegree = 512.0f;
constexpr float kTemperatureOffsetCelsius = 23.0f;

/// Read one register, as one exchange: the register address goes out and the value comes back.
bool read_byte(BusHandle device, std::uint8_t reg, std::uint8_t& value) {
    return bus_write_read(device, &reg, 1, &value, 1);
}

/// Read `count` consecutive registers, as one exchange — the form the twelve bytes of a sample and the
/// two of a temperature use, since the part's address pointer auto-increments.
bool read_bytes(BusHandle device, std::uint8_t reg, std::uint8_t* out, std::size_t count) {
    return bus_write_read(device, &reg, 1, out, count);
}

/// One axis: two bytes, LSB first, read as the signed word they are.
std::int16_t to_word(const std::uint8_t* bytes) {
    return static_cast<std::int16_t>(static_cast<std::uint16_t>(bytes[0]) |
                                     static_cast<std::uint16_t>(bytes[1]) << 8);
}

}  // namespace

Bmi270::Bmi270(BusHandle device) : device_(device) {}

bool Bmi270::probe() {
    // The part names itself, so this is identity and not presence: a different Bosch part answering at
    // the address reads false rather than being taken for a BMI270.
    std::uint8_t chip_id = 0;
    if (!read_byte(device_, kRegChipId, chip_id)) {
        return false;
    }
    return chip_id == kChipId;
}

bool Bmi270::read(Sample& sample) {
    // The ranges first: the full scale is the board's setting, and the raw counts mean nothing without
    // it. Reading the part beats assuming the reset default, which a board is free to have changed.
    std::uint8_t accel_range = 0;
    std::uint8_t gyro_range = 0;
    if (!read_byte(device_, kRegAccelRange, accel_range) ||
        !read_byte(device_, kRegGyroRange, gyro_range)) {
        return false;
    }
    constexpr std::size_t kAccelRanges = sizeof(kAccelCountsPerG) / sizeof(kAccelCountsPerG[0]);
    constexpr std::size_t kGyroRanges = sizeof(kGyroCountsPerDps) / sizeof(kGyroCountsPerDps[0]);
    if (accel_range >= kAccelRanges || gyro_range >= kGyroRanges) {
        return false;  // a range this driver does not know is a number it cannot scale
    }

    // The twelve bytes of the six axes, in one exchange: adjacent registers, one instant.
    std::uint8_t data[12] = {};
    if (!read_bytes(device_, kRegAccelData, data, sizeof(data))) {
        return false;
    }

    const float counts_per_g = kAccelCountsPerG[accel_range];
    const float counts_per_dps = kGyroCountsPerDps[gyro_range];
    sample.accel_x = static_cast<float>(to_word(&data[0])) / counts_per_g;
    sample.accel_y = static_cast<float>(to_word(&data[2])) / counts_per_g;
    sample.accel_z = static_cast<float>(to_word(&data[4])) / counts_per_g;
    sample.gyro_x = static_cast<float>(to_word(&data[6])) / counts_per_dps;
    sample.gyro_y = static_cast<float>(to_word(&data[8])) / counts_per_dps;
    sample.gyro_z = static_cast<float>(to_word(&data[10])) / counts_per_dps;
    return true;
}

bool Bmi270::temperature(float& celsius) {
    // The die temperature, the same little-endian word: signed like the axes, so a part colder than
    // 23 °C reports a negative count rather than a huge positive one.
    std::uint8_t pair[2] = {};
    if (!read_bytes(device_, kRegTemperature, pair, sizeof(pair))) {
        return false;
    }
    celsius = static_cast<float>(to_word(pair)) / kTemperatureCountsPerDegree +
              kTemperatureOffsetCelsius;
    return true;
}

}  // namespace bmi270
