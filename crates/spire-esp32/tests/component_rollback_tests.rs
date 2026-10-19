// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! **A failing edit is undone, byte for byte.**
//!
//! [`CodeDriver::revert`] is the promise the whole unattended path rests on: if verification says the
//! project got worse, the files go back exactly as they were. A batch regeneration found the promise
//! being reported without being kept — a run said `files_reverted` and the next run's baseline failed
//! on the rejected test's own assertion, so the model's bytes were still on disk. The restore now reads
//! itself back; this test is the end-to-end claim around it.
//!
//! Everything here is the real thing except the model: a **copy** of `sps30` (with its own host test),
//! the real `run_code_modify` spine, the real `cmake`/`ctest` gate, and the real apply/verify/revert
//! path. The "model" returns the component's own three files with one planted bug in the source — the
//! Sensirion CRC seed changed to the SHT2x's `0x00` — which **compiles** and is caught by the test. So
//! the rollback is triggered by the test leg, not by a compiler error, which is the shape a silent
//! protocol mistake actually takes.
//!
//! What is asserted afterwards is both halves of the promise: the report says what happened
//! (`files_reverted`, no `files_changed`, the judged observation still saying the test failed), and the
//! disk says the same (all three files byte-identical to the bytes read before the run).

mod common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use common::{copy_component, gate_is_runnable};

use spire_esp32::build::autofix::ErrorsByFile;
use spire_esp32::build::idf_projects::{build_host_test, component_scope, run_host_test_binary};
use spire_esp32::build::modify_code::{run_code_modify, CodeModifyBackend, PlannedChange};

/// The component whose three files are the plan. `sps30`'s host test needs no board and no extra
/// fake, and its seed is the bug this pipeline was hardened for.
const COMPONENT: &str = "sps30";

/// A scripted `modify/code` backend: one plan, the **real** host-test gate, and no target tests.
struct FailingEdit<'a> {
    root: &'a Path,
    /// The rewrites the model would have returned — here, the component's own files with one bug.
    plan: Vec<PlannedChange>,
    /// Whether the last build compiled. `host_tests` refuses to run against a stale binary otherwise —
    /// the same rule the coordinator's backend follows, because a test of code that is no longer there
    /// is worse than no test.
    compiled: Mutex<Option<bool>>,
}

#[async_trait::async_trait]
impl CodeModifyBackend for FailingEdit<'_> {
    async fn plan(&self, _prompt: &str, _scope: &[PathBuf]) -> Option<Vec<PlannedChange>> {
        Some(self.plan.clone())
    }

    async fn build(&self) -> ErrorsByFile {
        match build_host_test(self.root, COMPONENT).await {
            Ok(built) => {
                *self.compiled.lock().unwrap() = Some(built.success);
                if built.success {
                    return ErrorsByFile::new();
                }
                if built.build_errors.is_empty() {
                    // A link error names no file; an empty map would read as a clean build.
                    let mut errors = ErrorsByFile::new();
                    errors.insert(
                        format!("components/{COMPONENT}"),
                        vec![
                            "the host test did not build, and no compiler error named a file"
                                .into(),
                        ],
                    );
                    return errors;
                }
                built.build_errors
            }
            Err(e) => {
                *self.compiled.lock().unwrap() = Some(false);
                let mut errors = ErrorsByFile::new();
                errors.insert(format!("components/{COMPONENT}"), vec![e]);
                errors
            }
        }
    }

    async fn host_tests(&self) -> Option<bool> {
        if *self.compiled.lock().unwrap() != Some(true) {
            return None;
        }
        run_host_test_binary(self.root, COMPONENT)
            .await
            .ok()
            .map(|run| run.success)
    }

    async fn target_tests(&self) -> Option<bool> {
        None
    }
}

/// **The report and the disk agree: the failed edit was rolled back.**
#[tokio::test]
async fn a_failing_edit_is_rolled_back_byte_for_byte() {
    if !gate_is_runnable() {
        eprintln!(
            "no cmake/ctest — the component gate cannot run, so an edit could be neither verified nor \
             rolled back and this test is skipped"
        );
        return;
    }

    let tmp = tempfile::tempdir().expect("temp root");
    let root = tmp.path();
    let component = copy_component(root, COMPONENT);
    let scope = component_scope(root, COMPONENT);
    assert_eq!(
        scope.len(),
        3,
        "a component edit's scope is its header, source and test — one contract in three files: \
         {scope:?}"
    );
    let source = component.join("src").join(format!("{COMPONENT}.cpp"));

    // The bytes before the run, kept to compare against — reading them back *after* would compare the
    // rollback with itself.
    let before: Vec<(PathBuf, Vec<u8>)> = scope
        .iter()
        .map(|path| {
            (
                path.clone(),
                std::fs::read(path).expect("read a file before the run"),
            )
        })
        .collect();
    let pristine = std::fs::read_to_string(&source).expect("read the source");

    // A bug that compiles: the test leg, not the compiler, has to catch it.
    let anchor = "constexpr std::uint8_t kCrcSeed = 0xFF;";
    assert_eq!(
        pristine.matches(anchor).count(),
        1,
        "the seed anchor has moved; this test would otherwise plant nothing"
    );
    let bug = pristine.replacen(anchor, "constexpr std::uint8_t kCrcSeed = 0x00;", 1);

    // The whole scope, as a component edit plans it. Only the source carries the bug; the header and
    // test are planned unchanged because the real path rewrites all three and the rollback has to
    // restore every file it wrote, backed up or not.
    let plan: Vec<PlannedChange> = scope
        .iter()
        .map(|path| PlannedChange {
            file: path.to_string_lossy().to_string(),
            content: if path == &source {
                bug.clone()
            } else {
                std::fs::read_to_string(path).expect("read a planned file")
            },
        })
        .collect();

    let backend = FailingEdit {
        root,
        plan,
        compiled: Mutex::new(None),
    };
    let report = run_code_modify(&backend, "plant a seed bug", &scope, 1).await;

    // ── The report ──
    assert!(
        !report.success,
        "a rolled-back edit did not do what was asked, so it is not a success: {report:#?}"
    );
    assert!(
        report.files_changed.is_empty(),
        "a rejected round keeps nothing: {report:#?}"
    );
    let mut reverted = report.files_reverted.clone();
    reverted.sort();
    let mut expected: Vec<String> = scope
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    expected.sort();
    assert_eq!(
        reverted, expected,
        "every file the round wrote has to be named as reverted: {report:#?}"
    );
    assert_eq!(
        report.host_tests_passed_before,
        Some(true),
        "the baseline is measured, and it passed: {report:#?}"
    );
    assert_eq!(
        report.host_tests_passed_after,
        Some(false),
        "the verdict has to come from the judged round, not from the clean tree the revert left \
         behind: {report:#?}"
    );
    assert_eq!(
        report.build_errors_after, 0,
        "the bug compiles — it is the test, not the compiler, that caught it: {report:#?}"
    );
    assert!(
        report
            .log
            .iter()
            .any(|line| line.contains("rolled back:") && line.contains("FAILED")),
        "the log has to record the test failure the rollback was for: {:#?}",
        report.log
    );
    assert!(
        report.summary().contains("rolled back"),
        "the human summary has to say the change was undone: {}",
        report.summary()
    );

    // ── The disk ──
    for (path, bytes) in &before {
        let now = std::fs::read(path).expect("read a file after the run");
        assert_eq!(
            &now,
            bytes,
            "{} did not come back byte-for-byte after its edit failed the gate",
            path.display()
        );
    }
}
