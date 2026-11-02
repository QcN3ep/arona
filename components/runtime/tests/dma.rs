// SPDX-License-Identifier: MPL-2.0

#![no_std]
#![forbid(unsafe_code)]

use runtime::{
    Device, DeviceSet, DmaDomain, Error, Handle, ObjectType, Rights,
    device::Mmio,
    dma::{DmaDirection, DmaState},
    ipc::{Message, WaitMode},
    process, thread,
};

fn main() {
    let bootstrap = process::bootstrap().unwrap();
    let mut startup = bootstrap.receive(WaitMode::Poll).unwrap();
    let devices = startup
        .take_handle(0)
        .unwrap()
        .downcast::<DeviceSet>()
        .unwrap();
    let mut cursor = 0;
    let mut tested = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let info = devices.describe(id).unwrap();
        let Some(group) = info.group else {
            continue;
        };
        let grant = devices
            .claim_group(group, ObjectType::Device.supported_rights())
            .unwrap();
        let result = grant.create_dma(id, ObjectType::DmaDomain.supported_rights());
        if info.dma.address_bits == 0 {
            assert!(matches!(
                result,
                Err(Error::Kernel(abi::Error::Unsupported))
            ));
            continue;
        }
        let domain = result.unwrap();
        let Some(mut transport) = Transport::open(&devices, &grant, id) else {
            continue;
        };
        let peer = shared_peer(&devices, &grant, info);
        permissions(&grant, &domain, id);
        transfer(&domain, &mut transport, &grant, id, peer.as_ref());
        assert_eq!(domain.state().unwrap(), DmaState::Quiesced);
        domain.revoke().unwrap();
        assert_eq!(domain.state().unwrap(), DmaState::Revoked);
        assert_eq!(domain.resume(), Err(Error::Kernel(abi::Error::Closed)));
        let replacement = grant
            .create_dma(id, ObjectType::DmaDomain.supported_rights())
            .unwrap();
        grant.revoke().unwrap();
        assert_eq!(replacement.state(), Err(Error::Kernel(abi::Error::Closed)));
        tested += 1;
    }
    assert!(tested != 0);
    bootstrap
        .send(&mut Message::new(73, &[2]).unwrap(), WaitMode::Forever)
        .unwrap();
}

struct PeerDomain {
    _grant: Option<Handle<Device>>,
    domain: Handle<DmaDomain>,
}

fn shared_peer(
    devices: &Handle<DeviceSet>,
    grant: &Handle<Device>,
    current: abi::device::DeviceInfo,
) -> Option<PeerDomain> {
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let info = devices.describe(id).unwrap();
        if id == current.id || info.parent != current.parent || info.dma.address_bits == 0 {
            continue;
        }
        let mut config = [0; 4096];
        if devices.pci_config(id, &mut config).is_err() || config[..4] != [0xf4, 0x1a, 0x44, 0x10] {
            continue;
        }
        let peer = (info.group != current.group).then(|| {
            devices
                .claim_group(info.group.unwrap(), ObjectType::Device.supported_rights())
                .unwrap()
        });
        let domain = peer
            .as_ref()
            .unwrap_or(grant)
            .create_dma(id, ObjectType::DmaDomain.supported_rights())
            .unwrap();
        return Some(PeerDomain {
            _grant: peer,
            domain,
        });
    }
    None
}

fn permissions(grant: &Handle<Device>, domain: &Handle<DmaDomain>, id: u32) {
    let denied = grant.try_clone(Rights::MAP).unwrap();
    assert!(matches!(
        denied.create_dma(id, Rights::READ),
        Err(Error::Kernel(abi::Error::Denied))
    ));
    let denied = domain.try_clone(Rights::NONE).unwrap();
    assert_eq!(denied.state(), Err(Error::Kernel(abi::Error::Denied)));
    assert_eq!(denied.resume(), Err(Error::Kernel(abi::Error::Denied)));
    assert_eq!(denied.quiesce(), Err(Error::Kernel(abi::Error::Denied)));
    assert_eq!(denied.revoke(), Err(Error::Kernel(abi::Error::Denied)));
    let frames = process::task()
        .allocate(1, ObjectType::FrameAllocation.supported_rights())
        .unwrap();
    for rights in [
        Rights::READ_WRITE,
        Rights::DMA_MAP,
        Rights::READ | Rights::DMA_MAP,
        Rights::WRITE | Rights::DMA_MAP,
    ] {
        let restricted = frames.try_clone(rights).unwrap();
        assert!(matches!(
            domain.map(&restricted, 0, 1, DmaDirection::Bidirectional),
            Err(Error::Kernel(abi::Error::Denied))
        ));
    }
    assert!(matches!(
        domain.map(&frames, 0, 0, DmaDirection::ToDevice),
        Err(Error::Kernel(abi::Error::InvalidArgument))
    ));
    assert!(matches!(
        domain.map(&frames, u64::MAX, 2, DmaDirection::ToDevice),
        Err(Error::Kernel(abi::Error::InvalidArgument))
    ));
}

