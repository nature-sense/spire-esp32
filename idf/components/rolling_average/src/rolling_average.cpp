// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

#include "rolling_average.hpp"

namespace rolling_average {

bool RollingAverage::init(float* buffer, std::size_t size) {
    if (buffer == nullptr || size == 0) {
        return false;
    }
    buffer_ = buffer;
    size_ = size;
    head_ = 0;
    count_ = 0;
    sum_ = 0.0f;
    return true;
}

bool RollingAverage::push(float sample) {
    if (buffer_ == nullptr || size_ == 0) {
        return false;
    }
    if (count_ < size_) {
        buffer_[head_] = sample;
        sum_ += sample;
        ++count_;
    } else {
        sum_ -= buffer_[head_];
        buffer_[head_] = sample;
        sum_ += sample;
    }
    head_ = (head_ + 1) % size_;
    return true;
}

bool RollingAverage::mean(float& out) const {
    if (buffer_ == nullptr || size_ == 0 || count_ == 0) {
        return false;
    }
    out = sum_ / static_cast<float>(count_);
    return true;
}

bool RollingAverage::reset() {
    if (buffer_ == nullptr || size_ == 0) {
        return false;
    }
    head_ = 0;
    count_ = 0;
    sum_ = 0.0f;
    return true;
}

}  // namespace rolling_average