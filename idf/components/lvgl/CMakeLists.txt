# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# lvgl — the **vendoring wrapper** for LVGL, the display library an application draws with.
#
# This is not one of the library's product components, and it states no `SPIRE_COMPONENT_KIND`: it
# ships nobody's protocol and nobody's algorithm, it only pins somebody else's. It exists so that LVGL
# is **present and pinned** rather than fetched from a component registry at configure time — a build
# that reaches the network for a dependency is a build that fails on a bench with no wifi.
#
# The upstream source is a **git submodule** at `upstream/`, so the pin is the commit git records for
# it — one place, and one that cannot drift from the source actually built. Until it is checked out
# (`tools/vendor-lvgl.sh`) this component registers **nothing** and the library still configures and
# builds — which is the point. A wrapper that breaks `idf.py build` on a machine that has not fetched
# the submodule would break the library's one promise: that every component compiles.
#
# See `README.md` beside this file for the procedure, and `SPIRE.md` for why a display dependency is
# written down in the architecture at all.

# There is deliberately **no version constant here**. The pin is the submodule's commit, recorded by
# git in this repository; a second copy of it in this file would be a fact that can disagree with the
# source actually compiled. Read the pin back with:
#
#     git submodule status components/lvgl/upstream

if(EXISTS "${CMAKE_CURRENT_LIST_DIR}/upstream/src")
    file(GLOB_RECURSE LVGL_SOURCES
        "${CMAKE_CURRENT_LIST_DIR}/upstream/src/*.c"
        "${CMAKE_CURRENT_LIST_DIR}/upstream/src/*.cpp"
    )

    # LVGL reaches the FreeRTOS tick, the FreeRTOS kernel and its own log through IDF components, and
    # nothing else. Upstream's `esp.cmake` also names `esp_driver_ppa` and `esp_mm`, but only for the
    # PPA renderer, which is off here: `LV_USE_PPA` defaults to 0 and this component deliberately does
    # not hook LVGL's Kconfig into the menu, so every PPA include stays inside `#if LV_USE_PPA` and is
    # never compiled. Turn PPA on in `lv_conf.h` and those two names belong in this list, or the draw
    # unit will not find `<driver/ppa.h>`.
    idf_component_register(
        SRCS ${LVGL_SOURCES}
        INCLUDE_DIRS
            "include"
            "upstream"
            "upstream/src"
        PRIV_REQUIRES esp_timer log freertos
    )

    # `include/lv_conf.h` is **this library's**, and LVGL must be told to find it by bare name rather
    # than by a path relative to `lvgl.h` — which is what this symbol says, and why the header may sit
    # in a directory of our choosing. `PUBLIC`, because a consumer that includes `lvgl.h` has to read
    # the same configuration LVGL was compiled with.
    target_compile_definitions(${COMPONENT_LIB} PUBLIC LV_CONF_INCLUDE_SIMPLE)
else()
    # The submodule is not checked out. An interface component, so the tree still configures and
    # builds — and a missing submodule costs a display, not a failed build.
    idf_component_register(INCLUDE_DIRS "include")
endif()
