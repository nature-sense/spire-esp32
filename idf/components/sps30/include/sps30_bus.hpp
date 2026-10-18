// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// sps30's **seam**: the whole of its interface to the bus, and the only place in this component
// that touches IDF's driver API.
//
// Everything above this file is written in terms of these calls and nothing else. That is what makes
// the protocol testable on a host: `test/` compiles this component against a *fake* `sps30_bus.hpp`
// with the same functions, which records what the protocol wrote and answers from a script of frames.
// No board, no chip, no IDF, no hardware.
//
// Three **bus** calls, and *which* of them a read uses is the device's own fact. A hold-master sensor
// answers command and reply as **one** exchange (`bus_write_read`) — the SHT2x is read that way, the
// sensor stretching the clock while it measures. The SPS30 is not: its datasheet sends the command,
// takes a STOP, and *then* reads, which is `bus_write` followed by `bus_read`. Reaching for the wrong
// one is the most common way a driver works on one device and not on the next — so both forms are
// here, and the protocol above states which of them this device wants.
//
// The fourth call is the one this device needs and a one-shot sensor does not, and it is why this seam
// is four calls rather than three: **the SPS30's measurement loop has timing.** It asks for twenty
// milliseconds after start and stop before it will take the next command. That interval is the
// *device's* fact, off its datasheet, exactly like the command words — so it arrives here as a call
// rather than as a sleep buried in the driver, where a fake could not see it and no test could check
// that the wait happened at all. It is a delay and not a pin, which is why it is the one entry here
// that reaches a platform call rather than a bus one.
//
// The calls are deliberately narrow. A protocol that reached past them — for a GPIO, a second device
// on the bus — would be one whose behaviour a fake cannot reproduce, and so one that nothing can check
// without the hardware it happens to be wired to today.

#include <driver/i2c_master.h>
#include <freertos/FreeRTOS.h>
#include <freertos/task.h>

#include <cstddef>
#include <cstdint>
#include <vector>

namespace sps30 {

/// This component's handle to the bus.
///
/// In firmware it **is** the IDF handle for this bus — the same type IDF's API takes — and in the
/// host test it is a token that nothing dereferences. The component names `BusHandle` and never an
/// IDF type, which is what lets one header compile in both places and keeps the protocol's public
/// API free of the platform it happens to run on.
using BusHandle = i2c_master_dev_handle_t;

/// Send `out_len` bytes. `false` when the bus refused the transfer.
inline bool bus_write(BusHandle device, const uint8_t* out, std::size_t out_len) {
    return i2c_master_transmit(device, out, out_len, -1) == ESP_OK;
}

/// Receive exactly `in_len` bytes. `false` when the bus returned fewer.
inline bool bus_read(BusHandle device, uint8_t* in, std::size_t in_len) {
    return i2c_master_receive(device, in, in_len, -1) == ESP_OK;
}

/// Send `out_len` bytes and receive `in_len` back, as **one** exchange with the device.
///
/// The hold-master form, and unused by this device (see the header comment): the SPS30 answers a
/// command with a separate read rather than within one exchange.
inline bool bus_write_read(BusHandle device, const uint8_t* out, std::size_t out_len,
                           uint8_t* in, std::size_t in_len) {
    return i2c_master_transmit_receive(device, out, out_len, in, in_len, -1) == ESP_OK;
}

/// Wait `ms` milliseconds — the interval the device's own protocol asks for after start and stop.
///
/// A blocking wait, and deliberately: it happens when the measurement loop is started or stopped, not
/// once per sample, and the datasheet states it as a *minimum* the device needs rather than as a rate
/// to keep up with. The task yields while it waits.
inline void bus_delay_ms(std::uint32_t ms) {
    vTaskDelay(pdMS_TO_TICKS(ms));
}

}  // namespace sps30
