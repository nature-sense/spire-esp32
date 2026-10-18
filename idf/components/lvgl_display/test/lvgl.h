// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// The **fake LVGL** lvgl_display's host test runs against.
//
// LVGL is vendored for the chip and compiled by this project's build, so there is no host build of it
// to link a case against. What stands between this component and the panel, though, is a handful of
// v9 calls and one seam — `PixelSink` — and those calls are the whole surface a host case has to
// supply. That is what this file is: the v9 display API, with the real signatures, behind a fake
// that is just enough of a display to make `Display::render()` reach the sink.
//
// It is reached because `lvgl_display.hpp` includes `<lvgl.h>` with **angle brackets**: an
// angle-bracket include is resolved by the include path, and `test/` sits ahead of the component's
// `include/` there. `ili9342_bus.hpp` uses the same trick for the same reason — a quoted include
// would resolve beside the including file and always find the real header sitting next to it.
//
// Everything is `inline` and header-only on purpose. The generated unit host tests compile the
// component's `src/` but not this directory's `.cpp` files, so a fake split into a translation unit
// would link in lvgl_display's own test and fail to link in a unit's. Header-only means one fake
// serves both.
//
// What it *does*, and deliberately no more:
//
//  * a display is a real object: `lv_display_create()` allocates one and keeps the size, the buffer,
//    the render mode, the flush callback and the user data, so a case can read back exactly what
//    the component told LVGL to do;
//  * `lv_timer_handler()` **repaints the whole screen in bands**, the way partial rendering does —
//    `buf_size` bytes at a time, the last band clipped to the display — and offers each band to the
//    display's `flush_cb`. That is the fake's one piece of behaviour, and it is the behaviour under
//    test: what rectangle reaches `PixelSink::flush()`, how many times, and what a refusal does;
//  * nothing is drawn. `px_map` points into the component's own draw buffer and the pixels are
//    whatever was left there; a case that cared about them would be testing LVGL, not this component.

#include <cstddef>
#include <cstdint>
#include <new>
#include <vector>

// The colour format this library's panel speaks, and the one `lv_conf.h` names. Only its *size*
// matters to this component: the draw buffer is sized from it. RGB565 is two bytes a pixel.
#define LV_COLOR_FORMAT_RGB565 0x12
#define LV_COLOR_FORMAT_DEFAULT LV_COLOR_FORMAT_RGB565
#define LV_COLOR_FORMAT_GET_SIZE(cf) 2u

/// Forward-declared first because the flush callback names it, then defined below: opaque to
/// everything outside LVGL on a chip, public here so that a case can read what was configured.
typedef struct _lv_display_t lv_display_t;

/// LVGL's rectangle: corners **inclusive**, which is why the component's conversion to the panel's
/// window is a cast and nothing else.
typedef struct {
    std::int32_t x1;
    std::int32_t y1;
    std::int32_t x2;
    std::int32_t y2;
} lv_area_t;

/// The tick, read when LVGL asks — the v9.6 clock contract `lv_tick_set_cb()` installs.
typedef std::uint32_t (*lv_tick_get_cb_t)(void);

/// What LVGL calls when a rectangle is ready: `px_map` is `area`'s pixels in the display's format.
typedef void (*lv_display_flush_cb_t)(lv_display_t* disp, const lv_area_t* area, std::uint8_t* px_map);

/// Which of the three render modes the buffers belong to. Only `PARTIAL` is used here.
typedef enum {
    LV_DISPLAY_RENDER_MODE_PARTIAL,
    LV_DISPLAY_RENDER_MODE_DIRECT,
    LV_DISPLAY_RENDER_MODE_FULL,
} lv_display_render_mode_t;

/// One display, as much of it as `lvgl_display` touches. The real struct is LVGL's own; this one is
/// public so a case can assert the buffer size, render mode and callback the component chose.
struct _lv_display_t {
    std::int32_t hor_res = 0;
    std::int32_t ver_res = 0;
    void* user_data = nullptr;
    lv_display_flush_cb_t flush_cb = nullptr;
    void* buf1 = nullptr;
    void* buf2 = nullptr;
    std::uint32_t buf_size = 0;
    lv_display_render_mode_t render_mode = LV_DISPLAY_RENDER_MODE_PARTIAL;
};

// ---- the fake's own state --------------------------------------------------------------------
//
// Statics inside inline functions: one per program, shared by `lvgl_display.cpp` and the case that
// includes this header, which is what lets a test look at state the component wrote.

