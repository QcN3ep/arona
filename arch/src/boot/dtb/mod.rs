// SPDX-License-Identifier: MPL-2.0

//! 将固件 DTB 转换到固定容量的内核硬件描述中。

mod cpus;
pub mod devices;
mod memory;
mod nodes;

use fdt::{Fdt, FdtError, FdtHeader, nodes::AsNode, properties::PHandle};
use kernel::{HardwareInfo, PhysRange};

use nodes::{Node, Nodes};

type Result<T> = core::result::Result<T, Error>;
const MAX_DTB_SIZE: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    Fdt(FdtError),
    Invalid(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Fdt(error) => error.fmt(f),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl core::error::Error for Error {}

impl From<FdtError> for Error {
    fn from(error: FdtError) -> Self {
        Self::Fdt(error)
    }
}

impl From<fdt::cell_collector::CollectCellsError> for Error {
    fn from(error: fdt::cell_collector::CollectCellsError) -> Self {
        Self::Fdt(error.into())
    }
}

impl From<&'static str> for Error {
    fn from(message: &'static str) -> Self {
        Self::Invalid(message)
    }
}

/// 固件 DTB 的只读视图。
pub struct Dtb<'a> {
    header: FdtHeader,
    root: Node<'a>,
    bytes: &'a [u8],
}

impl<'a> Dtb<'a> {
    /// # Safety
    ///
    /// `dtb` 须指向完整 DTB，其内存须在 `'a` 内保持可读且不变。
    pub unsafe fn new(dtb: *const u8) -> Result<Self> {
        if dtb.is_null() || !(dtb as usize).is_multiple_of(8) {
            return Err("invalid DTB pointer".into());
        }
        // SAFETY: DTB 头部须可读且按 u32 对齐。
        let size = u32::from_be(unsafe { dtb.add(4).cast::<u32>().read() }) as usize;
        if size > MAX_DTB_SIZE {
            return Err("FDT exceeds 1 MiB".into());
        }
        // SAFETY: dtb 起的 size 字节须在 'a 内保持可读且不变。
        Self::from_bytes(unsafe { core::slice::from_raw_parts(dtb, size) })
    }

    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self> {
        let header = bytes.get(..40).ok_or("invalid FDT header")?;
        let words = header.as_chunks::<4>().0;
        let field = |index| u32::from_be_bytes(words[index]);
        let size = field(1) as usize;
        let structure_offset = field(2);
        let strings_offset = field(3);
        let strings_size = field(8);
        let structure_size = field(9);
        if size > MAX_DTB_SIZE {
            return Err("FDT exceeds 1 MiB".into());
        }
        let bytes = bytes.get(..size).ok_or("truncated FDT")?;
        // fdt 构造器用 u32 相加；先限制区块，避免损坏的偏移触发溢出。
        for (offset, length) in [
            (structure_offset, structure_size),
            (strings_offset, strings_size),
        ] {
            let end = u64::from(offset) + u64::from(length);
            if end > bytes.len() as u64 {
                return Err("FDT block is out of bounds".into());
            }
        }
        // fdt 的根节点读取需要 BEGIN_NODE 之外至少还有一个 token。
        if structure_size < 8 {
            return Err("FDT structure block is too short".into());
        }
        let fdt = Fdt::new_unaligned_fallible(bytes)?;
        let root = fdt.root()?.as_node();
        Ok(Self {
            header: *fdt.header(),
            root,
            bytes,
        })
    }

    pub fn total_size(&self) -> usize {
        self.bytes.len()
    }

    /// 将 DTB 转换为硬件描述，失败时结果不可用。
    pub fn dump(self, hardware: &mut HardwareInfo) -> Result<()> {
        hardware.timebase_frequency_hz = cpus::parse(&self, &mut hardware.cpus)?;
        memory::parse(&self, &mut hardware.memory)?;
        devices::parse(&self, hardware)
    }

    fn nodes(&self) -> Nodes<'a> {
        Nodes::new(self.root)
    }

    fn resolve_phandle(&self, phandle: u32) -> Result<Node<'a>> {
        if phandle == 0 || phandle == u32::MAX {
            return Err("invalid phandle".into());
        }
        let mut found = None;
        let mut walk = self.nodes();
        while let Some(node) = walk.next()? {
            if node
                .property::<PHandle>()?
                .is_some_and(|value| value.as_u32() == phandle)
                && found.replace(node).is_some()
            {
                return Err("duplicate phandle".into());
            }
        }
        found.ok_or_else(|| "phandle does not reference a node".into())
    }

    // REFERENCE: https://devicetree-specification.readthedocs.io/en/stable/flattened-format.html#memory-reservation-block
    fn reservations(&self) -> Result<impl Iterator<Item = Result<PhysRange>> + '_> {
        let header = &self.header;
        let offset = header.memory_reserve_map_offset as usize;
        if offset < 40 || !offset.is_multiple_of(8) {
            return Err("invalid reservation block offset".into());
        }
        let end = [
            header.structs_offset as usize,
            header.strings_offset as usize,
            self.bytes.len(),
        ]
        .into_iter()
        .filter(|&end| end >= offset)
        .min()
        .ok_or("reservation block is out of bounds")?;
        let block = self
            .bytes
            .get(offset..end)
            .ok_or("reservation block is out of bounds")?;
        let entries = block.as_chunks::<16>().0;
        let end = entries
            .iter()
            .position(|entry| *entry == [0; 16])
            .ok_or("reservation block is missing its terminator")?;
        Ok(entries[..end].iter().map(|entry| {
            let words = entry.as_chunks::<8>().0;
            let start = u64::from_be_bytes(words[0]);
            let size = u64::from_be_bytes(words[1]);
            range(start, size)
        }))
    }
}

fn range(start: u64, size: u64) -> Result<PhysRange> {
    if size == 0 {
        return Err("firmware range is empty".into());
    }
    Ok(PhysRange {
        start,
        end: start
            .checked_add(size)
            .ok_or("firmware address range overflow")?,
    })
}
