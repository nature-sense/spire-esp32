// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// lvgl_display — **the LVGL surface**: one `lv_display`, the draw buffer LVGL renders into, and the
// `flush_cb` that hands each changed rectangle to a **pixel sink** the application supplies.
//
// LVGL is a drawing library, not a display. It renders into memory and calls back with a rectangle
// to put on a panel; the other half — creating the display, sizing and locating the buffer, being
// told the time, and answering with the panel's own bytes — is a handful of v9 calls that every
// product with a screen would otherwise write again and get subtly wrong (a buffer the SPI cannot
// DMA from, a `flush_cb` that forgets to say the flush is done and freezes the screen for good, a
// tick that never advances). It lives here once, as a **library** component: no chip, no bus, no
// pin, no task.
//
// The seam is [`PixelSink`]. `sink_of(panel)` builds one from any device whose `flush()` has the
// panel's signature — `st7789::St7789::flush()` — so `st7789` stays a driver, and the decision that
// *this* panel is what LVGL draws on, on *these* pins, stays where board facts live: the
// application. This component never names a panel, and requires none.
//
// Three things it deliberately does **not** own:
//
//  * **a task.** `render()` is one `lv_timer_handler()` pass. Who calls it, how often and on which
//    task is the application's — normally an actor of its own, which is what `LV_USE_OS
//    LV_OS_FREERTOS` in `lv_conf.h` is for. `lv_timer_handler()` takes LVGL's lock itself, so a
//    second task (the actor that puts a reading on a label) only has to take `lv_lock()` around
//    what it touches;
//  * **the colour format.** It is `LV_COLOR_FORMAT_DEFAULT` in `lv_conf.h` — RGB565, which is what
//    this library's panel speaks — and `init()` does not restate it: a second place to say it is a
//    second place to disagree, and the draw buffer is sized from the same symbol so a change to the
//    format moves the buffer with it;
//  * **the panel's protocol.** `flush()` receives pixels and a rectangle. The power-on sequence,
//    the addressing window and the byte order are the driver's, and they arrive here already done.
//
// A flush **blocks** for as long as the transfer takes: the sink is called inline and LVGL is told
// the flush is done when it returns. That is the whole of the concurrency here, and the seam decides
// it — `st7789_bus.hpp` transmits by polling, so an asynchronous flush would need a completion
// callback it does not have. The cost is stated rather than hidden: while a band transfers, the task
// that called `render()` is inside this call.
//
// **Host test**, and the seam is what makes it possible. LVGL is vendored for the chip and compiled
// by this project's build, so there is no host build of it to link against — but what this component
// uses is a handful of v9 calls and nothing else, so `test/lvgl.h` supplies exactly those. It is reached
// because this header includes `<lvgl.h>` with **angle brackets**: an angle-bracket include is
// resolved by the include path, and `test/` sits ahead of `include/` there. `test/esp_timer.h` and
// `test/esp_heap_caps.h` do the same for the two platform calls, and `test/lvgl_display_test.cpp`
// runs the component on this machine. The sink is a seam one function wide, so the fake only ever
// has to stand in for LVGL — never for a panel.

#include <lvgl.h>

#include <cstddef>
#include <cstdint>