fn transfer(
    domain: &Handle<DmaDomain>,
    transport: &mut Transport<'_>,
    grant: &Handle<Device>,
    id: u32,
    peer: Option<&PeerDomain>,
) {
    let task = process::task();
    let peer_frames = task
        .allocate(1, ObjectType::FrameAllocation.supported_rights())
        .unwrap();
    let _peer_mapping = peer.map(|peer| {
        let mapping = peer
            .domain
            .map(&peer_frames, 0, 1, DmaDirection::ToDevice)
            .unwrap();
        peer.domain.resume().unwrap();
        mapping
    });
    let ring = task
        .allocate(1, ObjectType::FrameAllocation.supported_rights())
        .unwrap();
    let data = task
        .allocate(1, ObjectType::FrameAllocation.supported_rights())
        .unwrap();
    let mut ring = domain
        .map(&ring, 0, 1, DmaDirection::Bidirectional)
        .unwrap();
    let mut data = domain.map(&data, 0, 1, DmaDirection::FromDevice).unwrap();
    let ring_address = ring.bus_address().unwrap();
    let mut descriptor = [0; 16];
    descriptor[..8].copy_from_slice(&data.bus_address().unwrap().to_le_bytes());
    descriptor[8..12].copy_from_slice(&64u32.to_le_bytes());
    descriptor[12..14].copy_from_slice(&2u16.to_le_bytes());
    ring.write(0, &descriptor).unwrap();
    data.write(0, &[0xa5; 64]).unwrap();
    assert_eq!(
        data.write(4096, &[1]),
        Err(Error::Kernel(abi::Error::InvalidArgument))
    );
    transport.configure(ring_address);
    domain.resume().unwrap();
    if let Some(peer) = peer {
        peer.domain.quiesce().unwrap();
    }
    if matches!(transport, Transport::Pci { .. }) {
        let event = task.event(ObjectType::Event.supported_rights()).unwrap();
        grant
            .bind_interrupt(id, 0, &event, 1)
            .unwrap()
            .unbind()
            .unwrap();
    }
    assert_eq!(domain.state().unwrap(), DmaState::Active);
    assert_eq!(ring.unmap(), Err(Error::Kernel(abi::Error::Busy)));
    assert_eq!(domain.revoke(), Err(Error::Kernel(abi::Error::Busy)));
    transport.start();
    ring.write(132, &0u16.to_le_bytes()).unwrap();
    ring.write(130, &1u16.to_le_bytes()).unwrap();
    transport.notify();
    let deadline = thread::now_ns().unwrap() + 2_000_000_000;
    let mut used = [0; 2];
    loop {
        ring.read(258, &mut used).unwrap();
        if u16::from_le_bytes(used) == 1 {
            break;
        }
        assert!(thread::now_ns().unwrap() < deadline, "DMA did not complete");
        thread::sleep_until(thread::now_ns().unwrap() + 100_000).unwrap();
    }
    let mut completed = [0; 8];
    ring.read(260, &mut completed).unwrap();
    assert_eq!(u32::from_le_bytes(completed[..4].try_into().unwrap()), 0);
    assert_eq!(u32::from_le_bytes(completed[4..].try_into().unwrap()), 64);
    let mut bytes = [0; 64];
    data.read(0, &mut bytes).unwrap();
    assert_ne!(bytes, [0xa5; 64]);
    transport.reset();
    domain.quiesce().unwrap();
    ring.unmap().unwrap();
    data.unmap().unwrap();
}

enum Transport<'a> {
    Mmio(Mmio<'a>),
    Pci {
        registers: Mmio<'a>,
        common: usize,
        notify: usize,
        multiplier: usize,
        queue_notify: usize,
    },
}

impl<'a> Transport<'a> {
    fn open(devices: &Handle<DeviceSet>, grant: &'a Handle<Device>, id: u32) -> Option<Self> {
        let mut config = [0; 4096];
        if devices.pci_config(id, &mut config).is_ok() {
            if config[..4] != [0xf4, 0x1a, 0x44, 0x10] {
                return None;
            }
            let common = capability(&config, 1);
            let notify = capability(&config, 2);
            assert_eq!(common.0, notify.0);
            let bar = devices.pci_bar(id, usize::from(common.0)).unwrap();
            let registers = grant
                .map_mmio(id, usize::from(bar.region.unwrap()))
                .unwrap();
            Some(Self::Pci {
                registers,
                common: common.1,
                notify: notify.1,
                multiplier: notify.2,
                queue_notify: 0,
            })
        } else {
            let mut registers = grant.map_mmio(id, 0).unwrap();
            if registers.read_u32(0).unwrap() != 0x7472_6976 || registers.read_u32(8).unwrap() != 4
            {
                return None;
            }
            assert_eq!(registers.read_u32(4).unwrap(), 2);
            Some(Self::Mmio(registers))
        }
    }

