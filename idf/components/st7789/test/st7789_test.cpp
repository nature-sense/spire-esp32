// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// st7789's **host test** — the protocol exercised on this machine, with no panel and no chip.
//
// `st7789_bus.hpp` resolves to the fake in this directory, because the seam is included with angle
// brackets and this directory is ahead of the component's `include/` on the include path. So what the
// driver *sent* comes back out, already split into commands and data. See the fake's own comments.
//
// The case below is the one every stub passes: a driver whose init does nothing says so, rather than
// claiming the panel is there. **Replace it** with this panel's own cases as they are learned:
//
//   * the command sequence `init()` sends, asserted against `fake_commands()` — the exact words, in
//     order, because a display left in the wrong frame is a fault the hardware will not report;
//   * the addressing window `flush()` sets, with its two corner parameters;
//   * the pixels, and the byte order they went out in — the "everything is green" bug lives here;
//   * and the failures: a bus that refuses, a panel that is not fitted.

#include <st7789_bus.hpp>
#include "st7789.hpp"

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

}  // namespace

int main() {
    st7789::fake_reset();

    st7789::St7789 panel(nullptr);

    // When `init()` starts sending a real sequence, this becomes the case that asserts it:
    //
    //   expect(panel.init(), "init() accepts the panel's own id");
    //   expect(st7789::fake_commands() == std::vector<uint8_t>{0x01, 0x11, 0x3A, 0x29},
    //          "the power-on sequence went out as commands, in order");
    expect(!panel.init(), "an unfilled init() must not claim the panel is there");

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
