// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// The **fake bus** st7789's host test runs against.
//
// Same two calls as the real `st7789_bus.hpp` — that is the whole contract, and the reason the protocol
// above them is the same code the firmware runs. Behind them:
//
//   * a **recording** of what the protocol sent, **split by the data/command line**. That split is the
//     point: half of a display driver's bugs are in a byte sent as *data* where the panel wanted a
//     *command*, or the reverse, and a fake that merged the two could not tell those apart. A test
//     asserts against `fake_commands()` and `fake_data()` separately for exactly that reason.
//
// Globals, because the component's bus interface is free functions. That is the price of a seam this
// thin — and it is why this file lives under `test/`: it is never linked into firmware.

#include <cstddef>
#include <cstdint>
#include <vector>

namespace st7789 {

/// The same name as the real header's, and deliberately a different thing: a token. Nothing here
/// dereferences it. A component that reached into the handle would be one this fake could not stand in
/// for — and so one nothing could check without the panel it happens to be wired to.
using BusHandle = void*;

/// Every byte the protocol sent as a **command**, in order.
inline std::vector<uint8_t>& fake_commands() {
    static std::vector<uint8_t> commands;
    return commands;
}

/// Every byte the protocol sent as **data**, in order.
inline std::vector<uint8_t>& fake_data() {
    static std::vector<uint8_t> data;
    return data;
}

/// Forget the last test's traffic.
inline void fake_reset() {
    fake_commands().clear();
    fake_data().clear();
}

/// The handle is a token here: nothing below dereferences it.
inline bool bus_command(BusHandle, const uint8_t* out, std::size_t out_len) {
    fake_commands().insert(fake_commands().end(), out, out + out_len);
    return true;
}

inline bool bus_data(BusHandle, const uint8_t* out, std::size_t out_len) {
    fake_data().insert(fake_data().end(), out, out + out_len);
    return true;
}

}  // namespace st7789
