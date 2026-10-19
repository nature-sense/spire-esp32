// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **Mutation testing for a component: does its own test notice a planted bug?**
//!
//! The failure this exists for is *shared provenance*. `sps30`'s first test was written from the same
//! reading of the protocol as its implementation, so a wrong CRC seed passed every case: the fixture
//! had been computed with the implementation's seed, and the test and the code were wrong together.
//! Reading the test again cannot find that; only something the source did not produce can.
//!
//! A mutation is that something. The component is copied to a temp tree — the library is not a test
//! fixture and is never written to — and one anchor at a time in its source is replaced with a bug of
//! a kind that **compiles** and should be caught at run time: a checksum seed, a command word, a
//! decoded byte, a settling delay. The test is unchanged. If it passes, the two are still agreeing
//! about something they both got from the same place, and the suite says so.
//!
//! The **control** is the first thing asserted: the copy as shipped builds and its test passes. A
//! machine whose compiler is broken would otherwise make every mutation look caught, which is the
//! same shape of mistake — a check that cannot fail reporting success.
//!
//! Planted bugs are kept to ones that leave the test binary *linkable*: a build failure is not the
//! test catching anything, and this harness treats it as a survivor with the compiler's output
//! attached rather than counting it as a pass.

mod common;

use common::{copy_component, gate_is_runnable};

use spire_esp32::build::idf_projects::{build_host_test, run_host_test_binary};

/// The component whose protocol is checked. `sps30` first: it is pure, its host test needs no fake
/// header beyond its own seam, and it is the component whose seed bug motivated the exercise.
const COMPONENT: &str = "sps30";

/// One planted bug and the exact source text it replaces.
struct Mutation {
    /// What went wrong, in the reviewer's words — printed with the outcome.
    name: &'static str,
    /// The anchor. It has to occur **exactly once**, which the harness checks: an anchor that drifted
    /// or was duplicated would silently mutate nothing, and the table would become a comment.
    from: &'static str,
    /// The bug.
    to: &'static str,
}

/// The mutations, chosen to cover the classes of silent wrongness the protocol can carry.
///
/// Each one is a plausible mistake rather than nonsense: a seed copied from another Sensirion part
/// (`sht20` uses init `0x00`), a command word off by one, a reply trusted because it was read, a byte
/// taken from the checksum instead of the value, and a settling interval dropped because the bench
/// device answered quickly.
const MUTATIONS: &[Mutation] = &[
    Mutation {
        name: "the CRC seed is the SHT2x's 0x00 instead of Sensirion's 0xFF",
        from: "constexpr std::uint8_t kCrcSeed = 0xFF;",
        to: "constexpr std::uint8_t kCrcSeed = 0x00;",
    },
    Mutation {
        name: "the read-measurement command is off by one",
        from: "constexpr std::uint16_t kCmdReadMeasurement = 0x0300;",
        to: "constexpr std::uint16_t kCmdReadMeasurement = 0x0301;",
    },
    Mutation {
        name: "a reply is trusted without verifying its checksums",
        from: "    return all_words_valid(reply, sizeof(reply));\n}",
        to: "    return true;  // planted: the identity reply is trusted unchecked\n}",
    },
    Mutation {
        name: "a float is decoded from the checksum byte instead of the value",
        from: "static_cast<std::uint32_t>(reply[base + 3]) << 8 |",
        to: "static_cast<std::uint32_t>(reply[base + 2]) << 8 |",
    },
    Mutation {
        name: "the device's 20 ms settling interval is dropped",
        from: "constexpr std::uint32_t kStartStopDelayMs = 20;",
        to: "constexpr std::uint32_t kStartStopDelayMs = 0;",
    },
];

/// **Every planted bug is caught by the component's own host test.**
///
/// One run, every mutation, one report at the end — the same shape as the corpus measurement: a harness
/// that stopped at the first survivor would hide the rest, and re-running it after each fix would cost
/// a full build per attempt.
#[tokio::test]
async fn a_planted_bug_makes_the_components_own_test_fail() {
    if !gate_is_runnable() {
        eprintln!(
            "no cmake/ctest — the component gate cannot run, so there is no test to catch a mutation \
             and this test is skipped"
        );
        return;
    }

    let tmp = tempfile::tempdir().expect("temp root");
    let root = tmp.path();
    let component = copy_component(root, COMPONENT);
    let source_path = component.join("src").join(format!("{COMPONENT}.cpp"));
    let pristine = std::fs::read_to_string(&source_path).expect("read the copied source");

    // The control. Without it, a copy that never built (or a `ctest` that always fails for a reason of
    // its own) would make every mutation below look caught.
    let built = build_host_test(root, COMPONENT)
        .await
        .expect("build the copied component");
    assert!(
        built.success,
        "the copy as shipped does not build, so no mutation result below would mean anything:\n{}",
        built.output
    );
    let ran = run_host_test_binary(root, COMPONENT)
        .await
        .expect("run the copied component's test");
    assert!(
        ran.success,
        "the copy as shipped does not pass its own test, so it is the wrong baseline for a \
         mutation:\n{}",
        ran.output
    );

    let mut survivors: Vec<String> = Vec::new();
    for mutation in MUTATIONS {
        let anchors = pristine.matches(mutation.from).count();
        if anchors != 1 {
            survivors.push(format!(
                "{}: its anchor occurs {anchors} times (expected once), so this mutation no longer \
                 plants exactly one bug",
                mutation.name
            ));
            continue;
        }

        std::fs::write(
            &source_path,
            pristine.replacen(mutation.from, mutation.to, 1),
        )
        .expect("write the mutated source");
        let applied = std::fs::read_to_string(&source_path).expect("read the mutated source back");
        assert!(
            applied.contains(mutation.to),
            "the mutation did not reach the disk, so nothing below is a test of it: {}",
            mutation.name
        );

        let built = build_host_test(root, COMPONENT)
            .await
            .expect("build the mutated component");
        if !built.success {
            // A build failure is not the test catching anything. Left as a survivor so the mutation
            // gets replaced by one of the same class that still compiles.
            survivors.push(format!(
                "{}: the planted bug did not compile, so the test never ran:\n{}",
                mutation.name, built.output
            ));
            continue;
        }

        let ran = run_host_test_binary(root, COMPONENT)
            .await
            .expect("run the mutated component's test");
        if ran.success {
            survivors.push(format!(
                "{}: the component's own test **passed** over a planted bug — code, test and fixture \
                 may have been written from one wrong reading.\n  It compiled, so this is a run-time \
                 blind spot rather than a broken mutation:\n{}",
                mutation.name, built.output
            ));
        } else {
            println!("caught: {}", mutation.name);
        }
    }

    // Leave the copy exactly as it arrived, so a failure below cannot be blamed on a previous mutation
    // and a later test that reuses the temp tree (there is none today) would not inherit a bug.
    std::fs::write(&source_path, &pristine).expect("restore the copied source");

    assert!(
        survivors.is_empty(),
        "these planted bugs survived the component's own test:\n  {}\n\nThe test and the source may \
         share one wrong idea; the fix is an independent oracle (a datasheet frame, a capture), not \
         another assertion written from the same reading.",
        survivors.join("\n  ")
    );
}
