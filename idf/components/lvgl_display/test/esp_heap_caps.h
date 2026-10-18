// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

// The **fake heap** lvgl_display's host test runs against: `heap_caps_malloc()`, `heap_caps_free()`,
// and the two capability flags the component asks for.
//
// The flags are chip facts: the sink hands the buffer to an SPI driver that needs memory it can DMA
// from, so `MALLOC_CAP_DMA | MALLOC_CAP_INTERNAL` is a real requirement on an S3 and a name for
// nothing on this machine. They are accepted and ignored here, and the calls fall through to the
// host allocator.
//
// What *is* kept is the count of live blocks, because "the draw buffer is released on shutdown" is a
// contract a host case can check directly and a bench only finds by watching a long-running screen.

#include <cstddef>
#include <cstdint>
#include <cstdlib>

#define MALLOC_CAP_DMA (1u << 0)
#define MALLOC_CAP_INTERNAL (1u << 1)

/// Blocks the fake heap has handed out and not been given back.
inline std::size_t& fake_live_blocks() {
    static std::size_t blocks = 0;
    return blocks;
}

inline void* heap_caps_malloc(std::size_t size, std::uint32_t) {
    void* block = std::malloc(size);
    if (block != nullptr) {
        ++fake_live_blocks();
    }
    return block;
}

inline void heap_caps_free(void* block) {
    if (block != nullptr) {
        --fake_live_blocks();
        std::free(block);
    }
}
