// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use std::{env, path::PathBuf};

fn main() {
    let script =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../runtime/link.ld");
    println!("cargo:rerun-if-changed={}", script.display());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg-bin=devmgr=-T{}", script.display());
        println!("cargo:rustc-link-arg-tests=-T{}", script.display());
    }
}
