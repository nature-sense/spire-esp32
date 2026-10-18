// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// ili9342's **seam**: the whole of its interface to the host, and the only place in this component
// that touches a platform API.
//
// Everything above this file is written in terms of these three calls and nothing else. That is what
// makes the protocol testable on a host: `test/` compiles this component against a *fake*
// `ili9342_bus.hpp` with the same three functions, which records what the protocol sent, whether it
// sent it as a command or as data, and what it waited for.
//
// The first two are **write-only** — which is the difference between a display's seam and a sensor's.
// An LCD controller's conversation is one-directional in the common case: the module that wires the
// panel's SDO is the exception, so a driver written against a read it cannot count on is a driver
// that works on one breakout and not on the next.
//
// They are **the data/command line, made callable**. `bus_command` is a byte the panel reads as an
// instruction; `bus_data` is a byte it reads as a parameter or as pixels. *Which* GPIO that line is —
// and whether the bus drives it in a pre-transfer callback or by hand — is the board's, which is why
// this file names neither. A driver that reached past the pair for a `gpio_set_level` would be one
// whose behaviour a fake cannot reproduce, and so one that nothing can check without the display it
// happens to be wired to today.
//
// The third call is the one a display needs and a sensor does not, and it is why this seam is three
// calls rather than two: **a panel's power-on is a sequence with timing, not just bytes.** An ILI9342
// wants a hundred milliseconds after sleep-out before it will take a frame, and the same again after
// the display is switched on. That interval is the *panel's* fact, off its datasheet, exactly like the
// command words — so it arrives here as a call rather than as a sleep buried in the driver, where a
// fake could not see it and no test could check that the wait happened at all. It is a delay and not a
// pin, which is why it is the one entry here that reaches a platform call rather than a bus one.
//
// The **convention the board's bus honours**: a transaction whose `user` is non-null carries data,
// and one whose `user` is null carries a command. A `pre_cb` reads it and puts the data/command line
// where the panel expects before the first clock. The driver states *which* a byte is; the board
// decides *how* to say so.

#include <driver/spi_master.h>
#include <freertos/FreeRTOS.h>
#include <freertos/task.h>

#include <cstddef>
#include <cstdint>

namespace ili9342 {

/// This component's handle to the bus.
///
/// In firmware it **is** the IDF handle for this SPI device — the same type IDF's API takes — and in
/// the host test it is a token that nothing dereferences. The component names `BusHandle` and never an
/// IDF type, which is what lets one header compile in both places and keeps the protocol's public API
/// free of the platform it happens to run on.
using BusHandle = spi_device_handle_t;

/// The marker a board's bus looks for to know a transfer is **data**. The value is arbitrary; the
/// meaning is the convention above, and it is this header's to state because both sides read it here.
constexpr std::uintptr_t kIsData = 1;

/// Send `out_len` bytes the panel reads as an **instruction**.
inline bool bus_command(BusHandle device, const uint8_t* out, std::size_t out_len) {
    spi_transaction_t transaction{};
    transaction.length = out_len * 8;
    transaction.tx_buffer = out;
    transaction.user = nullptr;  // a command: the board's pre_cb leaves the data/command line low
    return spi_device_polling_transmit(device, &transaction) == ESP_OK;
}

/// Send `out_len` bytes the panel reads as **data** (a parameter, or pixel bytes).
inline bool bus_data(BusHandle device, const uint8_t* out, std::size_t out_len) {
    spi_transaction_t transaction{};
    transaction.length = out_len * 8;
    transaction.tx_buffer = out;
    transaction.user = reinterpret_cast<void*>(kIsData);  // data: the pre_cb raises the line
    return spi_device_polling_transmit(device, &transaction) == ESP_OK;
}

/// Wait `ms` milliseconds — the interval a panel's own init table asks for after a command.
///
/// A blocking wait, and deliberately: it happens a handful of times, once, during bring-up, and the
/// panel's datasheet states these as *minimum* intervals rather than as a rate to keep up with. The
/// task yields while it waits, so the rest of the system comes up alongside the panel.
inline void bus_delay_ms(std::uint32_t ms) {
    vTaskDelay(pdMS_TO_TICKS(ms));
}

}  // namespace ili9342
