// SPDX-License-Identifier: MPL-2.0

//! 设备目录、MMIO 访问与 IRQ 绑定。

use crate::{BorrowedHandle, Device, DeviceSet, Error, Event, Handle, Rights};
pub use abi::device::{DeviceField, DeviceInfo, PciBar};
use abi::method::{device, device_set};
use core::{cell::Cell, marker::PhantomData, ops::Range};

pub struct Mmio<'a> {
    device: BorrowedHandle<'a, Device>,
    range: Option<Range<usize>>,
    exclusive: PhantomData<Cell<()>>,
}

pub struct InterruptBinding<'a> {
    device: BorrowedHandle<'a, Device>,
    event: BorrowedHandle<'a, Event>,
    id: u32,
    index: usize,
    bound: bool,
}

impl Handle<DeviceSet> {
    /// 读取资源配置完成时的只读快照。
    pub fn pci_config(&self, id: u32, output: &mut [u8; 4096]) -> Result<(), Error> {
        if self.read(id, DeviceField::PciConfig, 0, output)?.len() != output.len() {
            return Err(Error::Protocol);
        }
        Ok(())
    }

    pub fn pci_bar(&self, id: u32, index: usize) -> Result<PciBar, Error> {
        let mut bytes = [0; PciBar::SIZE];
        PciBar::decode(self.read(id, DeviceField::PciBar, index, &mut bytes)?)
            .ok_or(Error::Protocol)
    }

    pub fn next(&self, after: u32) -> Result<Option<u32>, Error> {
        let [id, _] = self
            .borrow()
            .invoke(device_set::NEXT, [after as u64, 0, 0, 0, 0])?;
        let id = u32::try_from(id).map_err(|_| Error::Protocol)?;
        Ok((id != 0).then_some(id))
    }

    pub fn describe(&self, id: u32) -> Result<DeviceInfo, Error> {
        let mut bytes = [0; DeviceInfo::SIZE];
        self.borrow().invoke(
            device_set::DESCRIBE,
            [
                id as u64,
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
                0,
                0,
            ],
        )?;
        DeviceInfo::decode(&bytes).ok_or(Error::Protocol)
    }

    pub fn read<'a>(
        &self,
        id: u32,
        field: DeviceField,
        index: usize,
        output: &'a mut [u8],
    ) -> Result<&'a [u8], Error> {
        let [length, _] = self.borrow().invoke(
            device_set::READ,
            [
                id as u64,
                field as u64,
                index as u64,
                output.as_mut_ptr() as u64,
                output.len() as u64,
            ],
        )?;
        output
            .get(..usize::try_from(length).map_err(|_| Error::Protocol)?)
            .ok_or(Error::Protocol)
    }

    pub fn claim_group(&self, group: u32, rights: Rights) -> Result<Handle<Device>, Error> {
        let [raw, _] = self.borrow().invoke(
            device_set::CLAIM_GROUP,
            [group as u64, rights.bits(), 0, 0, 0],
        )?;
        // SAFETY: CLAIM_GROUP 成功须交付独占的新设备句柄。
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
}

impl Handle<Device> {
    pub fn map_mmio(&self, id: u32, region: usize) -> Result<Mmio<'_>, Error> {
        let [start, length] = self
            .borrow()
            .invoke(device::MAP, [id as u64, region as u64, 0, 0, 0])?;
        let end = start.checked_add(length).ok_or(Error::Protocol)?;
        let start = usize::try_from(start).map_err(|_| Error::Protocol)?;
        let end = usize::try_from(end).map_err(|_| Error::Protocol)?;
        if start == 0 || start >= end {
            return Err(Error::Protocol);
        }
        Ok(Mmio {
            device: self.borrow(),
            range: Some(start..end),
            exclusive: PhantomData,
        })
    }

    pub fn bind_interrupt<'a>(
        &'a self,
        id: u32,
        index: usize,
        event: &'a Handle<Event>,
        mask: u64,
    ) -> Result<InterruptBinding<'a>, Error> {
        self.borrow().invoke(
            device::BIND,
            [id as u64, index as u64, event.borrow().raw().raw(), mask, 0],
        )?;
        Ok(InterruptBinding {
            device: self.borrow(),
            event: event.borrow(),
            id,
            index,
            bound: true,
        })
    }

    pub fn revoke(&self) -> Result<(), Error> {
        self.borrow().invoke(device::REVOKE, [0; 5])?;
        Ok(())
    }
}

