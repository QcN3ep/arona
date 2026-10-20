// SPDX-License-Identifier: MPL-2.0

//! 解析 UART 寄存器布局、输入时钟和固件控制台选择。

use fdt::properties::{Compatible, values::U32List};
use kernel::{ConsoleInfo, HardwareDeviceKind, PhysRange, SerialParity, SerialSettings, UartInfo};

use super::super::{
    Dtb, Result,
    nodes::{Node, integer, property},
};
use super::{DeviceNode, instance_id};

pub(super) fn parse(dtb: &Dtb<'_>, node: Node<'_>, registers: &[PhysRange]) -> Result<UartInfo> {
    let mut frequency = integer(node, "clock-frequency")?;
    if frequency.is_none()
        && let Some(clocks) = property::<U32List<'_>>(node, "clocks")?
    {
        let mut cells = clocks.iter();
        let phandle = cells.next().ok_or("UART clocks is empty")?;
        let clock = dtb.resolve_phandle(phandle)?;
        if cells.next().is_some()
            || property::<u32>(clock, "#clock-cells")? != Some(0)
            || !clock
                .property::<Compatible>()?
                .is_some_and(|names| names.into_iter().any(|name| name == "fixed-clock"))
        {
            return Err("unsupported UART clock binding".into());
        }
        frequency = Some(integer(clock, "clock-frequency")?.ok_or("fixed clock has no frequency")?);
    }
    let info = UartInfo {
        register_shift: property::<u32>(node, "reg-shift")?
            .unwrap_or(0)
            .try_into()
            .map_err(|_| "UART register shift overflow")?,
        register_width: property::<u32>(node, "reg-io-width")?
            .unwrap_or(1)
            .try_into()
            .map_err(|_| "UART register width overflow")?,
        register_offset: property::<u32>(node, "reg-offset")?.unwrap_or(0),
        clock_frequency_hz: frequency,
        endian: super::endian(node)?,
        current_speed: property(node, "current-speed")?,
    };
    validate(&info, registers)?;
    Ok(info)
}

pub(super) fn console(dtb: &Dtb<'_>, nodes: &[DeviceNode<'_>]) -> Result<ConsoleInfo> {
    let Some(chosen) = dtb.root.child("chosen")? else {
        return Ok(ConsoleInfo::Unspecified);
    };
    let Some(stdout) = property::<&str>(chosen, "stdout-path")? else {
        return Ok(ConsoleInfo::Unspecified);
    };
    let (path, options) = stdout
        .split_once(':')
        .map_or((stdout, None), |(path, options)| (path, Some(options)));
    let path = if path.starts_with('/') {
        path
    } else {
        let aliases = dtb
            .root
            .child("aliases")?
            .ok_or("stdout alias has no aliases node")?;
        property::<&str>(aliases, path)?.ok_or("stdout alias is missing")?
    };
    let mut walk = dtb.nodes();
    while let Some(node) = walk.next()? {
        if walk.path().eq(path.bytes()) {
            let name = node.name()?.name;
            let index = nodes
                .iter()
                .position(|device| core::ptr::eq(device.name, name))
                .ok_or("stdout device is unsupported")?;
            if nodes[index].kind != HardwareDeviceKind::UART_16550 {
                return Err("stdout is not a UART".into());
            }
            return Ok(ConsoleInfo::Uart {
                device: instance_id(index),
                settings: options.and_then(|options| settings(options).ok()),
            });
        }
    }
    Err("stdout does not reference an enabled device".into())
}

fn settings(options: &str) -> Result<SerialSettings> {
    // REFERENCE: https://devicetree-specification.readthedocs.io/en/stable/devicenodes.html#chosen-node
    let digits = options.bytes().take_while(u8::is_ascii_digit).count();
    let baud_rate = options[..digits]
        .parse::<u32>()
        .ok()
        .filter(|&rate| rate != 0)
        .ok_or("invalid stdout baud rate")?;
    let mut suffix = options[digits..].bytes();
    let parity = match suffix.next() {
        None | Some(b'n') => SerialParity::None,
        Some(b'e') => SerialParity::Even,
        Some(b'o') => SerialParity::Odd,
        _ => return Err("unsupported stdout parity".into()),
    };
    let data_bits = match suffix.next() {
        None => 8,
        Some(value @ b'5'..=b'8') => value - b'0',
        _ => return Err("invalid stdout data bits".into()),
    };
    let flow_control = match suffix.next() {
        None => false,
        Some(b'r') => true,
        _ => return Err("invalid stdout flow control".into()),
    };
    if suffix.next().is_some() {
        return Err("invalid stdout options".into());
    }
    Ok(SerialSettings {
        baud_rate,
        parity,
        data_bits,
        flow_control,
    })
}

fn validate(info: &UartInfo, registers: &[PhysRange]) -> Result<()> {
    let first_size = registers
        .first()
        .map(|range| range.end.saturating_sub(range.start))
        .unwrap_or(0);
    let stride = 1_u64.checked_shl(u32::from(info.register_shift));
    let size = stride
        .and_then(|stride| stride.checked_mul(7))
        .and_then(|offset| {
            offset.checked_add(u64::from(info.register_offset) + u64::from(info.register_width))
        });
    if !matches!(info.register_width, 1 | 2 | 4)
        || registers.len() != 1
        || stride.is_none_or(|stride| stride < u64::from(info.register_width))
        || registers[0]
            .start
            .checked_add(u64::from(info.register_offset))
            .is_none_or(|start| !start.is_multiple_of(u64::from(info.register_width)))
        || size.is_none_or(|size| size > first_size)
        || info.clock_frequency_hz == Some(0)
        || info.current_speed == Some(0)
    {
        return Err("invalid UART register layout or clock".into());
    }
    Ok(())
}
