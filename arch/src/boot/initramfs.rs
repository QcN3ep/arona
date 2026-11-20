// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Locate the archive in the kernel image or firmware handoff.

use kernel::{KernelImage, boot::initramfs::Image, memory::PhysRange};

#[cfg(all(
    feature = "embedded-initramfs",
    any(target_arch = "riscv64", target_arch = "loongarch64")
))]
pub(crate) fn source(
    _external: Option<PhysRange>,
    kernel: KernelImage,
) -> Result<Image<'static>, &'static str> {
    const LENGTH: usize = include_bytes!(env!("ARONA_INITRAMFS")).len();
    #[used]
    #[unsafe(link_section = ".initramfs")]
    static BYTES: [u8; LENGTH] = *include_bytes!(env!("ARONA_INITRAMFS"));
    let address = core::ptr::addr_of!(BYTES) as usize;
    let segment = kernel.rodata;
    let offset = address
        .checked_sub(segment.virtual_start)
        .ok_or("initramfs address is below the kernel")?;
    let physical = segment
        .physical
        .start
        .checked_add(offset as u64)
        .ok_or("initramfs physical address overflow")?;
    Ok(Image::from_physical(PhysRange {
        start: physical,
        end: physical
            .checked_add(LENGTH as u64)
            .ok_or("initramfs physical range overflow")?,
    }))
}

#[cfg(all(
    not(feature = "embedded-initramfs"),
    any(target_arch = "riscv64", target_arch = "loongarch64")
))]
pub(crate) fn source(
    external: Option<PhysRange>,
    _kernel: KernelImage,
) -> Result<Image<'static>, &'static str> {
    Ok(Image::from_physical(
        external.ok_or("missing external initramfs")?,
    ))
}
