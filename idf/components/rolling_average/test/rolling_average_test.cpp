// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

// rolling_average's **host test** — inputs in, expected values out.
//
// Nothing is stood in for here: this component is code, so its test is a table of cases. That is why
// the same test runs on the host and on the chip — and why it is the gate a change to this component
// has to pass.

#include "rolling_average.hpp"

#include <cstdio>

namespace {

int failures = 0;

void expect(bool ok, const char* what) {
    if (!ok) {
        std::printf("FAIL %s\n", what);
        ++failures;
    }
}

void expect_near(float got, float want, const char* what) {
    const float diff = got - want;
    const float abs_diff = diff < 0.0f ? -diff : diff;
    if (abs_diff > 1e-5f) {
        std::printf("FAIL %s (got %f, want %f)\n", what, got, want);
        ++failures;
    }
}

}  // namespace

int main() {
    // The ordinary case: a window of four, filled with values a caller would really pass.
    {
        float buffer[4] = {0.0f, 0.0f, 0.0f, 0.0f};
        rolling_average::RollingAverage avg;
        expect(avg.init(buffer, 4), "init() accepts a real buffer and size");
        expect(avg.size() == 4, "size() reports the capacity given to init()");
        expect(avg.count() == 0, "a fresh window holds no samples");

        float out = 0.0f;
        expect(!avg.mean(out), "an empty window refuses to report a mean");

        expect(avg.push(1.0f), "push() accepts the first sample");
        expect(avg.count() == 1, "count() tracks the samples in the window");
        expect(avg.mean(out), "mean() succeeds with one sample");
        expect_near(out, 1.0f, "mean of a single sample is that sample");

        expect(avg.push(2.0f), "push() accepts the second sample");
        expect(avg.push(3.0f), "push() accepts the third sample");
        expect(avg.push(4.0f), "push() accepts the fourth sample");
        expect(avg.count() == 4, "count() reaches the window size");
        expect(avg.mean(out), "mean() succeeds with a full window");
        expect_near(out, 2.5f, "mean of 1,2,3,4 is 2.5");
    }

    // The window full: pushing past capacity evicts the oldest sample.
    {
        float buffer[3] = {0.0f, 0.0f, 0.0f};
        rolling_average::RollingAverage avg;
        expect(avg.init(buffer, 3), "init() accepts a three-slot window");

        expect(avg.push(1.0f), "push() 1");
        expect(avg.push(2.0f), "push() 2");
        expect(avg.push(3.0f), "push() 3");

        float out = 0.0f;
        expect(avg.mean(out), "mean() succeeds with a full window");
        expect_near(out, 2.0f, "mean of 1,2,3 is 2");

        expect(avg.push(4.0f), "push() evicts the oldest sample");
        expect(avg.count() == 3, "count() stays at the window size");
        expect(avg.mean(out), "mean() succeeds after eviction");
        expect_near(out, 3.0f, "mean of 2,3,4 is 3");

        expect(avg.push(5.0f), "push() evicts again");
        expect(avg.mean(out), "mean() succeeds after a second eviction");
        expect_near(out, 4.0f, "mean of 3,4,5 is 4");
    }

    // A value at the top of its range: the mean must not overflow or lose the sample.
    {
        float buffer[2] = {0.0f, 0.0f};
        rolling_average::RollingAverage avg;
        expect(avg.init(buffer, 2), "init() accepts a two-slot window");
        expect(avg.push(1.0e30f), "push() accepts a very large sample");
        expect(avg.push(1.0e30f), "push() accepts a second very large sample");

        float out = 0.0f;
        expect(avg.mean(out), "mean() succeeds with large samples");
        expect_near(out, 1.0e30f, "mean of two large samples is that value");
    }

    // reset(): clears the window while keeping the buffer binding and capacity.
    {
        float buffer[3] = {0.0f, 0.0f, 0.0f};
        rolling_average::RollingAverage avg;
        expect(avg.init(buffer, 3), "init() accepts a three-slot window for reset()");

        expect(avg.push(1.0f), "push() 1 before reset()");
        expect(avg.push(2.0f), "push() 2 before reset()");
        expect(avg.push(3.0f), "push() 3 before reset()");
        expect(avg.count() == 3, "count() is full before reset()");

        expect(avg.reset(), "reset() succeeds on an initialised object");
        expect(avg.count() == 0, "reset() empties the window");
        expect(avg.size() == 3, "reset() keeps the capacity");

        float out = 0.0f;
        expect(!avg.mean(out), "mean() refuses after reset()");

        // The object stays usable without a new init().
        expect(avg.push(10.0f), "push() works after reset()");
        expect(avg.count() == 1, "count() tracks samples after reset()");
        expect(avg.mean(out), "mean() succeeds after reset() and a push");
        expect_near(out, 10.0f, "mean after reset() is the new sample");

        // A reset on a full window must also clear the running sum, not just the count.
        expect(avg.push(20.0f), "push() 20 after reset()");
        expect(avg.push(30.0f), "push() 30 after reset()");
        expect(avg.reset(), "reset() succeeds on a full window");
        expect(avg.push(5.0f), "push() 5 after a second reset()");
        expect(avg.mean(out), "mean() succeeds after a second reset()");
        expect_near(out, 5.0f, "mean after a second reset() is the new sample");
    }

    // reset() on an uninitialised object must refuse, like the other methods.
    {
        rolling_average::RollingAverage fresh;
        expect(!fresh.reset(), "reset() refuses before init()");
    }

    // The refusals: a null buffer and a zero size must be rejected, not guessed at.
    {
        rolling_average::RollingAverage avg;
        expect(!avg.init(nullptr, 4), "init() refuses a null buffer");
        expect(!avg.init(nullptr, 0), "init() refuses a null buffer and zero size");

        float buffer[4] = {0.0f, 0.0f, 0.0f, 0.0f};
        expect(!avg.init(buffer, 0), "init() refuses a zero size");

        // An uninitialised object refuses to push or to report a mean.
        rolling_average::RollingAverage fresh;
        expect(!fresh.push(1.0f), "push() refuses before init()");
        float out = 0.0f;
        expect(!fresh.mean(out), "mean() refuses before init()");
    }

    if (failures != 0) {
        std::printf("%d case(s) failed\n", failures);
        return 1;
    }
    std::printf("ok\n");
    return 0;
}
