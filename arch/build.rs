// SPDX-License-Identifier: MPL-2.0

//! 配置 RISC-V 裸机映像的链接参数。

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=src/riscv64/link.ld");

    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("riscv64") {
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg=-T{manifest_dir}/src/riscv64/link.ld");
    println!("cargo:rustc-link-arg=--gc-sections");
}
