// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Device directory, MMIO access, and IRQ binding.

use crate::{BorrowedHandle, Device, DeviceSet, Error, Event, Handle, Rights};
pub use abi::device::DeviceInfo;
pub use abi::info::device as topic;
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

macro_rules! catalog {
    ($kind:ty) => {
        impl Handle<$kind> {
            pub fn next(&self, after: u32) -> Result<Option<u32>, Error> {
                let query = u64::from(after).to_le_bytes();
                // SAFETY: The input buffer remains borrowed until the call returns.
                let [id, _] = unsafe {
                    self.borrow().invoke(
                        abi::method::INFO,
                        [topic::NEXT_DEVICE, query.as_ptr() as u64, 8, 0, 0],
                    )
                }?;
                let id = u32::try_from(id).map_err(|_| Error::Protocol)?;
                Ok((id != 0).then_some(id))
            }
            pub fn describe(&self, id: u32) -> Result<DeviceInfo, Error> {
                let mut bytes = [0; DeviceInfo::SIZE];
                self.read(id, topic::DEVICE, 0, &mut bytes)?;
                DeviceInfo::decode(&bytes).ok_or(Error::Protocol)
            }
            pub fn configuration(&self, id: u32, output: &mut [u8]) -> Result<usize, Error> {
                Ok(self.read(id, topic::CONFIGURATION, 0, output)?.len())
            }
            pub fn read<'a>(
                &self,
                id: u32,
                topic: u64,
                index: usize,
                output: &'a mut [u8],
            ) -> Result<&'a [u8], Error> {
                let indexed = matches!(
                    topic,
                    topic::IDENTITY_NAMESPACE
                        | topic::IDENTITY
                        | topic::PROPERTY_NAMESPACE
                        | topic::PROPERTY_NAME
                        | topic::PROPERTY_VALUE
                );
                if !indexed && index != 0 {
                    return Err(Error::Kernel(abi::Error::InvalidArgument));
                }
                let mut query = [0; 16];
                query[..8].copy_from_slice(&u64::from(id).to_le_bytes());
                query[8..].copy_from_slice(&(index as u64).to_le_bytes());
                let length = if indexed { 16 } else { 8 };
                // SAFETY: The query stays borrowed and the output buffer stays exclusively borrowed for the call.
                let [written, _] = unsafe {
                    self.borrow().invoke(
                        abi::method::INFO,
                        [
                            topic,
                            query.as_ptr() as u64,
                            length,
                            output.as_mut_ptr() as u64,
                            output.len() as u64,
                        ],
                    )
                }?;
                output
                    .get(..usize::try_from(written).map_err(|_| Error::Protocol)?)
                    .ok_or(Error::Protocol)
            }
            pub fn property<'a>(
                &self,
                id: u32,
                namespace: &str,
                name: &str,
                output: &'a mut [u8],
            ) -> Result<Option<&'a [u8]>, Error> {
                let count = self.describe(id)?.properties;
                let mut bytes = [0; 128];
                for index in 0..count {
                    if self.read(id, topic::PROPERTY_NAMESPACE, index, &mut bytes)?
                        == namespace.as_bytes()
                        && self.read(id, topic::PROPERTY_NAME, index, &mut bytes)?
                            == name.as_bytes()
                    {
                        return Ok(Some(self.read(id, topic::PROPERTY_VALUE, index, output)?));
                    }
                }
                Ok(None)
            }
        }
    };
}
catalog!(DeviceSet);
catalog!(Device);

impl Handle<DeviceSet> {
    pub fn claim_group(&self, group: u32, rights: Rights) -> Result<Handle<Device>, Error> {
        // SAFETY: CLAIM_GROUP takes only handles and scalar values, with no local buffers.
        let [raw, _] = unsafe {
            self.borrow().invoke(
                device_set::CLAIM_GROUP,
                [group as u64, rights.bits(), 0, 0, 0],
            )
        }?;
        // SAFETY: A successful CLAIM_GROUP must deliver an exclusive new device handle.
        Ok(unsafe { Handle::from_raw(abi::Handle::from_raw(raw)) })
    }
}

impl Handle<Device> {
    pub fn map_mmio(&self, id: u32, region: usize) -> Result<Mmio<'_>, Error> {
        // SAFETY: MAP takes only handles and scalar values, with no local buffers.
        let [start, length] = unsafe {
            self.borrow()
                .invoke(device::MAP, [id as u64, region as u64, 0, 0, 0])
        }?;
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
        // SAFETY: BIND takes only handles and scalar values, with no local buffers.
        unsafe {
            self.borrow().invoke(
                device::BIND,
                [id as u64, index as u64, event.borrow().raw().raw(), mask, 0],
            )
        }?;
        Ok(InterruptBinding {
            device: self.borrow(),
            event: event.borrow(),
            id,
            index,
            bound: true,
        })
    }

    pub fn revoke(&self) -> Result<(), Error> {
        // SAFETY: REVOKE takes only handles and scalar values, with no local buffers.
        unsafe { self.borrow().invoke(device::REVOKE, [0; 5]) }?;
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
            // SAFETY: UNMAP takes only handles and scalar values, with no local buffers.
            unsafe {
                self.device
                    .invoke(device::UNMAP, [range.start as u64, 0, 0, 0, 0])
            }?;
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
        // SAFETY: ACK takes only handles and scalar values, with no local buffers.
        unsafe {
            self.device
                .invoke(device::ACK, [self.id as u64, self.index as u64, 0, 0, 0])
        }?;
        Ok(())
    }

    pub fn unbind(&mut self) -> Result<(), Error> {
        if self.bound {
            // SAFETY: UNBIND takes only handles and scalar values, with no local buffers.
            unsafe {
                self.device
                    .invoke(device::UNBIND, [self.id as u64, self.index as u64, 0, 0, 0])
            }?;
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
            /// Access after authorization is revoked triggers a user exception.
            pub fn $read(&mut self, offset: usize) -> Result<$ty, Error> {
                let address = self.address(offset, core::mem::size_of::<$ty>())?;
                #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
                let value: usize;
                #[cfg(target_arch = "riscv64")]
                // SAFETY: The mapped address must remain a device page or an inaccessible page during access.
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
                // SAFETY: The mapped address must remain a device page or an inaccessible page during access.
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

            /// Access after authorization is revoked triggers a user exception.
            pub fn $write(&mut self, offset: usize, value: $ty) -> Result<(), Error> {
                let address = self.address(offset, core::mem::size_of::<$ty>())?;
                #[cfg(target_arch = "riscv64")]
                // SAFETY: The mapped address must remain a device page or an inaccessible page during access.
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
                // SAFETY: The mapped address must remain a device page or an inaccessible page during access.
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
