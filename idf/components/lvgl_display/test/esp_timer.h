// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// The **fake esp_timer** lvgl_display's host test runs against: `esp_timer_get_time()`, the one
// platform fact in the component and the clock behind `lv_tick_set_cb()`.
//
// Monotonic host time rather than a frozen zero, because `tick_cb()` divides it and LVGL's own tick
// arithmetic is written to tolerate a clock that moves. A constant would satisfy the stub and quietly
// misrepresent the thing the callback exists to provide.

#include <chrono>
#include <cstdint>

inline std::int64_t esp_timer_get_time(void) {
    static const auto start = std::chrono::steady_clock::now();
    return std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now() -
                                                                 start)
        .count();
}
