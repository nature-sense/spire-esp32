# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# ft6336's **host test build** — the component compiled and run on this machine, with no panel, no chip
# and no IDF.
#
# The whole trick is one line below: `test/` goes on the include path *ahead* of the component's own
# `include/`. The seam is the one thing included with **angle brackets** — `<ft6336_bus.hpp>` — and
# that is deliberate, because an angle-bracket include is resolved by the include path and a quoted one
# is resolved beside the including file. A quoted seam would always find the real header sitting next to
# `ft6336.hpp`, and this build would be a firmware build with a host compiler.
#
# Nothing else about the component changes — the protocol under test is the same translation unit the
# firmware compiles.
#
#   cmake -S <this dir> -B <this dir>/build && cmake --build <this dir>/build && ctest --test-dir <this dir>/build

cmake_minimum_required(VERSION 3.16)
project(ft6336_test CXX)

# C++20, not because this component needs it but because the library does: the dataflow framework it
# ships is C++20, so every harness here is built at the same standard as a chip build (ESP-IDF compiles
# C++ at gnu++2b for chip targets).
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

enable_testing()

add_executable(ft6336_test
    ft6336_test.cpp
    ../src/ft6336.cpp
)

# The fake first — that order *is* the seam — then the component's own headers.
target_include_directories(ft6336_test PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}
    ${CMAKE_CURRENT_SOURCE_DIR}/../include
)

target_compile_options(ft6336_test PRIVATE -Wall -Wextra)

add_test(NAME ft6336 COMMAND ft6336_test)
