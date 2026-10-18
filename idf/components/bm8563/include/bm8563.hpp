// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <bm8563_bus.hpp>

#include <cstdint>

namespace bm8563 {

/// A point on the calendar, in the terms a caller thinks in — not in the bytes the part stores.
///
/// The fields are the part's own seven: seconds through years, plus the day-of-week register. The
/// **year is the full one**, century included, because the part keeps the century as a bit rather than
/// a number and a caller that has to add 1900 or 2000 itself is a caller doing the driver's job.
///
/// `weekday` is **the part's and nobody else's**: the BM8563 stores a day number without saying which
/// day is zero — that assignment is the application's, by convention, and this driver neither imposes
/// one nor interprets it. A caller that does not care reads the calendar fields and ignores it.
struct DateTime {
    int year = 0;     ///< Full year, century included — 2026, not 26.
    int month = 1;    ///< 1..12.
    int day = 1;      ///< 1..31, the day of the month.
    int weekday = 0;  ///< 0..6, a day number the application assigns; the part defines no mapping.
    int hour = 0;     ///< 0..23.
    int minute = 0;   ///< 0..59.
    int second = 0;   ///< 0..59.
};

/// A **BM8563** real-time clock — the PCF8563-compatible part, over I²C.
///
/// It is a **battery-backed wall clock**: the time it reports is the time that has passed since it was
/// last set, kept by an oscillator of its own that a small cell runs while the board has no power. It
/// counts seconds, minutes, hours, days, weekdays, months and years, and it does arithmetic on none of
/// them: what it holds is a calendar, and what a caller does with a calendar — a schedule, an
/// interval, a difference — is done above this layer, where the clock is.
///
/// What is fixed here is the shape, and the shape is worth keeping:
///
///  * it owns its **bus handle** and takes no board facts — the bus, the address and the pins are the
///    *board's*, supplied by the application. It takes the **handle**, not the address: opening the
///    bus and adding the device at its address is the application's job, and this constructor is
///    handed what comes back from that. A call site passing a bare `0x51` where a handle is wanted
///    does not compile, which is the compiler saying this sentence again;
///  * it names **no actor, no task and no framework type** — a protocol is a library, and what drives
///    it is the product's business;
///  * it reaches the bus through `bus_write` / `bus_read` / `bus_write_read` and nothing else, so it
///    is testable on a host (`test/`) as well as on the chip;
///  * it reports failure by **returning false**, never by logging, so the caller can tell a startup
///    refusal from a reading worth retrying.
///
/// **A time that cannot be trusted is not a time.** The part sets a *voltage-low* flag when its
/// oscillator has stopped at some point since it was last written — a power loss, or a backup cell
/// gone flat — and from then on the registers still read, and they read wrongly. `now()` treats that
/// the way a sensor treats a failed checksum: it returns **false**. It is the one signal a caller
/// needs to tell *the clock is good* from *the clock must be set*, and it is why `set()` is part of
/// this driver rather than something a caller does by poking registers.
///
/// What it is **not**: a network time source. It neither knows the real date nor can learn it; it only
/// remembers one. Where the time comes from — an SNTP client, a console, a default — is the
/// application's decision, handed in through `set()`.
class Bm8563 {
public:
    explicit Bm8563(BusHandle device);

    Bm8563(const Bm8563&) = delete;
    Bm8563& operator=(const Bm8563&) = delete;

    /// Whether the clock is really there: reads a control register and sees whether the device answers.
    ///
    /// The part exposes **no chip-id register** — unlike a panel controller or a flash part, there is no
    /// value here that is this part's and no other's. So this reports **presence**, not identity: a
    /// device answered at the address. What proves the part is a working clock is a reading, which is
    /// why `now()` checks the part's own voltage-low flag.
    ///
    /// An absent part and a bus that will not answer both read **false**: a clock that will not speak is
    /// no more usable than one that is not fitted.
    bool probe();

    /// Reads the clock and writes it to `when`.
    ///
    /// Returns **false** when the exchange failed, **and** when the part says the time is not to be
    /// trusted — its voltage-low flag is set, so the oscillator stopped since the clock was last set
    /// and the fields are wrong. Both are one `false` because the caller does the same thing with each:
    /// reach for the time some other way and call `set()`. A caller that ignores the return value and
    /// reads `when` anyway is reading a time this driver cannot vouch for.
    ///
    /// The whole clock is taken in **one** exchange — the part's address pointer auto-increments, so
    /// seconds through years come back from a single request, as one instant rather than a sequence
    /// that could straddle a tick.
    bool now(DateTime& when);

    /// Sets the clock from `when` and starts it counting from there.
    ///
    /// This is also what makes `now()` trustworthy again: writing the time clears the voltage-low flag
    /// the part set when it lost power, so a caller that sees `now()` fail can set the clock from a
    /// trustworthy source and carry on. Returns **false** when the bus refused the write — in which case
    /// the clock still holds its previous, untrusted value.
    bool set(const DateTime& when);

private:
    BusHandle device_;
};

}  // namespace bm8563
