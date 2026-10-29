// SPDX-License-Identifier: MPL-2.0

//! 读取启动归档并定位 init 映像。

mod elf;

pub use elf::Elf;
use kernel::{
    BootData, HardwareInfo, KernelImage,
    memory::{MemoryRegionState, PhysRange},
};

#[derive(Clone, Copy, Debug)]
pub struct Image<'a> {
    data: BootData,
    bytes: Option<&'a [u8]>,
}

impl<'a> Image<'a> {
    pub fn from_bytes(bytes: &'a [u8], physical_start: u64) -> Result<Self, &'static str> {
        let end = physical_start
            .checked_add(bytes.len() as u64)
            .ok_or("image range overflow")?;
        Ok(Self {
            data: BootData {
                physical: PhysRange {
                    start: physical_start,
                    end,
                },
            },
            bytes: Some(bytes),
        })
    }

    pub fn from_physical(physical: PhysRange) -> Self {
        Self {
            data: BootData { physical },
            bytes: None,
        }
    }

    pub fn data(self) -> BootData {
        self.data
    }

    pub fn len(self) -> Result<usize, &'static str> {
        self.data.len()
    }

    pub fn is_empty(self) -> bool {
        self.data.is_empty()
    }

    pub fn read(self, offset: usize, output: &mut [u8]) -> Result<(), &'static str> {
        if let Some(bytes) = self.bytes {
            let end = offset
                .checked_add(output.len())
                .ok_or("image offset overflow")?;
            output.copy_from_slice(
                bytes
                    .get(offset..end)
                    .ok_or("image read is out of bounds")?,
            );
            Ok(())
        } else {
            self.data.read(offset, output)
        }
    }

    pub(super) fn slice(self, offset: usize, length: usize) -> Result<Self, &'static str> {
        let size = self.len()?;
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= size)
            .ok_or("image slice is out of bounds")?;
        Ok(Self {
            data: BootData {
                physical: PhysRange {
                    start: self.data.physical.start + offset as u64,
                    end: self.data.physical.start + end as u64,
                },
            },
            bytes: self.bytes.map(|bytes| &bytes[offset..end]),
        })
    }
}

pub fn prepare<'a>(
    archive: Image<'a>,
    hardware: &mut HardwareInfo,
    kernel: KernelImage,
    machine: u16,
) -> Result<Elf<'a>, &'static str> {
    let range = archive.data.physical;
    if archive.is_empty() || !hardware.memory.is_mappable(range.page_envelope()?) {
        return Err("initramfs is outside mappable RAM");
    }
    if archive.bytes.is_none() && range.page_envelope()?.overlaps(kernel.physical) {
        return Err("initramfs overlaps the kernel");
    }
    hardware
        .memory
        .reserve(range, MemoryRegionState::BootReserved)?;
    Elf::new(find_init(archive)?, machine)
}

// REFERENCE: https://www.kernel.org/doc/html/latest/driver-api/early-userspace/buffer-format.html
pub fn find_init(archive: Image<'_>) -> Result<Image<'_>, &'static str> {
    let mut offset = 0usize;
    let mut init = None;
    loop {
        let mut header = [0; 110];
        archive.read(offset, &mut header)?;
        if &header[..6] != b"070701" {
            return Err("unsupported CPIO header");
        }
        let mode = hex(&header[14..22])?;
        let size = hex(&header[54..62])?;
        let name_size = hex(&header[94..102])?;
        if hex(&header[102..110])? != 0 {
            return Err("unsupported CPIO checksum");
        }
        if name_size == 0 {
            return Err("empty CPIO filename");
        }
        let name_offset = offset
            .checked_add(header.len())
            .ok_or("CPIO offset overflow")?;
        let name = archive.slice(name_offset, name_size)?;
        let mut terminator = [0];
        name.read(name_size - 1, &mut terminator)?;
        if terminator != [0] {
            return Err("unterminated CPIO filename");
        }
        let data_offset = name_offset
            .checked_add(name_size)
            .and_then(|end| end.checked_next_multiple_of(4))
            .ok_or("CPIO offset overflow")?;
        let data = archive.slice(data_offset, size)?;
        if matches_name(name, b"TRAILER!!!\0")? {
            if size != 0 {
                return Err("invalid CPIO trailer");
            }
            return init.ok_or("initramfs is missing bin/init");
        }
        if matches_name(name, b"bin/init\0")? || matches_name(name, b"./bin/init\0")? {
            if mode & 0o170000 != 0o100000
                || mode & 0o111 == 0
                || hex(&header[38..46])? != 1
                || init.is_some()
            {
                return Err("invalid or duplicate init entry");
            }
            init = Some(data);
        }
        offset = data_offset
            .checked_add(size)
            .and_then(|end| end.checked_next_multiple_of(4))
            .ok_or("CPIO offset overflow")?;
    }
}

fn matches_name(name: Image<'_>, expected: &[u8]) -> Result<bool, &'static str> {
    if name.len()? != expected.len() {
        return Ok(false);
    }
    let mut bytes = [0; 16];
    name.read(0, &mut bytes[..expected.len()])?;
    Ok(&bytes[..expected.len()] == expected)
}

fn hex(bytes: &[u8]) -> Result<usize, &'static str> {
    bytes.iter().try_fold(0usize, |value, &digit| {
        let digit = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            b'A'..=b'F' => digit - b'A' + 10,
            _ => return Err("invalid CPIO number"),
        };
        value
            .checked_mul(16)
            .and_then(|value| value.checked_add(digit as usize))
            .ok_or("CPIO number overflow")
    })
}

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
