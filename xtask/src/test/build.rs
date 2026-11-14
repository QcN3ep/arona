// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::super::{
    BuildOptions, Initramfs, Target, build_component, build_std, cargo_command, linux_test,
    package_init, target_directory,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) struct Build {
    pub root: PathBuf,
    pub target: Target,
    pub release: bool,
    programs: BTreeMap<String, PathBuf>,
}

impl Build {
    pub fn new(root: PathBuf, target: Target, release: bool) -> Self {
        Self {
            root,
            target,
            release,
            programs: BTreeMap::new(),
        }
    }

    pub fn image(&mut self, name: &str) -> Result<PathBuf, String> {
        let mut command = cargo_command(self.root.clone());
        build_std(&mut command);
        command.args([
            "rustc",
            "--locked",
            "-p",
            "arona-arch",
            "--test",
            name,
            "--features",
            if matches!(name, "execution" | "interrupt") {
                "baremetal,runtime-tests"
            } else {
                "baremetal"
            },
            "--target",
            self.target.triple(),
            "--message-format=json",
        ]);
        if name == "execution" {
            let program = self.program("runtime", "program")?;
            command.env("ARONA_RUNTIME_TEST_IMAGE", program);
        }
        if self.release {
            command.arg("--release");
        }
        command.args(["--", "-Cpanic=abort", "-Zpanic-abort-tests"]);
        linux_test::build_runner(command, name)
    }

    pub fn program(&mut self, package: &str, name: &str) -> Result<PathBuf, String> {
        let key = format!("{package}/{name}");
        if let Some(path) = self.programs.get(&key) {
            return Ok(path.clone());
        }
        let mut command = cargo_command(self.root.clone());
        build_std(&mut command);
        command.args([
            "rustc",
            "--locked",
            "-p",
            &format!("arona-{package}"),
            "--features",
            if package == "runtime" {
                "program"
            } else {
                "baremetal"
            },
            "--test",
            name,
            "--target",
            self.target.triple(),
            "--message-format=json",
        ]);
        if self.release {
            command.arg("--release");
        }
        command.args(["--", "-Cstrip=symbols", "-Clink-arg=-zmax-page-size=4096"]);
        let path = linux_test::build_runner(command, name)?;
        self.programs.insert(key, path.clone());
        Ok(path)
    }

    pub fn binary(&mut self, name: &str) -> Result<PathBuf, String> {
        build_component(self.target, self.release, name)?;
        Ok(target_directory(&self.root)
            .join(self.target.triple())
            .join(if self.release { "release" } else { "debug" })
            .join(name))
    }

    pub fn archive(&mut self, suite: &str) -> Result<PathBuf, String> {
        package_init(
            &BuildOptions {
                target: self.target,
                release: self.release,
                initramfs: Initramfs::External,
                rootfs: None,
                net: false,
            },
            match suite {
                "linux" => Some("linux"),
                "net_service" => Some("net"),
                _ => None,
            },
        )
    }

    pub fn fixture(&self, action: &str, directory: &Path) -> Result<(), String> {
        let mut command = std::process::Command::new("python3");
        command
            .arg(self.root.join("components/fs/tests/ext4/fixture.py"))
            .arg(action)
            .arg(directory);
        super::super::execute(command)
    }
}
