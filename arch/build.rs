// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Configure the link arguments for the bare-metal image.

use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=ARONA_INITRAMFS");
    if env::var_os("CARGO_FEATURE_EMBEDDED_INITRAMFS").is_some() {
        let path =
            env::var("ARONA_INITRAMFS").expect("ARONA_INITRAMFS must identify a CPIO archive");
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=src/riscv64/link.ld");
    println!("cargo:rerun-if-changed=src/loongarch64/link.ld");

    let target = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo must set CARGO_CFG_TARGET_ARCH");
    let isa = match target.as_str() {
        "riscv64" | "loongarch64" => target,
        _ => {
            println!("cargo:rustc-link-arg=-Wl,-z,nostart-stop-gc");
            return;
        }
    };

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg=-T{manifest_dir}/src/{isa}/link.ld");
    println!("cargo:rustc-link-arg=--gc-sections");
    println!("cargo:rustc-link-arg-tests=--wrap=_start_native");
}
