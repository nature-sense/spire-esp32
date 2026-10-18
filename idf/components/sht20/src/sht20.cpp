// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "sht20.hpp"
#include <sht20_bus.hpp>

#include <cstddef>
#include <cstdint>

namespace sht20 {

// The SHT20's protocol, off the datasheet. The SHT2x family shares it, so this is the whole of the
// conversation — two commands and one checksum:
//
//   0xE3  measure temperature   hold master: the command and the three-byte reply are **one**
//                               exchange, and the sensor stretches the clock while it measures
//   0xE5  measure humidity      the same, for the relative humidity
//   0xE7  read user register    one byte back, and no checksum
//
// (The family also carries 0xF3 and 0xF5 — the same two measurements in *no-hold* mode — and 0xE6 to
// write the user register. Neither is used here: no-hold would split the measurement into two
// exchanges with a wait between them, and this seam deliberately has no delay to wait with; and the
// config surface — resolution, heater, OTP reload — is a separate job from reading the sensor.)
//
// A measurement reply is three bytes, `[MSB, LSB, CRC]`, and the reading is the sixteen-bit word
// `MSB << 8 | LSB`. The conversions are the datasheet's, and the status bits the low byte carries are
// zero in normal operation, so the word is used exactly as it arrives:
//
//   temperature [°C] = -46.85 + 175.72 * raw / 65536
//   humidity    [%]  =  -6.00 + 125.00 * raw / 65536
namespace {

constexpr std::uint8_t kCmdMeasureTemperature = 0xE3;
constexpr std::uint8_t kCmdMeasureHumidity = 0xE5;
constexpr std::uint8_t kCmdReadUserRegister = 0xE7;

constexpr float kTemperatureOffsetC = -46.85f;
constexpr float kTemperatureScaleC = 175.72f / 65536.0f;
constexpr float kHumidityOffsetRh = -6.0f;
constexpr float kHumidityScaleRh = 125.0f / 65536.0f;

/// The SHT2x checksum: CRC-8 with polynomial `x^8 + x^5 + x^4 + 1` (`0x131`, or `0x31` with the
/// leading bit implied), seed `0x00`, most significant bit first, neither reflected nor finished with
/// a final xor. The sensor appends it to every measurement reply, so a frame is good when its data
/// bytes compute back to the checksum byte that arrived with them.
constexpr std::uint8_t kCrcPolynomial = 0x31;

bool crc_matches(const std::uint8_t* data, std::size_t len, std::uint8_t checksum) {
    std::uint8_t crc = 0x00;
    for (std::size_t i = 0; i < len; ++i) {
        crc ^= data[i];
        for (int bit = 0; bit < 8; ++bit) {
            crc = (crc & 0x80) ? static_cast<std::uint8_t>((crc << 1) ^ kCrcPolynomial)
                               : static_cast<std::uint8_t>(crc << 1);
        }
    }
    return crc == checksum;
}

/// Trigger `command` and take the sensor's three-byte reply back, as **one** exchange, writing the
/// sixteen-bit reading to `raw`.
///
/// Returns false for a refused exchange, for a reply too short to hold both the data and its checksum,
/// and for a reply whose checksum does not match — the three ways a measurement fails to be a
/// measurement. A caller retries or gives up on all of them alike, so they are one `false`, not three.
bool measure(BusHandle device, std::uint8_t command, std::uint16_t& raw) {
    std::uint8_t reply[3] = {};
    if (!bus_write_read(device, &command, 1, reply, sizeof(reply))) {
        return false;  // the bus refused, or the device answered with fewer than three bytes
    }
    if (!crc_matches(reply, 2, reply[2])) {
        return false;  // corrupt — a reading that cannot be trusted is not a reading
    }
    raw = static_cast<std::uint16_t>(reply[0] << 8 | reply[1]);
    return true;
}

}  // namespace

Sht20::Sht20(BusHandle device) : device_(device) {}

bool Sht20::probe() {
    // The user register, read as one exchange. This part has no chip-id register, so what this proves
    // is that *something* at the address answers — presence, not identity. The channel that identifies
    // the part is a measurement, and that is where the checksum above earns its keep.
    std::uint8_t user_register = 0;
    const std::uint8_t command = kCmdReadUserRegister;
    return bus_write_read(device_, &command, 1, &user_register, 1);
}

bool Sht20::temperature(float& celsius) {
    std::uint16_t raw = 0;
    if (!measure(device_, kCmdMeasureTemperature, raw)) {
        return false;
    }
    celsius = kTemperatureOffsetC + kTemperatureScaleC * static_cast<float>(raw);
    return true;
}

bool Sht20::humidity(float& relative_humidity) {
    std::uint16_t raw = 0;
    if (!measure(device_, kCmdMeasureHumidity, raw)) {
        return false;
    }
    relative_humidity = kHumidityOffsetRh + kHumidityScaleRh * static_cast<float>(raw);
    return true;
}

}  // namespace sht20
