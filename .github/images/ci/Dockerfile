# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

FROM ubuntu:26.04

LABEL org.opencontainers.image.source="https://github.com/QcN3ep/arona"

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates clang curl e2fsprogs git ipxe-qemu lld llvm \
        opensbi python3 qemu-system-misc qemu-system-riscv \
    && rm -rf /var/lib/apt/lists/*

ENV CARGO_HOME=/opt/cargo \
    RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:${PATH} \
    ARONA_CC=clang \
    ARONA_AR=llvm-ar

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --no-modify-path --default-toolchain none \
    && rm /tmp/rustup-init.sh

COPY rust-toolchain.toml /tmp/arona-toolchain/rust-toolchain.toml
RUN cd /tmp/arona-toolchain \
    && rustup show \
    && rustup default "$(rustup show active-toolchain | cut -d ' ' -f1)" \
    && rm -rf /tmp/arona-toolchain

WORKDIR /workspace
