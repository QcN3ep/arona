// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Compile the checked-in lwIP sources for the native program ABI.

use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=sources.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=ARONA_CC");
    println!("cargo:rerun-if-env-changed=ARONA_AR");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none")
        || env::var_os("CARGO_FEATURE_BAREMETAL").is_none()
    {
        return;
    }
    let compiler = env::var("ARONA_CC").unwrap_or_else(|_| "clang".into());
    let archiver = env::var("ARONA_AR").unwrap_or_else(|_| "llvm-ar".into());
    let flags = target_flags();
    let resource = Command::new(&compiler)
        .arg("-print-resource-dir")
        .output()
        .expect("run Clang");
    assert!(resource.status.success(), "query Clang builtin headers");
    let headers = PathBuf::from(String::from_utf8(resource.stdout).unwrap().trim()).join("include");
    let directory = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let manifest: toml::Table = fs::read_to_string("sources.toml").unwrap().parse().unwrap();
    let mut objects = Vec::new();
    for (index, source) in manifest["sources"].as_array().unwrap().iter().enumerate() {
        let source = source.as_str().unwrap();
        let object = directory.join(format!("{index}.o"));
        let status = Command::new(&compiler)
            .args(flags)
            .args([
                "-std=c11",
                "-ffreestanding",
                "-fno-builtin",
                "-fno-stack-protector",
                "-ffunction-sections",
                "-fdata-sections",
                "-nostdinc",
                "-isystem",
            ])
            .arg(&headers)
            .args(["-Isrc/include", "-Wall", "-Wextra", "-Werror"])
            .arg(format!("-O{}", env::var("OPT_LEVEL").unwrap()))
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(&object)
            .status()
            .expect("compile lwIP");
        assert!(status.success(), "compile {source}");
        objects.push(object);
    }
    let archive = directory.join("libarona_lwip.a");
    let _ = fs::remove_file(&archive);
    assert!(
        Command::new(archiver)
            .arg("crs")
            .arg(&archive)
            .args(objects)
            .status()
            .expect("archive lwIP")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", directory.display());
    println!("cargo:rustc-link-lib=static=arona_lwip");
    let script =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../runtime/link.ld");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rustc-link-arg-tests=-T{}", script.display());
}

fn target_flags() -> &'static [&'static str] {
    match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "riscv64" => &[
            "--target=riscv64-unknown-elf",
            "-march=rv64gc",
            "-mabi=lp64d",
            "-mcmodel=medany",
            "-mno-relax",
        ],
        "loongarch64" => &[
            "--target=loongarch64-unknown-elf",
            "-march=loongarch64",
            "-mabi=lp64d",
            "-mno-lsx",
            "-mno-lasx",
        ],
        _ => panic!("unsupported lwIP target"),
    }
}
