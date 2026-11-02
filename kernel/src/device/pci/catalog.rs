// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use arrayvec::ArrayVec;

use crate::{
    DeviceIdentity, DeviceProperty, DeviceUsePolicy, HardwareDeviceInfo, HardwareDeviceInstanceId,
    HardwareDeviceLocation,
    device::{Descriptor, DeviceError},
    memory::MemoryError,
};

use super::{Configuration, FunctionInfo};

#[derive(Clone, Copy)]
pub(super) struct Bdf {
    pub(super) segment: u16,
    pub(super) bus: u8,
    pub(super) slot: u8,
    pub(super) function: u8,
}

pub(super) fn record(
    output: &mut Vec<Descriptor>,
    properties: &mut Vec<DeviceProperty>,
    next_id: &mut u32,
    parent: HardwareDeviceInstanceId,
    bdf: Bdf,
    info: FunctionInfo,
    configuration: Configuration,
) -> Result<u32, DeviceError> {
    let mut name = *b"PCI 0000:00:00.0";
    let mut bdf_text = *b"0000:00:00.0";
    let mut vendor_device = *b"0000:0000";
    let mut class = *b"00:00:00";
    encode_hex(u32::from(bdf.segment), &mut bdf_text[..4]);
    encode_hex(u32::from(bdf.bus), &mut bdf_text[5..7]);
    encode_hex(u32::from(bdf.slot), &mut bdf_text[8..10]);
    bdf_text[11] = hex_digit(bdf.function);
    name[4..].copy_from_slice(&bdf_text);
    encode_hex(u32::from(info.vendor), &mut vendor_device[..4]);
    encode_hex(u32::from(info.device), &mut vendor_device[5..]);
    encode_hex(u32::from(info.class), &mut class[..2]);
    encode_hex(u32::from(info.subclass), &mut class[3..5]);
    encode_hex(u32::from(info.programming_interface), &mut class[6..]);
    output
        .try_reserve(1)
        .map_err(|_| DeviceError::Memory(MemoryError::NoMemory))?;
    properties
        .try_reserve(1)
        .map_err(|_| DeviceError::Memory(MemoryError::NoMemory))?;
    let instance_id = *next_id;
    let next_instance_id = instance_id
        .checked_add(1)
        .ok_or(DeviceError::Memory(MemoryError::NoMemory))?;
    let mut device = HardwareDeviceInfo::new(HardwareDeviceInstanceId(instance_id));
    device.name = core::str::from_utf8(&name)
        .map_err(|_| DeviceError::InvalidArgument)?
        .try_into()
        .map_err(|_| DeviceError::InvalidArgument)?;
    device.parent = Some(parent);
    device.use_policy = DeviceUsePolicy::KernelReserved;
    device.location = HardwareDeviceLocation::Pci {
        segment: bdf.segment,
        bus: bdf.bus,
        slot: bdf.slot,
        function: bdf.function,
        bars: ArrayVec::new(),
    };
    device
        .identities
        .try_push(
            DeviceIdentity::new(
                "pci",
                core::str::from_utf8(&vendor_device).map_err(|_| DeviceError::InvalidArgument)?,
            )
            .map_err(|_| DeviceError::InvalidArgument)?,
        )
        .map_err(|_| DeviceError::InvalidArgument)?;
    device
        .identities
        .try_push(
            DeviceIdentity::new(
                "pci-class",
                core::str::from_utf8(&class).map_err(|_| DeviceError::InvalidArgument)?,
            )
            .map_err(|_| DeviceError::InvalidArgument)?,
        )
        .map_err(|_| DeviceError::InvalidArgument)?;
    properties.push(
        DeviceProperty::new(
            HardwareDeviceInstanceId(instance_id),
            "pci",
            "bdf",
            &bdf_text,
        )
        .map_err(|_| DeviceError::InvalidArgument)?,
    );
    output.push(Descriptor {
        hardware: device,
        group: None,
        pci: Some(configuration),
    });
    *next_id = next_instance_id;
    Ok(instance_id)
}

fn encode_hex(mut value: u32, output: &mut [u8]) {
    for byte in output.iter_mut().rev() {
        *byte = hex_digit(value as u8);
        value >>= 4;
    }
}

fn hex_digit(value: u8) -> u8 {
    b"0123456789abcdef"[usize::from(value & 0x0f)]
}
