// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// sps30's **host test** — the protocol exercised on this machine, with no board and no chip.
//
// `sps30_bus.hpp` resolves to the fake in this directory, because the seam is included with angle
// brackets and this directory is ahead of the component's `include/` on the include path. So this
// device's real frames go in as *bytes* and what the protocol wrote comes back out. See the fake's own
// comments for the things it keeps.
//
// This device is **stateful**, so the cases below are a loop rather than a set of independent reads:
// start, wait for a sample, take it, stop — and, alongside, the malformed answers a bench teaches you
// to keep: a frame too short, a frame whose checksum does not match, and no answer at all. Those three
// matter most, because they are the ones that only ever happen on a bench.

#include <sps30_bus.hpp>
#include "sps30.hpp"

#include <cmath>
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

/// A sample's values are floats off the wire, so a value is "the right one" within a tolerance rather
/// than to the last bit.
bool close_to(float value, float expected) {
    return std::fabs(value - expected) < 1e-3f;
}

/// Queue a whole frame as an answer. The frames below are *bytes* — written from the datasheet, with
/// their checksums — which is why the test proves the protocol and not the fixture.
void answer(const std::vector<uint8_t>& frame) {
    sps30::fake_script().push_back(frame);
}

/// The serial-number reply: sixteen checksummed words, the 32-byte string "0123456789AB" padded with
/// zeroes. Each word is `[MSB, LSB, CRC]`, so the frame opens `30 31 C7 | 32 33 7C | ...`. The
/// checksum is the Sensirion CRC-8 — polynomial `0x31`, **init `0xFF`** — computed over every word,
/// which is what makes these frames the device's rather than this test's.
const std::vector<uint8_t> kSerialFrame{
    0x30, 0x31, 0xC7, 0x32, 0x33, 0x7C, 0x34, 0x35, 0x80, 0x36, 0x37, 0x3B,
    0x38, 0x39, 0x49, 0x41, 0x42, 0xA3, 0x00, 0x00, 0x81, 0x00, 0x00, 0x81,
    0x00, 0x00, 0x81, 0x00, 0x00, 0x81, 0x00, 0x00, 0x81, 0x00, 0x00, 0x81,
    0x00, 0x00, 0x81, 0x00, 0x00, 0x81, 0x00, 0x00, 0x81, 0x00, 0x00, 0x81,
};

/// A whole sample: ten values, each two checksummed words, built from clean numbers so the arithmetic
/// is checkable by eye — mass 1.0, 2.5, 4.0, 10.0 µg/m³, number 100, 90, 80, 70, 60 #/cm³, size 2.5 µm.
/// Each group of six is `[word0_MSB, word0_LSB, CRC, word1_MSB, word1_LSB, CRC]`, with the same
/// Sensirion CRC-8 (init `0xFF`) as the serial frame.
const std::vector<uint8_t> kSampleFrame{
    0x3F, 0x80, 0xD0, 0x00, 0x00, 0x81, 0x40, 0x20, 0x8E, 0x00, 0x00, 0x81,
    0x40, 0x80, 0x72, 0x00, 0x00, 0x81, 0x41, 0x20, 0x7A, 0x00, 0x00, 0x81,
    0x42, 0xC8, 0x2F, 0x00, 0x00, 0x81, 0x42, 0xB4, 0xAA, 0x00, 0x00, 0x81,
    0x42, 0xA0, 0x2D, 0x00, 0x00, 0x81, 0x42, 0x8C, 0xD6, 0x00, 0x00, 0x81,
    0x42, 0x70, 0x29, 0x00, 0x00, 0x81, 0x40, 0x20, 0x8E, 0x00, 0x00, 0x81,
};

}  // namespace

