# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 NatureSense
#
# rolling_average's **host test** — an ordinary unit test, because this component is ordinary code.
#
# There is no fake and no include-path trickery here, and that is the difference from a driver: a
# driver talks to something that is not there during a test and needs it stood in for; this component
# takes the caller's inputs and returns a value, so the test is a table of inputs and expected outputs.
#
#   cmake -S <this dir> -B <this dir>/build && cmake --build <this dir>/build && ctest --test-dir <this dir>/build

cmake_minimum_required(VERSION 3.16)
project(rolling_average_test CXX)

# C++20, not because this component needs it but because the library does: the dataflow
# framework it ships is C++20, so every harness here is built at the same standard as a chip build
# (ESP-IDF compiles C++ at gnu++2b for chip targets).
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

enable_testing()

add_executable(rolling_average_test
    rolling_average_test.cpp
    ../src/rolling_average.cpp
)

target_include_directories(rolling_average_test PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../include)

target_compile_options(rolling_average_test PRIVATE -Wall -Wextra)

add_test(NAME rolling_average COMMAND rolling_average_test)