impl Mmio<'_> {
    pub fn range(&self) -> Option<Range<usize>> {
        self.range.clone()
    }
    pub fn len(&self) -> usize {
        self.range
            .as_ref()
            .map_or(0, |range| range.end - range.start)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn unmap(&mut self) -> Result<(), Error> {
        if let Some(range) = &self.range {
            self.device
                .invoke(device::UNMAP, [range.start as u64, 0, 0, 0, 0])?;
            self.range = None;
        }
        Ok(())
    }

    fn address(&self, offset: usize, width: usize) -> Result<usize, Error> {
        let range = self
            .range
            .as_ref()
            .ok_or(Error::Kernel(abi::Error::Closed))?;
        if offset.checked_add(width).is_none_or(|end| end > self.len()) {
            return Err(Error::Kernel(abi::Error::InvalidArgument));
        }
        let address = range.start + offset;
        if !address.is_multiple_of(width) {
            return Err(Error::Kernel(abi::Error::InvalidArgument));
        }
        Ok(address)
    }
}

impl Drop for Mmio<'_> {
    fn drop(&mut self) {
        let _ = self.unmap();
    }
}

impl InterruptBinding<'_> {
    pub fn event(&self) -> BorrowedHandle<'_, Event> {
        self.event
    }

    pub fn acknowledge(&mut self) -> Result<(), Error> {
        if !self.bound {
            return Err(Error::Kernel(abi::Error::Closed));
        }
        self.device
            .invoke(device::ACK, [self.id as u64, self.index as u64, 0, 0, 0])?;
        Ok(())
    }

    pub fn unbind(&mut self) -> Result<(), Error> {
        if self.bound {
            self.device
                .invoke(device::UNBIND, [self.id as u64, self.index as u64, 0, 0, 0])?;
            self.bound = false;
        }
        Ok(())
    }
}

impl Drop for InterruptBinding<'_> {
    fn drop(&mut self) {
        let _ = self.unbind();
    }
}

macro_rules! registers {
    ($read:ident, $write:ident, $ty:ty, $rv_load:literal, $rv_store:literal, $la_load:literal, $la_store:literal) => {
        impl Mmio<'_> {
            /// 授权撤销后的访问触发用户异常。
            pub fn $read(&mut self, offset: usize) -> Result<$ty, Error> {
                let address = self.address(offset, core::mem::size_of::<$ty>())?;
                #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                let value: usize;
                #[cfg(target_arch = "riscv64")]
                // SAFETY: 映射地址须在访问期间保持为设备页或不可访问页。
                unsafe {
                    core::arch::asm!(
                        "fence iorw, iorw",
                        concat!($rv_load, " {value}, 0({address})"),
                        "fence iorw, iorw",
                        address = in(reg) address,
                        value = out(reg) value,
                        options(nostack),
                    );
                }
                #[cfg(target_arch = "loongarch64")]
                // SAFETY: 映射地址须在访问期间保持为设备页或不可访问页。
                unsafe {
                    core::arch::asm!(
                        "dbar 0",
                        concat!($la_load, " {value}, {address}, 0"),
                        "dbar 0",
                        address = in(reg) address,
                        value = out(reg) value,
                        options(nostack),
                    );
                }
                #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
                {
                    let _ = address;
                    Err(Error::Kernel(abi::Error::Unsupported))
                }
                #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                Ok(value as $ty)
            }

            /// 授权撤销后的访问触发用户异常。
            pub fn $write(&mut self, offset: usize, value: $ty) -> Result<(), Error> {
                let address = self.address(offset, core::mem::size_of::<$ty>())?;
                #[cfg(target_arch = "riscv64")]
                // SAFETY: 映射地址须在访问期间保持为设备页或不可访问页。
                unsafe {
                    core::arch::asm!(
                        "fence iorw, iorw",
                        concat!($rv_store, " {value}, 0({address})"),
                        "fence iorw, iorw",
                        address = in(reg) address,
                        value = in(reg) value as usize,
                        options(nostack),
                    );
                }
                #[cfg(target_arch = "loongarch64")]
                // SAFETY: 映射地址须在访问期间保持为设备页或不可访问页。
                unsafe {
                    core::arch::asm!(
                        "dbar 0",
                        concat!($la_store, " {value}, {address}, 0"),
                        "dbar 0",
                        address = in(reg) address,
                        value = in(reg) value as usize,
                        options(nostack),
                    );
                }
                #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
                {
                    let _ = (address, value);
                    Err(Error::Kernel(abi::Error::Unsupported))
                }
                #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                Ok(())
            }
        }
    };
}

registers!(read_u8, write_u8, u8, "lbu", "sb", "ld.bu", "st.b");
registers!(read_u16, write_u16, u16, "lhu", "sh", "ld.hu", "st.h");
registers!(read_u32, write_u32, u32, "lwu", "sw", "ld.wu", "st.w");
