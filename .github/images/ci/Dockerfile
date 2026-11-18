# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

FROM ubuntu:26.04 AS qemu

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl libfdt-dev libglib2.0-dev \
        libpixman-1-dev libslirp-dev meson ninja-build python3 python3-pip \
        python3-setuptools python3-venv python3-wheel zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /tmp/qemu
RUN curl --http1.1 --retry 3 -fsSL https://download.qemu.org/qemu-11.1.3.tar.xz -o qemu.tar.xz \
    && echo '5a8289c7940dc77d3873c81a6c29ba3afe1446fc9d36e77fa1658cdbecb84b22  qemu.tar.xz' | sha256sum -c - \
    && tar -xf qemu.tar.xz --strip-components=1 \
    && ./configure --prefix=/usr/local \
        --target-list=riscv64-softmmu,loongarch64-softmmu \
        --without-default-features --enable-fdt --enable-pixman --enable-slirp --enable-plugins \
        --disable-docs --disable-download \
    && make -j"$(nproc)" \
    && DESTDIR=/opt/qemu make install

FROM ubuntu:26.04

LABEL org.opencontainers.image.source="https://github.com/QcN3ep/arona"

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates clang curl e2fsprogs git ipxe-qemu lld llvm mtools \
        opensbi python3 qemu-system-misc qemu-system-riscv \
    && rm -rf /var/lib/apt/lists/*

COPY --from=qemu /opt/qemu/usr/local/ /usr/local/

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
