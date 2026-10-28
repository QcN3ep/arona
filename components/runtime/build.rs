// SPDX-License-Identifier: MPL-2.0

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=link.ld");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none")
        && env::var_os("CARGO_FEATURE_PROGRAM").is_some()
    {
        let directory = env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-tests=-T{directory}/link.ld");
    }
}
