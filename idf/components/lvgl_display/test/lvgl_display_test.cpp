// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// lvgl_display's **host test** — the component exercised on this machine, with no LVGL and no panel.
//
// `lvgl.h`, `esp_timer.h` and `esp_heap_caps.h` all resolve to the fakes in this directory, because
// they are included with angle brackets and this directory is ahead of the component's `include/`
// on the include path. What is under test is the real `src/lvgl_display.cpp`, unchanged.
//
// The fake paints the whole frame in bands when `lv_timer_handler()` runs, so `render()` is the path
// this component exists for and the cases below are its contract:
//
//  * **what `init()` tells LVGL** — the panel's geometry, a partial-rendering buffer of
//    `kDrawBufferRows` rows, the sink's callback, `this` as user data, and a display made default;
//  * **the bands** — one pass covers the frame exactly once, top to bottom, in buffer-sized bands
//    with the last clipped to the display, and the arithmetic follows the display rather than a
//    constant copied from the first one;
//  * **the refusal** — a sink that returns false makes `healthy()` false, and it stays false after a
//    later flush succeeds, because one stale band is a screen that is wrong until the next full
//    redraw;
//  * **the lifetime** — the draw buffer is one heap block, released exactly once, and `init()` and
//    `shutdown()` are both idempotent.

#include <esp_heap_caps.h>
#include <lvgl_display.hpp>

#include <cstdint>
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

/// The rectangle a band covered, as the panel would read it.
struct Band {
    std::uint16_t x0;
    std::uint16_t y0;
    std::uint16_t x1;
    std::uint16_t y1;
};

/// The panel's role, reduced to what this component can be wrong about: where and how often a
/// rectangle is offered, and whether it was accepted.
struct RecordingSink {
    std::vector<Band> bands;
    bool refuse = false;

    bool flush(std::uint16_t x0, std::uint16_t y0, std::uint16_t x1, std::uint16_t y1,
               const std::uint8_t*) {
        bands.push_back(Band{x0, y0, x1, y1});
        return !refuse;
    }
};

}  // namespace

