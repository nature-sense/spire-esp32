// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// axp2101's **seam**: the whole of its interface to the bus, and the only place in this component that
// touches IDF's driver API.
//
// Everything above this file is written in terms of these three calls and nothing else. That is what
// makes the protocol testable on a host: `test/` compiles this component against a *fake*
// `axp2101_bus.hpp` with the same three functions, which records what the protocol wrote and answers
// from a script of frames. No board, no chip, no IDF, no hardware.
//
// Three calls, because this device is read the one way a **register** is read: the address pointer goes
// out and the value comes back as **one** exchange (`bus_write_read`), and the part's pointer
// auto-increments, so a two-byte ADC value — a high register and the low one beside it — is a single
// exchange rather than two that could straddle a conversion. Splitting that into a write and a read is
// the most common way a driver works on one device and not on the next: on I²C the two halves are
// separated by a STOP, which is a different transaction that plenty of devices do not answer.
//
// The three are deliberately narrow. A protocol that reached past them — for a delay, a GPIO, a second
// device on the bus — would be one whose behaviour a fake cannot reproduce, and so one that nothing can
// check without the hardware it happens to be wired to today. This device needs no delay: its status
// and ADC registers are readable whenever the part is powered.

#include <driver/i2c_master.h>

#include <cstddef>
#include <cstdint>

namespace axp2101 {

/// This component's handle to the bus.
///
/// In firmware it **is** the IDF handle for this bus — the same type IDF's API takes — and in the host
/// test it is a token that nothing dereferences. The component names `BusHandle` and never an IDF type,
/// which is what lets one header compile in both places and keeps the protocol's public API free of the
/// platform it happens to run on.
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
/// The form a register read uses: the address pointer goes out and the value comes back without an
/// intervening STOP, which is what lets a two-byte ADC value be taken as one conversion.
inline bool bus_write_read(BusHandle device, const uint8_t* out, std::size_t out_len,
                           uint8_t* in, std::size_t in_len) {
    return i2c_master_transmit_receive(device, out, out_len, in, in_len, -1) == ESP_OK;
}

}  // namespace axp2101
