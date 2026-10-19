// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "sps30.hpp"
#include <sps30_bus.hpp>

#include <bit>
#include <cstddef>
#include <cstdint>

namespace sps30 {

// The SPS30's protocol, off the datasheet and Sensirion's own driver (`embedded-sps`). The whole of
// the conversation, as sixteen-bit big-endian command words:
//
//   0x0010 + 0x0300  start measurement   the argument selects big-endian IEEE754 floats — the device's
//                                        own default; then wait 20 ms before the next command
//   0x0104           stop measurement    then wait 20 ms
//   0x0202           data ready          one checksummed word back: 0x0001 = a sample is waiting
//   0x0300           read measurement    twenty words back: ten big-endian floats, checksummed in pairs
//   0xD033           read serial         sixteen words back: a 32-byte identity string
//
// (The part also has reset (0xD304), sleep/wake, manual fan cleaning, an auto-clean interval, a device
// status register and a firmware-version read. None is used here: they are bring-up and maintenance, a
// separate job from reading the sensor — and reset's own 100 ms wait is not a measurement's.)
//
// **A read is two exchanges, not one.** The SHT2x is hold-master: its command and reply are a single
// exchange (`bus_write_read`), the sensor stretching the clock while it measures. The SPS30 is not:
// its datasheet sends the command, takes a STOP, and *then* reads — `bus_write` followed by
// `bus_read`. Reaching for the wrong form is the classic way a driver works on one device and not the
// next, which is why the seam offers both and this file states which one is wanted here.
//
// Each reply is a run of words, each word three bytes, `[MSB, LSB, CRC]`. The CRC-8 is the Sensirion
// family's — polynomial 0x31 (x^8 + x^5 + x^4 + 1), seed 0xFF, most significant bit first, neither
// reflected nor finished with a final xor — and a reply is good only when every word's two data bytes
// compute back to the checksum byte that arrived after them.

namespace {

constexpr std::uint16_t kCmdStartMeasurement = 0x0010;
/// The start command's argument, and a *format* rather than a command: it asks for big-endian IEEE754
/// floats. It shares its value with the read command's word; the two are unrelated.
constexpr std::uint16_t kArgBigEndianFloat = 0x0300;
constexpr std::uint16_t kCmdStopMeasurement = 0x0104;
constexpr std::uint16_t kCmdDataReady = 0x0202;
constexpr std::uint16_t kCmdReadMeasurement = 0x0300;
constexpr std::uint16_t kCmdReadSerial = 0xD033;

/// The interval the device's datasheet asks for after start and stop, in milliseconds.
constexpr std::uint32_t kStartStopDelayMs = 20;

/// The data-ready word when a sample is waiting; anything else means there is nothing to read yet.
constexpr std::uint16_t kDataReadyFlag = 0x0001;

/// Reply sizes on the wire, in bytes — the serial is sixteen words, a sample twenty.
constexpr std::size_t kSerialReplyBytes = 48;
constexpr std::size_t kMeasurementReplyBytes = 60;
constexpr std::size_t kMeasurementValueCount = 10;

static_assert(kSerialReplyBytes == 16 * 3, "a serial number is sixteen checksummed words");
static_assert(kMeasurementValueCount * 6 == kMeasurementReplyBytes,
              "a sample is ten values of two checksummed words");

/// The Sensirion checksum: CRC-8 with polynomial `x^8 + x^5 + x^4 + 1` (`0x131`, or `0x31` with the
/// leading bit implied), seed `0xFF`, most significant bit first, neither reflected nor finished with a
/// final xor. The device appends it after every sixteen-bit word, so a word is good when its two data
/// bytes compute back to the checksum byte that arrived with them.
constexpr std::uint8_t kCrcPolynomial = 0x31;
constexpr std::uint8_t kCrcSeed = 0xFF;

/// The two bytes of a sixteen-bit word, most significant first — the order the device sends in.
std::uint8_t high_byte(std::uint16_t word) { return static_cast<std::uint8_t>(word >> 8); }
std::uint8_t low_byte(std::uint16_t word) { return static_cast<std::uint8_t>(word & 0xFF); }

bool crc_matches(const std::uint8_t* data, std::size_t len, std::uint8_t checksum) {
    std::uint8_t crc = kCrcSeed;
    for (std::size_t i = 0; i < len; ++i) {
        crc ^= data[i];
        for (int bit = 0; bit < 8; ++bit) {
            crc = (crc & 0x80) ? static_cast<std::uint8_t>((crc << 1) ^ kCrcPolynomial)
                               : static_cast<std::uint8_t>(crc << 1);
        }
    }
    return crc == checksum;
}

/// Whether every three-byte word in `reply` carries its checksum — the whole frame is good, or it is
/// no frame. A single bad word condemns the reply: a value that cannot be trusted is not a value.
bool all_words_valid(const std::uint8_t* reply, std::size_t len) {
    for (std::size_t i = 0; i + 3 <= len; i += 3) {
        if (!crc_matches(reply + i, 2, reply[i + 2])) {
            return false;
        }
    }
    return true;
}

/// Send a sixteen-bit command (big-endian) and take `in_len` bytes back — as **two** exchanges, the
/// way this device reads a register: the command goes out, and the device answers after a STOP.
///
/// Returns false for a refused write, for a refused read, and for a reply too short — the ways a read
/// of a register fails to be one. A caller retries or gives up on all of them alike, so they are one
/// `false`, not three.
bool command_read(BusHandle device, std::uint16_t command, std::uint8_t* in, std::size_t in_len) {
    const std::uint8_t out[2] = {high_byte(command), low_byte(command)};
    if (!bus_write(device, out, sizeof(out))) {
        return false;
    }
    return bus_read(device, in, in_len);
}

/// The float the device encoded in `reply`'s `index`-th value: two big-endian sixteen-bit words, most
/// significant first, each followed by its checksum byte — so the four data bytes of value `i` sit at
/// `6*i + {0, 1, 3, 4}` and the two checksums at `6*i + {2, 5}`.
float value_at(const std::uint8_t* reply, std::size_t index) {
    const std::size_t base = index * 6;
    const std::uint32_t bits = static_cast<std::uint32_t>(reply[base]) << 24 |
                               static_cast<std::uint32_t>(reply[base + 1]) << 16 |
                               static_cast<std::uint32_t>(reply[base + 3]) << 8 |
                               static_cast<std::uint32_t>(reply[base + 4]);
    return std::bit_cast<float>(bits);
}

}  // namespace

Sps30::Sps30(BusHandle device) : device_(device) {}

bool Sps30::probe() {
    // The serial number: sixteen checksummed words, a 32-byte identity string. This part *has* an
    // identity, unlike the SHT2x, so a valid reply is evidence of *which* device answered rather than
    // merely that something did — and a reply with a bad checksum is not that evidence.
    std::uint8_t reply[kSerialReplyBytes] = {};
    if (!command_read(device_, kCmdReadSerial, reply, sizeof(reply))) {
        return false;
    }
    return all_words_valid(reply, sizeof(reply));
}

bool Sps30::start_measurement() {
    // The command and its argument go out as one write — four bytes, all big-endian — and *then* the
    // device's own settling interval is waited out before the next command is allowed to follow.
    const std::uint8_t out[4] = {high_byte(kCmdStartMeasurement), low_byte(kCmdStartMeasurement),
                                 high_byte(kArgBigEndianFloat), low_byte(kArgBigEndianFloat)};
    if (!bus_write(device_, out, sizeof(out))) {
        return false;
    }
    bus_delay_ms(kStartStopDelayMs);
    return true;
}

bool Sps30::stop_measurement() {
    const std::uint8_t out[2] = {high_byte(kCmdStopMeasurement), low_byte(kCmdStopMeasurement)};
    if (!bus_write(device_, out, sizeof(out))) {
        return false;
    }
    bus_delay_ms(kStartStopDelayMs);
    return true;
}

bool Sps30::data_ready() {
    // One checksummed word. "Not ready" and "the bus would not answer" both come back false, and the
    // caller's response to either is the same — do not read, try again next tick — so they are one
    // `false` here rather than two.
    std::uint8_t reply[3] = {};
    if (!command_read(device_, kCmdDataReady, reply, sizeof(reply))) {
        return false;
    }
    if (!crc_matches(reply, 2, reply[2])) {
        return false;
    }
    const std::uint16_t flag = static_cast<std::uint16_t>(reply[0] << 8 | reply[1]);
    return flag == kDataReadyFlag;
}

bool Sps30::read(Measurement& measurement) {
    std::uint8_t reply[kMeasurementReplyBytes] = {};
    if (!command_read(device_, kCmdReadMeasurement, reply, sizeof(reply))) {
        return false;
    }
    if (!all_words_valid(reply, sizeof(reply))) {
        return false;  // a sample that cannot be trusted is not a sample
    }

    // Decode into a local and hand it over only once the whole frame is good, so a failed read never
    // leaves the caller's sample half-written.
    Measurement decoded;
    decoded.mass_pm1 = value_at(reply, 0);
    decoded.mass_pm2_5 = value_at(reply, 1);
    decoded.mass_pm4 = value_at(reply, 2);
    decoded.mass_pm10 = value_at(reply, 3);
    decoded.number_pm0_5 = value_at(reply, 4);
    decoded.number_pm1 = value_at(reply, 5);
    decoded.number_pm2_5 = value_at(reply, 6);
    decoded.number_pm4 = value_at(reply, 7);
    decoded.number_pm10 = value_at(reply, 8);
    decoded.typical_particle_size = value_at(reply, 9);
    measurement = decoded;
    return true;
}

}  // namespace sps30