    fn status(&mut self, value: Option<u8>) -> u8 {
        match self {
            Self::Mmio(registers) => {
                if let Some(value) = value {
                    registers.write_u32(0x70, u32::from(value)).unwrap();
                }
                registers.read_u32(0x70).unwrap() as u8
            }
            Self::Pci {
                registers, common, ..
            } => {
                if let Some(value) = value {
                    registers.write_u8(*common + 20, value).unwrap();
                }
                registers.read_u8(*common + 20).unwrap()
            }
        }
    }

    fn reset(&mut self) {
        self.status(Some(0));
        let deadline = thread::now_ns().unwrap() + 1_000_000_000;
        while self.status(None) != 0 {
            assert!(
                thread::now_ns().unwrap() < deadline,
                "VirtIO reset did not complete"
            );
            thread::yield_now().unwrap();
        }
    }

    // REFERENCE: https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html
    fn configure(&mut self, address: u64) {
        self.reset();
        self.status(Some(3));
        match self {
            Self::Mmio(registers) => {
                registers.write_u32(0x14, 1).unwrap();
                assert_ne!(registers.read_u32(0x10).unwrap() & 1, 0);
                for (select, features) in [(0, 0), (1, 1)] {
                    registers.write_u32(0x24, select).unwrap();
                    registers.write_u32(0x20, features).unwrap();
                }
            }
            Self::Pci {
                registers, common, ..
            } => {
                registers.write_u32(*common, 1).unwrap();
                assert_ne!(registers.read_u32(*common + 4).unwrap() & 1, 0);
                for (select, features) in [(0, 0), (1, 1)] {
                    registers.write_u32(*common + 8, select).unwrap();
                    registers.write_u32(*common + 12, features).unwrap();
                }
            }
        }
        assert_eq!(self.status(Some(11)), 11);
        match self {
            Self::Mmio(registers) => {
                registers.write_u32(0x30, 0).unwrap();
                assert!(registers.read_u32(0x34).unwrap() >= 8);
                registers.write_u32(0x38, 8).unwrap();
                for (offset, value) in [
                    (0x80, address),
                    (0x90, address + 128),
                    (0xa0, address + 256),
                ] {
                    write_address(registers, offset, value);
                }
                registers.write_u32(0x44, 1).unwrap();
            }
            Self::Pci {
                registers,
                common,
                queue_notify,
                ..
            } => {
                registers.write_u16(*common + 22, 0).unwrap();
                assert!(registers.read_u16(*common + 24).unwrap() >= 8);
                registers.write_u16(*common + 24, 8).unwrap();
                for (offset, value) in [(32, address), (40, address + 128), (48, address + 256)] {
                    write_address(registers, *common + offset, value);
                }
                *queue_notify = usize::from(registers.read_u16(*common + 30).unwrap());
                registers.write_u16(*common + 28, 1).unwrap();
            }
        }
    }

    fn start(&mut self) {
        assert_eq!(self.status(Some(15)), 15);
    }

    fn notify(&mut self) {
        match self {
            Self::Mmio(registers) => registers.write_u32(0x50, 0).unwrap(),
            Self::Pci {
                registers,
                notify,
                multiplier,
                queue_notify,
                ..
            } => registers
                .write_u16(*notify + *queue_notify * *multiplier, 0)
                .unwrap(),
        }
    }
}

fn write_address(registers: &mut Mmio<'_>, offset: usize, address: u64) {
    registers.write_u32(offset, address as u32).unwrap();
    registers
        .write_u32(offset + 4, (address >> 32) as u32)
        .unwrap();
}

fn capability(config: &[u8; 4096], kind: u8) -> (u8, usize, usize) {
    let mut offset = usize::from(config[0x34]);
    let mut seen = [false; 64];
    while offset != 0 {
        assert!((0x40..=0xfc).contains(&offset) && offset.is_multiple_of(4) && !seen[offset / 4]);
        seen[offset / 4] = true;
        if config[offset] == 9 && config[offset + 3] == kind {
            let start =
                u32::from_le_bytes(config[offset + 8..offset + 12].try_into().unwrap()) as usize;
            let multiplier = if kind == 2 {
                u32::from_le_bytes(config[offset + 16..offset + 20].try_into().unwrap()) as usize
            } else {
                0
            };
            return (config[offset + 4], start, multiplier);
        }
        offset = usize::from(config[offset + 1]);
    }
    panic!("VirtIO capability is missing");
}
