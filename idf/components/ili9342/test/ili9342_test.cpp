// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// ili9342's **host test** — the protocol exercised on this machine, with no panel and no chip.
//
// `ili9342_bus.hpp` resolves to the fake in this directory, because the seam is included with angle
// brackets and this directory is ahead of the component's `include/` on the include path. So what the
// driver *sent* comes back out, in order, split by the data/command line, with the waits it asked for
// recorded beside it. See the fake's own comments for the three things it keeps.
//
// The cases below are the ones a display driver lives or dies by, and they are the ones a bench
// teaches you to keep:
//
//   * the **power-on sequence** each revision sends, asserted against the journal — the exact words, in
//     order, because a panel left in the wrong frame is a fault the hardware will not report and the
//     eye reads as "the colours are wrong" three layers up;
//   * the **frame** the panel is left in — the access control, the pixel format — read back out of the
//     journal as the parameters attached to their commands, which is what makes a byte sent as data
//     where a command was wanted a failure and not a coincidence;
//   * the **waits**, because the sequence is a sequence *with timing*: a dropped sleep-out delay passes
//     here and fails on a cold board;
//   * the **addressing window** and the pixels, with the interleaving asserted — each window command
//     followed by its own two corners, and the write by the pixels;
//   * and the failures: a rectangle inside out, no pixels, a bus that refuses from the start, and a bus
//     that answers and then stops — which is the one a bench actually produces.

#include <ili9342_bus.hpp>
#include "ili9342.hpp"

#include <cstdio>
#include <cstdint>
#include <initializer_list>
#include <vector>

namespace {

int failures = 0;

void expect(bool ok, const char* what) {
    if (!ok) {
        std::printf("FAIL %s\n", what);
        ++failures;
    }
}

using Bytes = std::vector<std::uint8_t>;

/// Join byte lists, so an expected sequence can be written as the parts it is made of and still be
/// asserted as one transcript.
Bytes cat(std::initializer_list<Bytes> parts) {
    Bytes joined;
    for (const Bytes& part : parts) {
        joined.insert(joined.end(), part.begin(), part.end());
    }
    return joined;
}

/// The parameters the protocol sent **directly after** `command`, as a bench would read them off a
/// capture: walk the journal, remember the command in flight, and collect the data that follows until
/// the next command. This is what lets a case assert `MADCTL` is `0x08` rather than that `0x08` went out
/// somewhere.
Bytes parameters_of(std::uint8_t command) {
    Bytes found;
    bool in_flight = false;
    for (const ili9342::FakeTransaction& transaction : ili9342::fake_transactions()) {
        if (!transaction.data) {
            in_flight = transaction.bytes.size() == 1 && transaction.bytes[0] == command;
            continue;
        }
        if (in_flight) {
            found.insert(found.end(), transaction.bytes.begin(), transaction.bytes.end());
        }
    }
    return found;
}

}  // namespace

