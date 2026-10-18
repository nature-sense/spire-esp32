# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# lvgl_display's **host test build** — the component compiled and run on this machine, with no LVGL,
# no panel, no chip and no IDF.
#
# The whole trick is one line below: `test/` goes on the include path *ahead* of the component's own
# `include/`. This component includes three platform headers — `lvgl.h` from its own header, and
# `esp_timer.h` and `esp_heap_caps.h` from its implementation — and all three are included with
# **angle brackets**, so the include path is what resolves them. On this machine that is the fakes in
# this directory; a quoted include would resolve beside the including file and always find the chip's.
#
# Nothing else about the component changes — what is under test is the same translation unit the
# firmware compiles.
#
#   cmake -S <this dir> -B <this dir>/build && cmake --build <this dir>/build && ctest --test-dir <this dir>/build

cmake_minimum_required(VERSION 3.16)
project(lvgl_display_test CXX)

# C++20, not because this component needs it but because the library does: the dataflow framework it
# ships is C++20, so every harness here is built at the same standard as a chip build (ESP-IDF compiles
# C++ at gnu++2b for chip targets).
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

enable_testing()

add_executable(lvgl_display_test
    lvgl_display_test.cpp
    ../src/lvgl_display.cpp
)

# The fakes first — that order *is* the seam — then the component's own headers.
target_include_directories(lvgl_display_test PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
    ${CMAKE_CURRENT_SOURCE_DIR}/../include
)

target_compile_options(lvgl_display_test PRIVATE -Wall -Wextra)

add_test(NAME lvgl_display COMMAND lvgl_display_test)
