// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// The **fake bus** st7789's host test runs against.
//
// Same calls as the real `st7789_bus.hpp` — that is the whole contract, and the reason the protocol
// above them is the same code the firmware runs. Behind them:
//
//   * a **recording** of what the protocol sent, **split by the data/command line**. That split is the
//     point: half of a display driver's bugs are in a byte sent as *data* where the panel wanted a
//     *command*, or the reverse, and a fake that merged the two could not tell those apart. A test
//     asserts against `fake_commands()` and `fake_data()` separately for exactly that reason;
//   * a **recording of the waits**, because a panel's power-on is a sequence *with timing*. A driver
//     that dropped its sleep-out delay would come up on the bench and not in a cold room, and no
//     assertion about the bytes sent could see the difference;
//   * a **refusal switch**, for the case the panel is not there: `fake_refuse()` makes every transfer
//     fail. A write-only seam has no read to fail, so this is the only failure it can be told about —
//     and the facts document states the claim it puts to the test: "the bus refuses a transfer →
//     `false`; `init()` must not claim a panel it never reached". A fake without this call could not
//     express that, and a regenerated test tried to use it anyway.
//
// Globals, because the component's bus interface is free functions. That is the price of a seam this
// thin — and it is why this file lives under `test/`: it is never linked into firmware.

#include <cstddef>
#include <cstdint>
#include <limits>
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

/// Every wait, in milliseconds, in the order the protocol asked for it.
inline std::vector<std::uint32_t>& fake_delays() {
    static std::vector<std::uint32_t> delays;
    return delays;
}

/// Whether the bus is refusing everything — a panel that is not fitted, or not wired.
inline bool& fake_refusing() {
    static bool refusing = false;
    return refusing;
}

/// How many transactions the bus accepts before it starts refusing — "all of them" by default.
inline std::size_t& fake_accepted_before_refusing() {
    static std::size_t count = std::numeric_limits<std::size_t>::max();
    return count;
}

/// Transactions accepted since `fake_reset()` — what `fake_refuse_after()` counts against.
inline std::size_t& fake_accepted() {
    static std::size_t accepted = 0;
    return accepted;
}

/// Whether a transfer is accepted: the bus is not refusing outright, and it has not already answered
/// as many times as it was allowed to.
inline bool fake_accepting() {
    return !fake_refusing() && fake_accepted() < fake_accepted_before_refusing();
}

/// Forget the last test's traffic, and let the bus answer again.
inline void fake_reset() {
    fake_commands().clear();
    fake_data().clear();
    fake_delays().clear();
    fake_refusing() = false;
    fake_accepted_before_refusing() = std::numeric_limits<std::size_t>::max();
    fake_accepted() = 0;
}

/// Make every transfer fail, the way a bus with no device on it does.
inline void fake_refuse() { fake_refusing() = true; }

/// Make the bus accept `count` transfers and then fail every one after that — a device that answered
/// and then stopped, which is the failure a bench actually produces rather than one that never spoke.
inline void fake_refuse_after(std::size_t count) { fake_accepted_before_refusing() = count; }

/// The handle is a token here: nothing below dereferences it.
inline bool bus_command(BusHandle, const uint8_t* out, std::size_t out_len) {
    if (!fake_accepting()) {
        return false;
    }
    ++fake_accepted();
    fake_commands().insert(fake_commands().end(), out, out + out_len);
    return true;
}

inline bool bus_data(BusHandle, const uint8_t* out, std::size_t out_len) {
    if (!fake_accepting()) {
        return false;
    }
    ++fake_accepted();
    fake_data().insert(fake_data().end(), out, out + out_len);
    return true;
}

/// Record the wait rather than performing it: what a test needs to know is that it was asked for, at
/// that point in the sequence, and how long for.
inline void bus_delay_ms(std::uint32_t ms) { fake_delays().push_back(ms); }

}  // namespace st7789