int main() {
    using lvgl_display::Display;

    // ---- what init() tells LVGL ------------------------------------------------------------------

    {
        RecordingSink sink;
        Display display(lvgl_display::sink_of(sink), 240, 240);

        expect(display.handle() == nullptr, "handle() is null before init()");
        expect(fake_live_blocks() == 0, "constructing a display allocates nothing");
        display.render();
        expect(sink.bands.empty(), "render() before init() does nothing");
        expect(display.healthy(), "a display that has not rendered is healthy");

        expect(display.init(), "init() brings the display up");
        expect(display.handle() != nullptr, "handle() is the display after init()");
        expect(display.handle() == fake_default_display(),
               "init() makes it the display LVGL draws on");
        expect(display.handle()->hor_res == 240 && display.handle()->ver_res == 240,
               "the display is the panel's geometry");
        expect(display.handle()->user_data == &display,
               "the display is handed back to the flush callback as user data");
        expect(display.handle()->flush_cb != nullptr, "the flush callback is installed");
        expect(fake_tick_cb() != nullptr, "the tick is installed — LVGL has a clock");
        expect(display.handle()->render_mode == LV_DISPLAY_RENDER_MODE_PARTIAL,
               "rendering is in bands, not screen-sized");
        expect(display.handle()->buf_size ==
                   240u * static_cast<std::uint32_t>(Display::kDrawBufferRows) * 2u,
               "the buffer is kDrawBufferRows rows of the panel, two bytes a pixel");
        expect(display.handle()->buf1 != nullptr && display.handle()->buf2 == nullptr,
               "the draw buffer is single, and it is the one the display was given");
        expect(fake_live_blocks() == 1, "init() allocates exactly one block — the draw buffer");

        expect(display.init(), "init() twice succeeds");
        expect(fake_live_blocks() == 1, "init() twice allocates nothing more");

        display.shutdown();
        expect(display.handle() == nullptr, "shutdown() releases the display");
        expect(fake_displays().empty(), "LVGL no longer knows the display");
        expect(fake_default_display() == nullptr, "it is no longer the display LVGL draws on");
        expect(fake_live_blocks() == 0, "shutdown() releases the draw buffer");

        display.shutdown();
        expect(display.handle() == nullptr, "shutdown() twice is a no-op");
        expect(fake_live_blocks() == 0, "shutdown() twice frees nothing twice");
    }


    // ---- one render reaches the sink, band by band ----------------------------------------------

    {
        RecordingSink sink;
        Display display(lvgl_display::sink_of(sink), 240, 240);
        expect(display.init(), "a 240x240 display comes up");

        display.render();

        // 32-row bands over 240 rows: seven full bands and a last one of 16 rows.
        expect(sink.bands.size() == 8, "a 240-row screen of 32-row bands flushes eight times");
        expect(sink.bands.front().x0 == 0 && sink.bands.front().y0 == 0 &&
                   sink.bands.front().x1 == 239 && sink.bands.front().y1 == 31,
               "the first band is the top 32 rows, full width, corners inclusive");
        expect(sink.bands.back().y0 == 224 && sink.bands.back().y1 == 239,
               "the last band is clipped to the display, not overrun");

        bool contiguous = true;
        std::uint16_t next_top = 0;
        for (const Band& band : sink.bands) {
            contiguous = contiguous && band.x0 == 0 && band.x1 == 239 && band.y0 == next_top;
            next_top = static_cast<std::uint16_t>(band.y1 + 1);
        }
        expect(contiguous && next_top == 240, "the bands tile the whole frame, top to bottom");
        expect(display.healthy(), "a sink that accepted every band leaves the display healthy");

        // A second pass is another full repaint, not a continuation of the first.
        sink.bands.clear();
        display.render();
        expect(sink.bands.size() == 8 && sink.bands.front().y0 == 0,
               "every pass starts again at the top");

        display.shutdown();
    }

    // ---- the band arithmetic follows the display, not a constant --------------------------------

    {
        RecordingSink sink;
        Display display(lvgl_display::sink_of(sink), 100, 40);
        expect(display.init(), "a 100x40 display comes up");

        display.render();

        // 32-row bands over 40 rows: one full band and one of 8 rows.
        expect(sink.bands.size() == 2, "a 40-row screen of 32-row bands flushes twice");
        expect(sink.bands[0].x0 == 0 && sink.bands[0].x1 == 99 && sink.bands[0].y0 == 0 &&
                   sink.bands[0].y1 == 31,
               "the full band is this panel's width, not the first panel's");
        expect(sink.bands[1].x0 == 0 && sink.bands[1].x1 == 99 && sink.bands[1].y0 == 32 &&
                   sink.bands[1].y1 == 39,
               "the short last band covers what is left of this panel");

        display.shutdown();
    }

    // ---- a refused band is sticky ---------------------------------------------------------------

    {
        RecordingSink sink;
        Display display(lvgl_display::sink_of(sink), 240, 240);
        expect(display.init(), "the display comes up");
        expect(display.healthy(), "nothing has been refused yet");

        sink.refuse = true;
        display.render();
        expect(!display.healthy(), "a sink that refused a band makes the display unhealthy");
        expect(sink.bands.size() == 8, "the render still offered every band");

        sink.refuse = false;
        display.render();
        expect(!display.healthy(), "health is sticky: a later good flush does not clear it");

        display.shutdown();

        // A fresh display has its own state; nothing carries over from the last one.
        RecordingSink fresh_sink;
        Display fresh(lvgl_display::sink_of(fresh_sink), 240, 240);
        expect(fresh.init() && fresh.healthy(), "a new display starts healthy");
        fresh.shutdown();
    }

    // ---- a display is inert once shutdown -------------------------------------------------------

    {
        RecordingSink sink;
        Display display(lvgl_display::sink_of(sink), 240, 240);
        expect(display.init(), "the display comes up");
        display.shutdown();
        display.render();
        expect(sink.bands.empty(), "render() after shutdown() does nothing");
    }

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}

