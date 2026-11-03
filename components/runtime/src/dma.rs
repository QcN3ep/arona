// SPDX-License-Identifier: MPL-2.0

//! DMA 域、总线地址映射与缓冲区访问。

use crate::{BorrowedHandle, Device, DmaDomain, Error, FrameAllocation, Handle, Rights, Task};
pub use abi::dma::{DmaDirection, DmaInfo, DmaProfile, DmaState};
use abi::method::{device, dma, memory};

pub struct DmaMapping<'a> {
    range: DmaRange<'a>,
    frames: BorrowedHandle<'a, FrameAllocation>,
}

pub struct DmaBuffer<'a> {
    range: DmaRange<'a>,
    frames: Handle<FrameAllocation>,
}

struct DmaRange<'a> {
    domain: BorrowedHandle<'a, DmaDomain>,
    address: Option<u64>,
    offset: u64,
    length: u64,
}

impl Handle<Device> {
    pub fn create_dma(&self, id: u32, rights: Rights) -> Result<Handle<DmaDomain>, Error> {
        let [raw, _] = self
            .borrow()
            .invoke(device::CREATE_DMA, [id as u64, rights.bits(), 0, 0, 0])?;
        // SAFETY: CREATE_DMA 成功须交付独占的新 DMA 域句柄。
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
}

impl Handle<DmaDomain> {
    pub fn state(&self) -> Result<DmaState, Error> {
        let [state, _] = self.borrow().invoke(dma::STATE, [0; 5])?;
        DmaState::from_raw(state).ok_or(Error::Protocol)
    }

    pub fn map<'a>(
        &'a self,
        frames: &'a Handle<FrameAllocation>,
        first: u64,
        count: u64,
        direction: DmaDirection,
    ) -> Result<DmaMapping<'a>, Error> {
        Ok(DmaMapping {
            range: self.map_range(frames, first, count, direction)?,
            frames: frames.borrow(),
        })
    }

    pub fn allocate<'a>(
        &'a self,
        task: &Handle<Task>,
        count: u64,
        direction: DmaDirection,
    ) -> Result<DmaBuffer<'a>, Error> {
        let frames = task.allocate(count, Rights::READ_WRITE.union(Rights::DMA_MAP))?;
        Ok(DmaBuffer {
            range: self.map_range(&frames, 0, count, direction)?,
            frames,
        })
    }

    fn map_range(
        &self,
        frames: &Handle<FrameAllocation>,
        first: u64,
        count: u64,
        direction: DmaDirection,
    ) -> Result<DmaRange<'_>, Error> {
        let [address, length] = self.borrow().invoke(
            dma::MAP,
            [
                frames.borrow().raw().raw(),
                first,
                count,
                direction as u64,
                0,
            ],
        )?;
        Ok(DmaRange {
            domain: self.borrow(),
            address: Some(address),
            offset: first.checked_mul(4096).ok_or(Error::Protocol)?,
            length,
        })
    }

    pub fn resume(&self) -> Result<(), Error> {
        self.borrow().invoke(dma::RESUME, [0; 5])?;
        Ok(())
    }

    /// # Safety
    ///
    /// 设备队列须已停止，且在途 DMA 已完成。
    pub fn quiesce(&self) -> Result<(), Error> {
        self.borrow().invoke(dma::QUIESCE, [0; 5])?;
        Ok(())
    }

    pub fn revoke(&self) -> Result<(), Error> {
        self.borrow().invoke(dma::REVOKE, [0; 5])?;
        Ok(())
    }
}

impl DmaRange<'_> {
    fn bus_address(&self) -> Option<u64> {
        self.address
    }
    fn len(&self) -> u64 {
        self.length
    }
    fn is_empty(&self) -> bool {
        self.length == 0
    }

    fn read(
        &self,
        frames: BorrowedHandle<'_, FrameAllocation>,
        offset: u64,
        output: &mut [u8],
    ) -> Result<(), Error> {
        let offset = self.check(offset, output.len())?;
        synchronize();
        frames.invoke(
            memory::READ,
            [
                offset,
                output.as_mut_ptr() as u64,
                output.len() as u64,
                0,
                0,
            ],
        )?;
        synchronize();
        Ok(())
    }

    fn write(
        &mut self,
        frames: BorrowedHandle<'_, FrameAllocation>,
        offset: u64,
        input: &[u8],
    ) -> Result<(), Error> {
        let offset = self.check(offset, input.len())?;
        synchronize();
        frames.invoke(
            memory::WRITE,
            [offset, input.as_ptr() as u64, input.len() as u64, 0, 0],
        )?;
        synchronize();
        Ok(())
    }

    fn unmap(&mut self) -> Result<(), Error> {
        if let Some(address) = self.address {
            self.domain.invoke(dma::UNMAP, [address, 0, 0, 0, 0])?;
            self.address = None;
        }
        Ok(())
    }

    fn check(&self, offset: u64, length: usize) -> Result<u64, Error> {
        self.address.ok_or(Error::Kernel(abi::Error::Closed))?;
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::Kernel(abi::Error::InvalidArgument));
        }
        self.offset.checked_add(offset).ok_or(Error::Protocol)
    }
}

macro_rules! buffer_access {
    ($ty:ident, $($access:tt)*) => {
        impl $ty<'_> {
            pub fn bus_address(&self) -> Option<u64> { self.range.bus_address() }
            pub fn len(&self) -> u64 { self.range.len() }
            pub fn is_empty(&self) -> bool { self.range.is_empty() }
            pub fn read(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
                self.range.read(self.frames $($access)*, offset, output)
            }
            pub fn write(&mut self, offset: u64, input: &[u8]) -> Result<(), Error> {
                self.range.write(self.frames $($access)*, offset, input)
            }
            pub fn unmap(&mut self) -> Result<(), Error> { self.range.unmap() }
        }
    };
}
buffer_access!(DmaMapping,);
buffer_access!(DmaBuffer, .borrow());

pub fn synchronize() {
    #[cfg(target_arch = "riscv64")]
    // SAFETY: 屏障不得修改栈或通用寄存器。
    unsafe {
        core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "loongarch64")]
    // SAFETY: 屏障不得修改栈或通用寄存器。
    unsafe {
        core::arch::asm!("dbar 0", options(nostack, preserves_flags));
    }
}

impl Drop for DmaRange<'_> {
    fn drop(&mut self) {
        // 活动域拒绝解除映射，页框由域继续保留。
        let _ = self.unmap();
    }
}
