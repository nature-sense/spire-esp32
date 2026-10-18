// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 NatureSense

//! CMake build module — CMakeLists.txt analysis and build/test execution.

use async_trait::async_trait;
use std::path::Path;

use super::generic_helpers::parse_source_file_std;
use super::{BuildModuleMessage, BuildOptions, BuildOutput, ModuleCapability, TestOptions};

use super::generic_helpers::{parse_key_value, run_cmd};
use crate::Actor;
use spire_core::build_types::{BuildMetadata, ProjectStructure};

/// Static CMake build module.
pub struct CmakeBuildModule;

impl CmakeBuildModule {
    pub fn new() -> Self {
        Self
    }

    fn analyze(&self, path: &Path) -> Result<BuildMetadata, String> {
        let mut metadata = parse_key_value(path, "CMakeLists.txt", "")?;
        metadata.build_system = "CMake".to_string();
        metadata.project_type = "CMake_project".to_string();
        // An **ESP-IDF** project states its own type in its root `CMakeLists.txt`, and this is where
        // that is read. Analysis of one belongs to this module — its `CMakeLists.txt` *is* a CMake
        // project (see `IdfBuildModule`, which owns only the invocation) — so the marker is read
        // here rather than by a module of its own.
        //
        // Only a root carries the marker, so a component under it stays what it is: a component.
        // `structure_from` returns `None` for every other CMake project, which is what keeps this
        // from claiming the whole of Spire.
        if let Ok(content) = std::fs::read_to_string(path.join("CMakeLists.txt")) {
            if let Some(structure) = crate::build::idf_projects::structure_from(&content) {
                metadata.structure = structure;
                // The chip an ESP-IDF **application** builds for is its **platform target**, and it
                // is recorded here for the same reason the Meson analyzer records its
                // `option('platform', …)` values: the analysis is where a cross-platform project
                // states the platforms it builds for, and every reader — the dashboard's build
                // action, the graph's `Platform` nodes, the router that needs the id to reach the
                // `esp-idf` module — reads this field rather than re-deriving the chip for itself.
                //
                // Only an application names a chip. A component library is built *by* the
                // application that depends on it, and its own `CMakeLists.txt` states a component,
                // not a target — so it records none, even when a `sdkconfig` sits beside it.
                if structure == ProjectStructure::IdfApplication {
                    metadata.platform_targets =
                        crate::build::idf_projects::declared_idf_target(path)
                            .into_iter()
                            .collect();
                }
            }
        }
        Ok(metadata)
    }

    async fn build(&self, path: &Path, _opts: &BuildOptions) -> Result<BuildOutput, String> {
        run_cmd(path, "cmake", &["--build", "build"]).await
    }

    async fn test(&self, path: &Path, _opts: &TestOptions) -> Result<BuildOutput, String> {
        run_cmd(path, "ctest", &["--test-dir", "build"]).await
    }

    /// Run `cmake --build build --target clean`.
    async fn clean(&self, path: &Path, _metadata: &BuildMetadata) -> Result<BuildOutput, String> {
        run_cmd(path, "cmake", &["--build", "build", "--target", "clean"]).await
    }
}

impl Default for CmakeBuildModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Actor for CmakeBuildModule {
    type Message = BuildModuleMessage;