int main() {
    using ili9342::Ili9342;
    using ili9342::Revision;

    // The sequences, written out as the authorities give them rather than recomputed from the driver's
    // own tables — a test that derived them from the source could not notice the source changing.

    // Both revisions: the software reset and the sleep-out, which go out before either table.
    const Bytes power_on = {0x01, 0x11};

    // The power-on block both share: power control, driver timing, VCOM, frame rate, gamma set, both
    // gamma curves, entry mode, display function — Espressif's defaults, and the BSP's E table's first
    // block line for line.
    const Bytes common_block = {
        0xCF, 0xED, 0xE8, 0xCB, 0xF7, 0xF7, 0xEA, 0xC0, 0xC1, 0xC5, 0xC7,
        0xB1, 0xF2, 0x26, 0xE0, 0xE1, 0xB7, 0xB6,
    };

    // The E revision's extra tuning, and the whole of what separates it from the C.
    const Bytes e_block = {0xDD, 0xD5, 0xB1, 0xC8, 0xCB, 0xC9, 0xCA, 0xB7, 0xE4, 0xE5};

    // The frame both are left in, last, so no table above can leave the panel somewhere else.
    const Bytes frame = {0x36, 0x3A, 0x21, 0x29};

    const Bytes c_sequence = cat({power_on, common_block, frame});
    const Bytes e_sequence = cat({power_on, common_block, e_block, frame});

    // ---- init(): the C revision ---------------------------------------------------------------
    //
    // The C is happy with the vendors' stock sequence, so nothing beyond the shared block goes out.
    ili9342::fake_reset();
    Ili9342 c_panel(nullptr, Revision::C);
    expect(c_panel.init(), "init() succeeds on a bus that answers");
    expect(ili9342::fake_commands() == c_sequence,
           "the C revision's power-on sequence went out as commands, in order, and nothing else");
    expect(parameters_of(0x36) == Bytes{0x08},
           "MADCTL is left at 0x08: BGR, no mirror, no swap");
    expect(parameters_of(0x3A) == Bytes{0x55}, "COLMOD is left at 0x55: 16 bits per pixel, RGB565");
    expect(parameters_of(0xCF) == Bytes{0x00, 0xAA, 0xE0},
           "power control B carries the vendor's own values");
    expect(parameters_of(0xE0).size() == 15 && parameters_of(0xE1).size() == 15,
           "both gamma curves go out whole, not truncated");
    expect(ili9342::fake_delays() == std::vector<std::uint32_t>{20, 120, 120},
           "init waits 20ms after the reset and 120ms after sleep-out and display-on");

    // ---- init(): the E revision ---------------------------------------------------------------
    //
    // The same sequence with the E-only block inserted before the frame — and with the shared block
    // still in it, because the E's own table repeats it rather than replacing it.
    ili9342::fake_reset();
    Ili9342 e_panel(nullptr, Revision::E);
    expect(e_panel.init(), "init() succeeds for the E revision");
    expect(ili9342::fake_commands() == e_sequence,
           "the E revision's sequence is the shared block with the E tuning before the frame");
    expect(parameters_of(0xB1) == cat({Bytes{0x00, 0x1B}, Bytes{0x22}}),
           "the E's frame rate control overwrites the shared block's, as the vendor has it");
    expect(parameters_of(0xB7) == cat({Bytes{0x07}, Bytes{0x5A, 0x41, 0x11, 0x19}}),
           "the E's entry mode set overwrites the shared block's, as the vendor has it");
    expect(parameters_of(0xE4).size() == 15 && parameters_of(0xE5).size() == 15,
           "the E's own gamma curves go out whole");
    expect(ili9342::fake_delays() == std::vector<std::uint32_t>{20, 120, 120},
           "the E waits where the C waits: the frame commands are shared, so the waits are too");

    // ---- flush(): the addressing window, then the pixels --------------------------------------
    //
    // Three pixels in a row: red, green, blue — the colours that make a swapped byte order visible.
    const Bytes rgb = {0xF8, 0x00, 0x07, 0xE0, 0x00, 0x1F};

    ili9342::fake_reset();
    expect(c_panel.flush(0x0040, 0x0080, 0x0042, 0x0080, rgb.data()), "flush() succeeds");
    expect(ili9342::fake_commands() == Bytes{0x2A, 0x2B, 0x2C},
           "the window is set by CASET and PASET, and then the pixels are written");
    expect(ili9342::fake_data() == cat({Bytes{0x00, 0x40, 0x00, 0x42},   // x0, x1 — inclusive
                                        Bytes{0x00, 0x80, 0x00, 0x80},   // y0, y1 — inclusive
                                        rgb}),
           "both corners go out high byte first, and the pixels go out untouched");

    const std::vector<ili9342::FakeTransaction>& journal = ili9342::fake_transactions();
    expect(journal.size() == 6 && !journal[0].data && journal[0].bytes == Bytes{0x2A} &&
               journal[1].data && journal[1].bytes == Bytes{0x00, 0x40, 0x00, 0x42} &&
               !journal[2].data && journal[2].bytes == Bytes{0x2B} &&
               journal[3].data && journal[3].bytes == Bytes{0x00, 0x80, 0x00, 0x80} &&
               !journal[4].data && journal[4].bytes == Bytes{0x2C} && journal[5].data &&
               journal[5].bytes == rgb,
           "each window command is followed by its own two corners, and the write by the pixels");
    expect(ili9342::fake_delays().empty(), "flush waits for nothing: the panel is already awake");

    // A band rather than a row: the pixel count is the window's own area, two bytes each.
    ili9342::fake_reset();
    const Bytes band(2 * 3 * 2, 0x5A);  // two rows of three pixels
    expect(c_panel.flush(0x0010, 0x0020, 0x0012, 0x0021, band.data()), "a two-row band flushes");
    expect(ili9342::fake_data() == cat({Bytes{0x00, 0x10, 0x00, 0x12},
                                        Bytes{0x00, 0x20, 0x00, 0x21}, band}),
           "the pixel block is (x1-x0+1) * (y1-y0+1) pixels, two bytes each");

    // ---- the failures -------------------------------------------------------------------------

    // A rectangle inside out, and a buffer with nothing in it: the panel would take either without
    // complaining — a reversed window is one it declares and never fills — so both are refused here.
    ili9342::fake_reset();
    expect(!c_panel.flush(0x0000, 0x0000, 0x0000, 0x0000, nullptr),
           "a flush with no pixels is refused");
    expect(!c_panel.flush(0x0010, 0x0010, 0x0000, 0x0010, rgb.data()),
           "a rectangle inside out is refused");
    expect(ili9342::fake_commands().empty() && ili9342::fake_data().empty(),
           "a refused flush sends nothing at all");

    // A bus that never answers: a write-only seam has no id to read back, so this is all it can say.
    ili9342::fake_reset();
    ili9342::fake_refuse();
    Ili9342 dark_panel(nullptr, Revision::C);
    expect(!dark_panel.init(), "init() reports false when the bus refuses");
    expect(ili9342::fake_commands().empty(), "it stops at the first command the bus refused");

    ili9342::fake_reset();
    ili9342::fake_refuse();
    expect(!c_panel.flush(0x0000, 0x0000, 0x0000, 0x0000, rgb.data()),
           "flush() reports false when the bus refuses");

    // A bus that answers and then stops — the failure a bench actually produces. The reset, the
    // sleep-out and the first power-control command are accepted; its parameters are not.
    ili9342::fake_reset();
    ili9342::fake_refuse_after(3);
    expect(!dark_panel.init(), "init() reports false when the bus stops answering part-way");
    expect(ili9342::fake_commands() == Bytes{0x01, 0x11, 0xCF} && ili9342::fake_data().empty(),
           "it stopped at the transfer that failed, having sent nothing after it");

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
