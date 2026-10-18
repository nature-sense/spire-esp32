# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# lvgl_display — a **library** component: the algorithm that turns LVGL's rendering into a panel's
# pixels, and nothing else.
#
# It names no device and no bus, so nothing here names `st7789`: the panel arrives as a `PixelSink`
# the application builds (`lvgl_display::sink_of(panel)`). That is what keeps the panel a driver, the
# wiring in the application, and this component reusable on any board with any panel.
#
# `REQUIRES lvgl`, because the public header includes `lvgl.h` — and so a consumer of this header
# reads the same LVGL configuration the library was compiled with. `PRIV_REQUIRES esp_timer`, because
# the tick is the one thing in the implementation that belongs to the platform: `lv_tick_set_cb()`
# takes a clock, and IDF has exactly one (`esp_timer_get_time()`).
#
# It does **not** require `st7789`, `ft6336`, `toolkit` or `actors`: a component that needed the
# framework it happened to be used under would be one that could not be used under the other, and a
# display is device- and framework-agnostic.

# What this component **is**, stated rather than guessed — the same rule the project follows with
# `SPIRE_PROJECT_STRUCTURE`. Nothing else in the tree says which kind of component this is: it is an
# algorithm with a seam, not a driver (it owns no chip and no bus) and not an actor (it owns no
# mailbox and no task).
set(SPIRE_COMPONENT_KIND library)

idf_component_register(
    SRCS "src/lvgl_display.cpp"
    INCLUDE_DIRS "include"
    REQUIRES lvgl
    PRIV_REQUIRES esp_timer
)