    async fn handle(&mut self, msg: Self::Message) {
        match msg {
            BuildModuleMessage::DescribeCapabilities { reply_to } => {
                let _ = reply_to.send(ModuleCapability {
                    name: "cmake".to_string(),
                    config_files: vec!["CMakeLists.txt".to_string()],
                    build_system: "CMake".to_string(),
                    language: "C++".to_string(),
                    source_extensions: vec![
                        "c".to_string(),
                        "cpp".to_string(),
                        "h".to_string(),
                        "hpp".to_string(),
                        "cmake".to_string(),
                    ],
                    supports_clean: true,
                    supports_lint: false,
                    supports_format: false,
                    supports_fix: false,
                    supports_flash: false,
                    mcp_servers: vec![],
                });
            }
            BuildModuleMessage::Analyze { path, reply_to } => {
                let _ = reply_to.send(self.analyze(&path));
            }
            BuildModuleMessage::Build {
                path,
                opts,
                reply_to,
                ..
            } => {
                let _ = reply_to.send(self.build(&path, &opts).await);
            }

            BuildModuleMessage::BuildStreaming {
                path,
                metadata: _metadata,
                opts,
                build_spec: _,
                event_tx,
                reply_to,
            } => {
                // Not yet streaming for this module — fall back to a batch build
                // and emit a single synthetic "finished" event.
                let result = self.build(&path, &opts).await;
                let _ = event_tx.send(super::BuildEvent {
                    line: format!(
                        "Finished {} in {:?}s",
                        path.display(),
                        result.as_ref().map(|o| o.duration_secs).unwrap_or(0.0)
                    ),
                    level: "finished".to_string(),
                    target: None,
                    file: None,
                    line_number: None,
                    message: None,
                    detail: None,
                });
                let _ = reply_to.send(result);
            }
            BuildModuleMessage::Test {
                path,
                opts,
                reply_to,
                ..
            } => {
                let _ = reply_to.send(self.test(&path, &opts).await);
            }

            BuildModuleMessage::Clean {
                path,
                metadata,
                reply_to,
                ..
            } => {
                let _ = reply_to.send(self.clean(&path, &metadata).await);
            }

            BuildModuleMessage::Lint { reply_to, .. } => {
                let _ = reply_to.send(Err("lint not supported for this module".to_string()));
            }

            BuildModuleMessage::Format { reply_to, .. } => {
                let _ = reply_to.send(Err("format not supported for this module".to_string()));
            }
            BuildModuleMessage::Fix { reply_to, .. } => {
                let _ = reply_to.send(Err("fix not supported for this module".to_string()));
            }
            BuildModuleMessage::LintStreaming { reply_to, .. } => {
                let _ = reply_to.send(Err("lint not supported for this module".to_string()));
            }

            BuildModuleMessage::FixStreaming { reply_to, .. } => {
                let _ = reply_to.send(Err("fix not supported for this module".to_string()));
            }

            BuildModuleMessage::ParseSourceFile {
                file_path,
                reply_to,
            } => {
                let result = parse_source_file_std(&std::path::PathBuf::from(&file_path), "C/C++");
                let _ = reply_to.send(result);
            }

            BuildModuleMessage::ScaffoldBuildConfig {
                project_name,
                goal: _goal,
                platforms: _platforms,
                structure: _structure,
                embedded: _,
                library: _library,
                application: _application,
                reply_to,
            } => {
                let bc = r#"cmake_minimum_required(VERSION 3.16)
project(__P__)
add_executable(__P__ src/main.cpp)
"#
                .replace("__P__", &project_name);
                let sc = r#"#include <iostream>
int main() {
    std::cout << "Hello from __P__!" << std::endl;
    return 0;
}
"#
                .replace("__P__", &project_name);
                let _ = reply_to.send(Ok(super::ScaffoldOutput {
                    build_file: "CMakeLists.txt".to_string(),
                    build_content: bc,
                    source_dir: "src".to_string(),
                    source_file: "main.cpp".to_string(),
                    source_content: sc,
                    ..Default::default()
                }));
            }

            // The manager refuses a flash for a module that declared `supports_flash: false`
            // before routing, so reaching here would mean the capability gate was bypassed —
            // answer rather than fall silent, so the caller gets an error and not a lost channel.
            BuildModuleMessage::Flash { reply_to, .. } => {
                let _ = reply_to.send(Err("flash not supported for this module".to_string()));
            }

            BuildModuleMessage::CallTool { reply_to, .. } => {
                let _ = reply_to
                    .send(serde_json::json!({ "error": "cmake module CallTool not yet wired" }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ESP-IDF application records the chip its tree declares as a **platform target** — the
    /// same field a Meson project states `rpi5` in — which is what lets a dashboard build of a
    /// project scaffolded in an earlier session route to `idf.py` without anyone naming a platform.
    #[test]
    fn an_idf_application_records_the_chip_it_declares() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.16)\n\
             project(spire_test)\n\
             set(SPIRE_PROJECT_STRUCTURE idf_application)\n\
             include($ENV{IDF_PATH}/tools/cmake/project.cmake)\n",
        )
        .unwrap();
        std::fs::write(root.join("sdkconfig"), "CONFIG_IDF_TARGET=\"esp32s3\"\n").unwrap();

        let meta = CmakeBuildModule::new().analyze(root).unwrap();

        assert_eq!(meta.structure, ProjectStructure::IdfApplication);
        assert_eq!(meta.platform_targets, vec!["esp32s3".to_string()]);
    }

    /// A component library names no chip: it is built *by* the application that depends on it, and
    /// a `sdkconfig` beside it does not make it the thing that picks the target.
    #[test]
    fn an_idf_component_library_records_no_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.16)\n\
             project(spire_idf)\n\
             set(SPIRE_PROJECT_STRUCTURE idf_library)\n",
        )
        .unwrap();
        std::fs::write(root.join("sdkconfig"), "CONFIG_IDF_TARGET=\"esp32s3\"\n").unwrap();

        let meta = CmakeBuildModule::new().analyze(root).unwrap();

        assert_eq!(meta.structure, ProjectStructure::IdfLibrary);
        assert!(meta.platform_targets.is_empty());
    }

    /// A plain CMake project stays single-platform even with a stray `sdkconfig` beside it — the
    /// structure marker is what makes a tree ESP-IDF's, not one file in it.
    #[test]
    fn a_plain_cmake_project_records_no_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.16)\nproject(plain)\n",
        )
        .unwrap();
        std::fs::write(root.join("sdkconfig"), "CONFIG_IDF_TARGET=\"esp32s3\"\n").unwrap();

        let meta = CmakeBuildModule::new().analyze(root).unwrap();

        assert_eq!(meta.structure, ProjectStructure::Native);
        assert!(meta.platform_targets.is_empty());
    }
}