int main() {
    sps30::Sps30 device(nullptr);

    // ---- probe(): the identity -------------------------------------------------
    // The serial number, sixteen checksummed words. This part *has* an identity, unlike the SHT2x, so a
    // valid reply says *which* device answered, not merely that something did.
    sps30::fake_reset();
    answer(kSerialFrame);
    expect(device.probe(), "probe() accepts the device's own serial number");
    expect(sps30::fake_written() == std::vector<uint8_t>{0xD0, 0x33},
           "it asked for the serial number");

    // The same frame with one checksum spoiled is no identity at all.
    sps30::fake_reset();
    std::vector<uint8_t> corrupt_serial = kSerialFrame;
    corrupt_serial[2] = 0x00;
    answer(corrupt_serial);
    expect(!device.probe(), "a serial frame with a corrupt checksum reads false");

    // A silent bus is no device.
    sps30::fake_reset();
    expect(!device.probe(), "probe() fails rather than claiming a device that did not answer");

    // ---- start / stop: the command, its argument, and the device's own wait -----
    sps30::fake_reset();
    expect(device.start_measurement(), "starting the measurement reads true");
    expect(sps30::fake_written() == std::vector<uint8_t>{0x00, 0x10, 0x03, 0x00},
           "it sent start-measurement with the big-endian float argument");
    expect(sps30::fake_delays() == std::vector<std::uint32_t>{20},
           "it waited the device's 20 ms after starting");

    // A refused command is a failure, and nothing was waited for.
    sps30::fake_reset();
    sps30::fake_refuse();
    expect(!device.start_measurement(), "a refused start-measurement reads false");
    expect(sps30::fake_delays().empty(), "a refused start-measurement waits for nothing");

    // ---- data_ready(): one checksummed word ------------------------------------
    sps30::fake_reset();
    answer({0x00, 0x01, 0xB0});
    expect(device.data_ready(), "a waiting sample reads ready");
    expect(sps30::fake_written() == std::vector<uint8_t>{0x02, 0x02}, "it asked the data-ready flag");

    sps30::fake_reset();
    answer({0x00, 0x00, 0x81});
    expect(!device.data_ready(), "no sample yet reads not-ready");

    // The word is right and its checksum is not — no answer, not a "ready".
    sps30::fake_reset();
    answer({0x00, 0x01, 0x00});
    expect(!device.data_ready(), "a corrupt data-ready flag reads false");

    sps30::fake_reset();
    expect(!device.data_ready(), "a data-ready flag that never answered reads false");

    // ---- read(): the whole sample ----------------------------------------------
    sps30::fake_reset();
    answer(kSampleFrame);
    sps30::Sps30::Measurement sample;
    expect(device.read(sample), "reading a ready sample reads true");
    expect(sps30::fake_written() == std::vector<uint8_t>{0x03, 0x00},
           "it asked for the measured values");
    expect(close_to(sample.mass_pm1, 1.0f), "PM1.0 mass is 1.0 µg/m³");
    expect(close_to(sample.mass_pm2_5, 2.5f), "PM2.5 mass is 2.5 µg/m³");
    expect(close_to(sample.mass_pm4, 4.0f), "PM4.0 mass is 4.0 µg/m³");
    expect(close_to(sample.mass_pm10, 10.0f), "PM10 mass is 10.0 µg/m³");
    expect(close_to(sample.number_pm0_5, 100.0f), "PM0.5 number is 100 #/cm³");
    expect(close_to(sample.number_pm1, 90.0f), "PM1.0 number is 90 #/cm³");
    expect(close_to(sample.number_pm2_5, 80.0f), "PM2.5 number is 80 #/cm³");
    expect(close_to(sample.number_pm4, 70.0f), "PM4.0 number is 70 #/cm³");
    expect(close_to(sample.number_pm10, 60.0f), "PM10 number is 60 #/cm³");
    expect(close_to(sample.typical_particle_size, 2.5f), "the typical particle size is 2.5 µm");

    // A sample whose data is right and whose checksum is not is *no sample* — and the caller's value is
    // left untouched rather than half-decoded.
    sps30::fake_reset();
    std::vector<uint8_t> corrupt_sample = kSampleFrame;
    corrupt_sample[2] = 0x00;
    answer(corrupt_sample);
    sps30::Sps30::Measurement untouched;
    untouched.mass_pm1 = 99.0f;
    expect(!device.read(untouched), "a sample with a corrupt checksum reads false");
    expect(close_to(untouched.mass_pm1, 99.0f), "a failed read leaves the caller's sample untouched");

    // A reply too short to hold a sample...
    sps30::fake_reset();
    answer({0x03});
    expect(!device.read(sample), "a reply too short to hold a sample reads false");

    // ...and no reply at all.
    sps30::fake_reset();
    expect(!device.read(sample), "an exchange that failed reads false");

    // ---- stop ------------------------------------------------------------------
    sps30::fake_reset();
    expect(device.stop_measurement(), "stopping the measurement reads true");
    expect(sps30::fake_written() == std::vector<uint8_t>{0x01, 0x04},
           "it sent the stop-measurement command");
    expect(sps30::fake_delays() == std::vector<std::uint32_t>{20},
           "it waited the device's 20 ms after stopping");

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