namespace lvgl_display {

/// Where a rectangle of pixels goes: **the seam**.
///
/// One call and one context, no heap and no allocation — a sink is *made*, per board, at startup.
/// The signature is the panel's own: corners inclusive, and `pixels` is `(x1 - x0 + 1) * (y1 - y0 +
/// 1)` pixels in **the display's colour format**, which `lv_conf.h` names — RGB565: two bytes each,
/// big-endian, which is what an ST7789 takes.
///
/// Returning **false** is how a panel that stopped answering is reported. LVGL cannot be told: its
/// `flush_cb` must call `lv_display_flush_ready()` either way, or LVGL waits for a flush that will
/// never be acknowledged and stops redrawing for good. So the failure is *kept* — [`healthy()`] —
/// for the loop that calls `render()` to notice, which is the drivers' division of labour exactly:
/// the layer that knows returns false, and whoever can do something about it decides.
struct PixelSink {
    bool (*flush)(void* context, std::uint16_t x0, std::uint16_t y0, std::uint16_t x1,
                  std::uint16_t y1, const std::uint8_t* pixels);
    void* context;
};

/// Build a sink from any device whose `flush()` has the panel's signature.
///
/// A template rather than a `std::function`, deliberately: what is stored is a function pointer and
/// a context, nothing is allocated at a moment that matters, and the device's *type* is resolved
/// here instead of being named by this component — which is the whole of how `lvgl_display` can
/// drive a panel it has never heard of. The device must outlive the display.
template <class Device>
PixelSink sink_of(Device& device) {
    return PixelSink{
        [](void* context, std::uint16_t x0, std::uint16_t y0, std::uint16_t x1, std::uint16_t y1,
           const std::uint8_t* pixels) {
            return static_cast<Device*>(context)->flush(x0, y0, x1, y1, pixels);
        },
        &device};
}

/// The display LVGL draws on, over a panel it reaches only through a sink.
class Display {
public:
    /// The draw buffer, in **panel rows** — not in bytes, because the width is geometry this
    /// component is already told, and rows keep a buffer proportional to the screen it serves
    /// instead of to a number copied from another board.
    ///
    /// Thirty-two rows of a 240-pixel panel is 15 KiB: enough that the panel is still taking the
    /// last band while LVGL renders the next, which is what partial rendering is for. Rendering
    /// **bands** (`LV_DISPLAY_RENDER_MODE_PARTIAL`) rather than the whole screen is the difference
    /// between this and a screen-sized buffer holding 150 KiB of internal RAM on the S3 — and an S3
    /// is the chip this library's first board carries.
    static constexpr std::size_t kDrawBufferRows = 32;

    /// A display `width` x `height` **pixels** — the panel's geometry, not the board's rotation. A
    /// rotated screen is the same panel drawn in a different order, not a second display.
    Display(PixelSink sink, std::uint16_t width, std::uint16_t height);

    /// Destroys the `lv_display` and releases the buffer, so a display that was stopped and one that
    /// was never stopped both end here.
    ~Display();

    Display(const Display&) = delete;
    Display& operator=(const Display&) = delete;

    /// Bring up LVGL and this display: `lv_init()` (idempotent — LVGL is **process-global**, and a
    /// second display warns rather than re-initialising it), the tick, the display itself, the draw
    /// buffer, and the flush callback.
    ///
    /// Returns **false** rather than logging, like every driver here: the caller knows whether that
    /// was a heap with no room in it or a screen worth trying again. Safe to call twice — the second
    /// call is a no-op.
    bool init();

    /// One `lv_timer_handler()` pass: timers, animations, and the redraw of whatever is dirty, which
    /// reaches the panel through the sink. Call it in a loop — LVGL answers with how long it would
    /// like to sleep, and ignoring that costs a busy screen, not correctness.
    ///
    /// Does nothing before `init()` and after `shutdown()`.
    void render();

    /// Whether every flush so far has reached the panel. A display whose sink returned false is one
    /// whose screen is stale, and this is where that becomes visible — see [`PixelSink`].
    bool healthy() const { return !flush_failed_; }

    /// The `lv_display` this owns, for what this component deliberately does not wrap: the active
    /// screen, a theme, or an `lv_indev` built from a touch panel
    /// (`lv_indev_set_display(..., handle())`). **`nullptr` before `init()`.**
    lv_display_t* handle() const { return display_; }

    /// Destroy the display and release the buffer. Idempotent, and safe on a display that never
    /// initialised.
    void shutdown();

private:
    /// LVGL's callback, and the reason `Display` is not movable: LVGL is given `this` as the
    /// display's user data, so a moved `Display` would leave LVGL pointing at the old address.
    static void flush_cb(lv_display_t* display, const lv_area_t* area, std::uint8_t* pixels);

    /// Hand one rectangle to the sink. LVGL's corners are `int32_t` and the panel's are `uint16_t`;
    /// the cast is safe because LVGL never renders outside the display it was created with.
    void flush(const lv_area_t& area, const std::uint8_t* pixels);

    PixelSink sink_;
    std::uint16_t width_;
    std::uint16_t height_;
    lv_display_t* display_ = nullptr;
    std::uint8_t* buffer_ = nullptr;
    bool flush_failed_ = false;
};

}  // namespace lvgl_display
