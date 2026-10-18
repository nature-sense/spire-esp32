// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "st7789.hpp"
#include <st7789_bus.hpp>

namespace st7789 {

St7789::St7789(BusHandle device) : device_(device) {}

// TODO: the protocol. Every branch below is a datasheet question and none of them can be guessed:
//
//   - the command words, and the parameters each one carries;
//   - the order they go out in, and the delay the panel needs between them;
//   - the pixel format the panel is put in, and the addressing window's two corners;
//   - what a panel that stays silent, or answers on a bus that is not wired, looks like.
//
// Talk to the panel through `bus_command` and `bus_data` and nothing else — see `st7789_bus.hpp`,
// which is this component's entire bus interface and the only file in it that names IDF. Which byte
// is a **command** and which is **data** is the whole of the ST7789's conversation; a driver that
// toggles the data/command line itself, rather than saying which a byte is, is one a fake cannot stand
// in for.
//
// Return false rather than logging: the caller knows whether a failure is a panel that is not fitted,
// a bus that is not wired, or a frame worth retrying — and a driver that decides that for it is one
// whose failures cannot be told apart.
//
// Then **prove it**, which is the part that makes this component worth having: a protocol is the one
// layer of a firmware stack that can be tested on a host, against a fake bus recording what went out —
// no panel, no IDF, no hardware. `test/` is already built that way and already runs; the cases in
// `test/st7789_test.cpp` are yours, and a protocol whose cases are written is one that can be changed
// later.

bool St7789::init() {
    // TODO: the power-on sequence, then a `probe`-shaped check — the panel id, if this board wires the
    // panel's SDO, or a status register — rather than trusting that a bus handle exists. An init that
    // returns true because a handle was passed is one that reports success on a board where the panel
    // is not fitted.
    //
    // Until then: `false`. The component's contract is to report what it knows, and a stub knows
    // nothing yet.
    return false;
}

bool St7789::flush(std::uint16_t, std::uint16_t, std::uint16_t, std::uint16_t,
                   const std::uint8_t*) {
    // TODO: set the addressing window — two commands, each with its two corner parameters — then push
    // the pixels as one block of data. `pixels` is big-endian RGB565; a panel left in RGB666 or in
    // swapped byte order is the classic "everything is green" bug, and it is a datasheet answer rather
    // than a guess.
    return false;
}

}  // namespace st7789
