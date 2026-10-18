// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "ft6336.hpp"
#include <ft6336_bus.hpp>

#include <cstddef>

namespace ft6336 {

// The FT6336's register map, off the datasheet. The FT6336 is register-compatible with the FT6x36
// family, so the family's map is the whole protocol — a handful of registers and one decode:
//
//   0x02  TD_STATUS   low nibble = how many points are down; 0 is *no finger*, not "no reply"
//   0x03  P1_XH       X[11:8] in bits 3:0 (bits 7:4 carry the event flag and the touch id)
//   0x04  P1_XL       X[7:0]
//   0x05  P1_YH       Y[11:8] in bits 3:0 (bits 7:4 carry the touch id)
//   0x06  P1_YL       Y[7:0]
//   0xA3  CHIP_ID     0x64 on an FT6336U — the id that proves the part is there
//
// A coordinate is twelve bits, high nibble first: `((high & 0x0F) << 8) | low`. That is the whole of
// the decode — the register holds the controller's own grid, and what that grid maps onto (the
// panel's pixels, then a widget) is a question for above this layer.
namespace {

constexpr std::uint8_t kRegTouchStatus = 0x02;
constexpr std::uint8_t kRegPoint1Xh = 0x03;
constexpr std::uint8_t kRegChipId = 0xA3;

/// The FT6336U's own id. The FT6x36 family shares this map, so a board carrying a sibling part is a
/// change to this line and nothing else.
constexpr std::uint8_t kChipIdFt6336u = 0x64;

constexpr std::uint8_t kPointCountMask = 0x0Fu;  ///< the points-down field of TD_STATUS
constexpr std::uint8_t kCoordHighMask = 0x0Fu;   ///< the X/Y[11:8] field of a high byte

/// Point `device` at `reg` and take `len` bytes back, as **one** exchange.
///
/// Two calls would put a STOP between the address and the reply, which is a different transaction that
/// plenty of devices do not answer — which is why `bus_write_read` exists at all, and why the whole of
/// this protocol is built on it.
bool read_register(BusHandle device, std::uint8_t reg, std::uint8_t* in, std::size_t len) {
    return bus_write_read(device, &reg, 1, in, len);
}

}  // namespace

Ft6336::Ft6336(BusHandle device) : device_(device) {}

bool Ft6336::probe() {
    std::uint8_t id = 0;
    if (!read_register(device_, kRegChipId, &id, 1)) {
        // The bus refused the exchange. A caller can do nothing with "the part might be there but
        // would not answer", so a probe reports that the same way it reports an absent part: this is
        // for deciding whether to use the panel at all, and the answer is *no* either way.
        return false;
    }
    return id == kChipIdFt6336u;
}

bool Ft6336::read(Touch& touch) {
    std::uint8_t status = 0;
    if (!read_register(device_, kRegTouchStatus, &status, 1)) {
        return false;  // the exchange failed — which is *not* the same as "no finger", below
    }

    if ((status & kPointCountMask) == 0) {
        touch.down = false;  // read successfully, and the panel simply has nothing on it
        return true;
    }

    // The first point, and only the first. What a second finger means — pinch, two-finger scroll — is
    // a gesture, and gestures are recognised above this layer, where the timing that tells a tap from
    // a hold lives.
    std::uint8_t point[4] = {};
    if (!read_register(device_, kRegPoint1Xh, point, sizeof(point))) {
        return false;
    }

    touch.x = static_cast<std::uint16_t>((point[0] & kCoordHighMask) << 8 | point[1]);
    touch.y = static_cast<std::uint16_t>((point[2] & kCoordHighMask) << 8 | point[3]);
    touch.down = true;
    return true;
}

}  // namespace ft6336
