// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use std::{env, path::PathBuf};

fn main() {
    let runtime_script =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../runtime/link.ld");
    println!("cargo:rerun-if-changed={}", runtime_script.display());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!(
            "cargo:rustc-link-arg-bin=init=-T{}",
            runtime_script.display()
        );
    }
}
