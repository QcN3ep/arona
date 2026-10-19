// SPDX-License-Identifier: MPL-2.0

//! 配置裸机映像的链接参数。

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=src/riscv64/link.ld");
    println!("cargo:rerun-if-changed=src/loongarch64/link.ld");

    let target = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo must set CARGO_CFG_TARGET_ARCH");
    let isa = match target.as_str() {
        "riscv64" | "loongarch64" => target,
        _ => return,
    };

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg=-T{manifest_dir}/src/{isa}/link.ld");
    println!("cargo:rustc-link-arg=--gc-sections");
}
