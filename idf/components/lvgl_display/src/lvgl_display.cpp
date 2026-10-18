// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include <lvgl_display.hpp>

#include <esp_heap_caps.h>
#include <esp_timer.h>

namespace lvgl_display {
namespace {

/// LVGL's clock, in milliseconds, from the one IDF keeps on every target: `esp_timer`.
///
/// `lv_tick_set_cb()` **is** the tick in v9.6 — a callback read when LVGL asks, rather than a value
/// fed in from a periodic interrupt. So there is no second timer to keep in step with the first, and
/// nothing to call from an ISR; `lv_tick_inc()` has no caller in this library and does not need one.
///
/// It is the only platform fact in this component, and it is the *clock* rather than a pin: every
/// target IDF builds for has `esp_timer`. `uint32_t` wraps after 49 days, which is LVGL's own
/// contract and what `lv_tick_elaps()` is written for.
std::uint32_t tick_cb() {
    return static_cast<std::uint32_t>(esp_timer_get_time() / 1000);
}

}  // namespace

Display::Display(PixelSink sink, std::uint16_t width, std::uint16_t height)
    : sink_(sink), width_(width), height_(height) {}

Display::~Display() { shutdown(); }

bool Display::init() {
    if (display_ != nullptr) {
        return true;
    }

    // Process-global and idempotent: it initialises LVGL's memory pool, its OSAL — which is what
    // makes `lv_lock()` mean anything — and the display list, once. A second `Display` warns and
    // carries on, rather than tearing down the first.
    lv_init();
    lv_tick_set_cb(&tick_cb);

    display_ = lv_display_create(width_, height_);
    if (display_ == nullptr) {
        return false;
    }

    // One band of the panel, in the format `lv_conf.h` names — so the colour depth and this buffer
    // cannot disagree, because it is the same symbol that sizes both.
    //
    // **DMA-capable internal RAM.** The sink's transfer hands these bytes to the SPI driver, which
    // on the S3 requires memory it can read by DMA; internal rather than PSRAM because a DMA out of
    // PSRAM is slower and fussier about alignment, and this buffer is 15 KiB rather than the 150 KiB
    // that would make the question worth asking.
    const std::size_t bytes = static_cast<std::size_t>(width_) * kDrawBufferRows *
                              LV_COLOR_FORMAT_GET_SIZE(LV_COLOR_FORMAT_DEFAULT);
    buffer_ = static_cast<std::uint8_t*>(heap_caps_malloc(bytes, MALLOC_CAP_DMA | MALLOC_CAP_INTERNAL));
    if (buffer_ == nullptr) {
        lv_display_delete(display_);
        display_ = nullptr;
        return false;
    }

    // `this` first: LVGL hands it back in `flush_cb`, and there is no other way for a C callback to
    // reach an instance without a global.
    lv_display_set_user_data(display_, this);
    lv_display_set_flush_cb(display_, &Display::flush_cb);
    lv_display_set_buffers(display_, buffer_, nullptr, bytes, LV_DISPLAY_RENDER_MODE_PARTIAL);

    // LVGL's first display is already the default, so `lv_screen_active()` works without this. It is
    // here because the first display is not the only one a product may make, and because "which
    // screen is the screen" is a decision, not an accident of construction order.
    lv_display_set_default(display_);
    return true;
}

void Display::render() {
    if (display_ == nullptr) {
        return;
    }
    // Takes LVGL's lock itself, and refuses a concurrent call rather than corrupting a drawing —
    // which is why the caller does not lock around this, and does lock around anything else it does
    // with LVGL from a second task.
    lv_timer_handler();
}

void Display::shutdown() {
    if (buffer_ != nullptr) {
        heap_caps_free(buffer_);
        buffer_ = nullptr;
    }
    if (display_ != nullptr) {
        lv_display_delete(display_);
        display_ = nullptr;
    }
}

void Display::flush_cb(lv_display_t* display, const lv_area_t* area, std::uint8_t* pixels) {
    auto* self = static_cast<Display*>(lv_display_get_user_data(display));
    if (self != nullptr && area != nullptr) {
        self->flush(*area, pixels);
    }
    // **Either way.** LVGL waits for this before it can reuse the buffer, so a flush that never says
    // so is a screen that stops redrawing — the failure a hand-written `flush_cb` usually has. The
    // transfer's own failure is kept on the display instead: see `PixelSink` and `healthy()`.
    lv_display_flush_ready(display);
}

void Display::flush(const lv_area_t& area, const std::uint8_t* pixels) {
    const bool sent = sink_.flush(sink_.context, static_cast<std::uint16_t>(area.x1),
                                  static_cast<std::uint16_t>(area.y1),
                                  static_cast<std::uint16_t>(area.x2),
                                  static_cast<std::uint16_t>(area.y2), pixels);
    if (!sent) {
        // Sticky: one dropped band is a screen that is wrong until the next full redraw, and a
        // caller that only saw the last flush succeed would never learn about it.
        flush_failed_ = true;
    }
}

}  // namespace lvgl_display