/// Every display `lv_display_create()` has made and `lv_display_delete()` has not taken back.
inline std::vector<lv_display_t*>& fake_displays() {
    static std::vector<lv_display_t*> displays;
    return displays;
}

/// The display LVGL considers active — what `lv_screen_active()` would draw on.
inline lv_display_t*& fake_default_display() {
    static lv_display_t* display = nullptr;
    return display;
}

/// The clock `lv_tick_set_cb()` installed; `nullptr` until a display brings LVGL up.
inline lv_tick_get_cb_t& fake_tick_cb() {
    static lv_tick_get_cb_t cb = nullptr;
    return cb;
}

// ---- the v9 calls `lvgl_display` makes -------------------------------------------------------
//
// Declared with the real signatures, taken from `components/lvgl/upstream/include/lvgl/`, so that a
// call the component would not be able to make on the chip does not compile here either.

inline void lv_init(void) {}

inline void lv_tick_set_cb(lv_tick_get_cb_t cb) { fake_tick_cb() = cb; }

inline lv_display_t* lv_display_create(std::int32_t hor_res, std::int32_t ver_res) {
    auto* display = new (std::nothrow) lv_display_t{};
    if (display == nullptr) {
        return nullptr;
    }
    display->hor_res = hor_res;
    display->ver_res = ver_res;
    fake_displays().push_back(display);
    return display;
}

inline void lv_display_delete(lv_display_t* display) {
    if (display == nullptr) {
        return;
    }
    std::vector<lv_display_t*>& displays = fake_displays();
    for (auto it = displays.begin(); it != displays.end(); ++it) {
        if (*it == display) {
            displays.erase(it);
            break;
        }
    }
    if (fake_default_display() == display) {
        fake_default_display() = nullptr;
    }
    delete display;
}

inline void lv_display_set_user_data(lv_display_t* display, void* user_data) {
    display->user_data = user_data;
}

inline void* lv_display_get_user_data(lv_display_t* display) { return display->user_data; }

inline void lv_display_set_flush_cb(lv_display_t* display, lv_display_flush_cb_t flush_cb) {
    display->flush_cb = flush_cb;
}

inline void lv_display_set_buffers(lv_display_t* display, void* buf1, void* buf2,
                                   std::uint32_t buf_size, lv_display_render_mode_t render_mode) {
    display->buf1 = buf1;
    display->buf2 = buf2;
    display->buf_size = buf_size;
    display->render_mode = render_mode;
}

inline void lv_display_set_default(lv_display_t* display) { fake_default_display() = display; }

/// One pass of the renderer: every live display is repainted from the top, `buf_size` bytes at a
/// time, and each band is offered to the display's `flush_cb`.
///
/// Real LVGL redraws only what is dirty and starts from an empty screen; this fake has no widgets
/// and no dirty tracking, so it redraws the whole frame. That is the smallest thing that exercises
/// the flush path — and it is what makes "a render reaches the sink" assertable at all.
inline std::uint32_t lv_timer_handler(void) {
    // A snapshot: a flush callback is the component's code, and the fake must not be deleted from
    // under a walk of its own list.
    const std::vector<lv_display_t*> displays = fake_displays();
    for (lv_display_t* display : displays) {
        if (display->flush_cb == nullptr || display->buf1 == nullptr || display->hor_res <= 0 ||
            display->ver_res <= 0) {
            continue;
        }

        // Partial rendering flushes what the draw buffer holds: `buf_size` bytes is that many rows,
        // and the last band is clipped to the display. The arithmetic uses the display's own width
        // and the buffer's real size, so a case asserts the bands a panel would actually see.
        const std::uint32_t bytes_per_row = static_cast<std::uint32_t>(display->hor_res) *
                                            LV_COLOR_FORMAT_GET_SIZE(LV_COLOR_FORMAT_DEFAULT);
        std::uint32_t band_rows = display->buf_size / bytes_per_row;
        if (band_rows == 0 || band_rows > static_cast<std::uint32_t>(display->ver_res)) {
            band_rows = static_cast<std::uint32_t>(display->ver_res);
        }

        for (std::int32_t top = 0; top < display->ver_res;
             top += static_cast<std::int32_t>(band_rows)) {
            std::int32_t bottom = top + static_cast<std::int32_t>(band_rows) - 1;
            if (bottom >= display->ver_res) {
                bottom = display->ver_res - 1;
            }
            lv_area_t area{0, top, display->hor_res - 1, bottom};
            display->flush_cb(display, &area, static_cast<std::uint8_t*>(display->buf1));
        }
    }
    return 0;
}

inline void lv_display_flush_ready(lv_display_t*) {}

