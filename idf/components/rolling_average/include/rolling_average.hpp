// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#pragma once

#include <cstddef>
#include <cstdint>

namespace rolling_average {

/// A sliding-window average of `float` samples.
///
/// The caller owns the storage: it hands in a buffer of `size` floats, and the object keeps a
/// running sum and a count of how many samples are currently in the window. Push each new sample
/// with `push()` and read the mean of the samples currently in the window with `mean()`.
///
/// This is a library component: it is pure, it holds no global mutable state, it names no IDF type,
/// no actor and no task, and it reports failure by returning `false` rather than by logging.
class RollingAverage {
public:
    /// Bind the object to the caller's buffer.
    ///
    /// `buffer` must be non-null and `size` must be greater than zero. The buffer is not owned and
    /// must outlive this object. Returns `false` if `buffer` is null or `size` is zero.
    bool init(float* buffer, std::size_t size);

    /// Push a new sample into the window, evicting the oldest if the window is full.
    ///
    /// Returns `false` if the object has not been initialised.
    bool push(float sample);

    /// Read the mean of the samples currently in the window.
    ///
    /// Returns `false` if the object has not been initialised or the window is empty; otherwise
    /// writes the mean to `out` and returns `true`.
    bool mean(float& out) const;

    /// Clear the window, discarding every sample currently in it.
    ///
    /// The buffer binding and capacity are kept, so the object stays usable without a new `init()`.
    /// Returns `false` if the object has not been initialised.
    bool reset();

    /// Number of samples currently in the window.
    std::size_t count() const { return count_; }

    /// Capacity of the window, as given to `init()`.
    std::size_t size() const { return size_; }

private:
    float* buffer_ = nullptr;
    std::size_t size_ = 0;
    std::size_t head_ = 0;
    std::size_t count_ = 0;
    float sum_ = 0.0f;
};

}  // namespace rolling_average