// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use super::{BuildOptions, append_entry, build_std, cargo_command, linux_test};
use std::{fs, path::Path};

pub(super) fn append(
    options: &BuildOptions,
    root: &Path,
    bytes: &mut Vec<u8>,
) -> Result<(), String> {
    for (test, path) in [("service", "bin/init"), ("native", "bin/net-native")] {
        let mut command = cargo_command(root.to_path_buf());
        build_std(&mut command);
        command.args([
            "rustc",
            "--locked",
            "-p",
            "arona-net",
            "--features",
            "baremetal",
            "--test",
            test,
            "--target",
            options.target.triple(),
            "--message-format=json",
        ]);
        if options.release {
            command.arg("--release");
        }
        command.args(["--", "-Cstrip=symbols", "-Clink-arg=-zmax-page-size=4096"]);
        let executable = linux_test::build_runner(command, test)?;
        append_entry(
            bytes,
            path,
            0o100755,
            &fs::read(executable).map_err(|error| error.to_string())?,
        )?;
    }
    Ok(())
}
