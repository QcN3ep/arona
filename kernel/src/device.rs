// SPDX-License-Identifier: MPL-2.0

//! 设备资源授权、用户映射与中断投递。

// TODO: 支持设备热插拔及对应的设备目录更新与授权回收。

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::{Mutex, Once};

use crate::{
    DeviceProperty, DeviceUsePolicy, DmaCapability, HardwareDeviceInfo, HardwareDeviceInstanceId,
    HardwareDeviceLocation, HardwareInfo, HardwareInterruptInfo,
    hardware::{
        self,
        cpu::InterruptGuard,
        interrupt::{InterruptError, InterruptSource},
    },
    ipc::EventReference,
    memory::{self, AddressSpace, HeapArc, MemoryError, PhysRange, UserDeviceMapping, VirtRange},
};
pub use abi::device::{DeviceField, DeviceInfo};

pub mod dma;
mod pci;
mod reference;

static REGISTRY: Once<Registry> = Once::new();
static GRANT_TOKENS: AtomicUsize = AtomicUsize::new(0);

pub fn active_grant_tokens() -> usize {
    GRANT_TOKENS.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceError {
    NotInitialized,
    AlreadyInitialized,
    InvalidArgument,
    Denied,
    Busy,
    Closed,
    Unsupported,
    Memory(MemoryError),
    Interrupt(InterruptError),
    Pci(hardware::pci::PciConfigError),
}

impl From<MemoryError> for DeviceError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}
impl From<InterruptError> for DeviceError {
    fn from(error: InterruptError) -> Self {
        Self::Interrupt(error)
    }
}

/// # Safety
///
/// 操作须在中断控制器所属处理器执行并保留内核映射。
#[derive(Clone)]
pub struct DeviceSet(());

/// # Safety
///
/// 操作及析构须在中断控制器所属处理器执行并保留内核映射。
pub struct Device {
    reference: DeviceReference,
}

/// 不计入开放句柄，最后一个 Device 析构时撤销授权。
#[derive(Clone)]
pub struct DeviceReference(HeapArc<GrantToken>);

struct GrantToken {
    id: u64,
    credits: AtomicUsize,
    revoked: AtomicBool,
}

struct Descriptor {
    hardware: HardwareDeviceInfo,
    group: Option<u32>,
    pci: Option<pci::Configuration>,
}

struct Registry {
    devices: Vec<Descriptor>,
    properties: Vec<DeviceProperty>,
    console: Option<HardwareDeviceInstanceId>,
    state: Mutex<DeviceState>,
}

struct DeviceState {
    grants: Vec<Grant>,
    routes: Vec<IrqRoute>,
    next_id: u64,
}

struct Grant {
    token: DeviceReference,
    group: u32,
    mappings: Vec<MappedRegion>,
    domains: Vec<dma::Domain>,
}

struct MappedRegion {
    mapping: HeapArc<UserDeviceMapping>,
    range: VirtRange,
}

struct IrqRoute {
    owner: u64,
    group: u32,
    interrupt: HardwareInterruptInfo,
    event: EventReference,
    mask: u64,
    pending: bool,
}

impl DeviceSet {
    /// # Safety
    ///
    /// 初始化及设备操作须在中断控制器所属处理器执行并保留内核映射。
    pub fn initialize(hardware: &HardwareInfo) -> Result<Self, DeviceError> {
        let _interrupts = InterruptGuard::new();
        let mut initialized = false;
        REGISTRY.try_call_once(|| {
            let registry = Registry::new(hardware)?;
            initialized = true;
            Ok::<_, DeviceError>(registry)
        })?;
        if initialized {
            Ok(Self(()))
        } else {
            Err(DeviceError::AlreadyInitialized)
        }
    }

    pub fn next(&self, after: u32) -> Result<Option<u32>, DeviceError> {
        let registry = registry()?;
        Ok(registry
            .devices
            .iter()
            .map(|device| device.hardware.instance_id.0)
            .filter(|id| *id > after)
            .min())
    }

    pub fn describe(&self, id: u32) -> Result<DeviceInfo, DeviceError> {
        let registry = registry()?;
        let device = registry.device(id)?;
        let hardware = &device.hardware;
        Ok(DeviceInfo {
            id,
            parent: hardware.parent.map(|id| id.0),
            group: device.group,
            mmio_regions: hardware.mmio_regions.len(),
            interrupts: hardware.interrupts.len(),
            identities: hardware.identities.len(),
            properties: registry
                .properties
                .iter()
                .filter(|property| property.device == hardware.instance_id)
                .count(),
            console: registry.console == Some(hardware.instance_id),
            dma: abi::dma::DmaInfo {
                profile: hardware::dma().profile(),
                address_bits: hardware::dma().address_bits(hardware).unwrap_or(0),
            },
        })
    }

    pub fn read(
        &self,
        id: u32,
        field: DeviceField,
        index: usize,
    ) -> Result<&'static [u8], DeviceError> {
        let registry = registry()?;
        let descriptor = registry.device(id)?;
        let device = &descriptor.hardware;
        let bytes = match field {
            DeviceField::Name if index == 0 => device.name.as_bytes(),
            DeviceField::IdentityNamespace | DeviceField::Identity => {
                let identity = device
                    .identities
                    .get(index)
                    .ok_or(DeviceError::InvalidArgument)?;
                if field == DeviceField::IdentityNamespace {
                    identity.namespace.as_bytes()
                } else {
                    identity.value.as_bytes()
                }
            }
            DeviceField::PropertyNamespace
            | DeviceField::PropertyName
            | DeviceField::PropertyValue => {
                let property = registry
                    .properties
                    .iter()
                    .filter(|property| property.device.0 == id)
                    .nth(index)
                    .ok_or(DeviceError::InvalidArgument)?;
                match field {
                    DeviceField::PropertyNamespace => property.namespace.as_bytes(),
                    DeviceField::PropertyName => property.name.as_bytes(),
                    _ => property.value.as_slice(),
                }
            }
            DeviceField::PciConfig if index == 0 => descriptor
                .pci
                .as_ref()
                .ok_or(DeviceError::InvalidArgument)?
                .snapshot(),
            DeviceField::PciBar => descriptor
                .pci
                .as_ref()
                .and_then(|config| config.bars.get(index))
                .and_then(Option::as_ref)
                .ok_or(DeviceError::InvalidArgument)?
                .as_slice(),
            _ => return Err(DeviceError::InvalidArgument),
        };
        Ok(bytes)
    }

    pub fn claim_group(&self, group: u32) -> Result<Device, DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        if !registry
            .devices
            .iter()
            .any(|device| device.group == Some(group))
        {
            return Err(DeviceError::Denied);
        }
        let mut state = registry.state.lock();
        if state
            .grants
            .iter()
            .any(|grant| grant.group == group && !grant.token.0.revoked.load(Ordering::Acquire))
        {
            return Err(DeviceError::Busy);
        }
        state
            .grants
            .try_reserve(1)
            .map_err(|_| MemoryError::NoMemory)?;
        let id = state.next_id;
        state.next_id = id.checked_add(1).ok_or(MemoryError::NoMemory)?;
        let reference = DeviceReference(memory::manager().allocate_arc(GrantToken {
            id,
            credits: AtomicUsize::new(1),
            revoked: AtomicBool::new(false),
        })?);
        GRANT_TOKENS.fetch_add(1, Ordering::Relaxed);
        registry.pci_memory(group, true)?;
        state.grants.push(Grant {
            token: reference.clone(),
            group,
            mappings: Vec::new(),
            domains: Vec::new(),
        });
        Ok(Device { reference })
    }
}

impl Drop for GrantToken {
    fn drop(&mut self) {
        GRANT_TOKENS.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Device {
    pub fn reference(&self) -> DeviceReference {
        self.reference.clone()
    }

    pub fn try_clone(&self) -> Result<Self, DeviceError> {
        if self.reference.0.revoked.load(Ordering::Acquire) {
            return Err(DeviceError::Closed);
        }
        self.reference
            .0
            .credits
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |credits| {
                credits.checked_add(1)
            })
            .map_err(|_| MemoryError::ReferenceOverflow)?;
        Ok(Self {
            reference: self.reference(),
        })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if self.reference.0.credits.fetch_sub(1, Ordering::AcqRel) == 1
            && let Err(error) = self.reference.revoke()
        {
            log::error!("Failed to release device grant: {error:?}");
            hardware::cpu().halt();
        }
    }
}

impl Registry {
    fn new(hardware: &HardwareInfo) -> Result<Self, DeviceError> {
        let mut registry = Self {
            devices: Vec::new(),
            properties: Vec::new(),
            console: hardware.console.as_ref().map(|console| console.device),
            state: Mutex::new(DeviceState {
                grants: Vec::new(),
                routes: Vec::new(),
                next_id: 1,
            }),
        };
        if let Err(error) = registry.populate(hardware) {
            pci::rollback(&registry.devices)?;
            return Err(error);
        }
        Ok(registry)
    }

    fn populate(&mut self, hardware: &HardwareInfo) -> Result<(), DeviceError> {
        let devices = &mut self.devices;
        devices
            .try_reserve_exact(hardware.devices.len())
            .map_err(|_| MemoryError::NoMemory)?;
        for device in &hardware.devices {
            devices.push(Descriptor {
                hardware: device.clone(),
                group: Some(device.instance_id.0),
                pci: None,
            });
        }
        let first_pci_id = devices
            .iter()
            .map(|device| device.hardware.instance_id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(DeviceError::InvalidArgument)?;
        let properties = &mut self.properties;
        properties
            .try_reserve_exact(hardware.device_properties.len())
            .map_err(|_| MemoryError::NoMemory)?;
        properties.extend(hardware.device_properties.iter().cloned());
        let count = pci::enumerate(hardware, first_pci_id, properties, devices)?;
        log::info!("Enumerated {count} PCI devices");
        log::info!("DMA profile: {:?}", hardware::dma().profile());
        if hardware::dma().profile() == abi::dma::DmaProfile::CompetitionTrusted {
            log::warn!("Trusted DMA does not isolate malicious device accesses");
        }
        for device in devices.iter_mut() {
            device.group = Some(device.hardware.instance_id.0);
        }
        // 共享 MMIO 页或 IRQ 的设备属于同一授权组。
        for left in 0..devices.len() {
            for right in 0..left {
                if shares_resources(&devices[left], &devices[right])? {
                    let from = devices[left].group;
                    let to = devices[right].group;
                    for device in devices.iter_mut() {
                        if device.group == from {
                            device.group = to;
                        }
                    }
                }
            }
        }
        for index in 0..devices.len() {
            let device = &devices[index].hardware;
            if device.use_policy != DeviceUsePolicy::UserAssignable
                || (devices[index].pci.is_none()
                    && (device.dma != DmaCapability::None
                        || device.location != HardwareDeviceLocation::Platform)
                    && hardware::dma().address_bits(device).is_none())
            {
                let group = devices[index].group;
                if group.is_some() {
                    for device in devices.iter_mut() {
                        if device.group == group {
                            device.group = None;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn device(&self, id: u32) -> Result<&Descriptor, DeviceError> {
        self.devices
            .iter()
            .find(|device| device.hardware.instance_id.0 == id)
            .ok_or(DeviceError::InvalidArgument)
    }

    fn pci_memory(&self, group: u32, enabled: bool) -> Result<(), DeviceError> {
        let result = self
            .devices
            .iter()
            .filter(|device| device.group == Some(group))
            .filter_map(|device| device.pci.as_ref())
            .try_for_each(|config| config.command(enabled, false));
        if result.is_err() {
            for device in self
                .devices
                .iter()
                .filter(|device| device.group == Some(group))
            {
                if let Some(config) = &device.pci {
                    config.command(false, false)?;
                }
            }
        }
        result
    }

    fn pci_interrupts(
        &self,
        group: u32,
        interrupt: HardwareInterruptInfo,
        enabled: bool,
    ) -> Result<(), DeviceError> {
        let configs = || {
            self.devices
                .iter()
                .filter(|device| {
                    device.group == Some(group)
                        && device
                            .hardware
                            .interrupts
                            .iter()
                            .any(|source| same_source(*source, interrupt))
                })
                .filter_map(|device| device.pci.as_ref())
        };
        let result = configs().try_for_each(|config| config.interrupts(enabled));
        if result.is_err() {
            for config in configs() {
                config.interrupts(false)?;
            }
        }
        result
    }

    fn member(&self, group: u32, id: u32) -> Result<&HardwareDeviceInfo, DeviceError> {
        let device = self.device(id)?;
        if device.group != Some(group) {
            return Err(DeviceError::Denied);
        }
        Ok(&device.hardware)
    }

    fn interrupt(
        &self,
        group: u32,
        id: u32,
        index: usize,
    ) -> Result<HardwareInterruptInfo, DeviceError> {
        self.member(group, id)?
            .interrupts
            .get(index)
            .copied()
            .ok_or(DeviceError::InvalidArgument)
    }

    fn envelope(&self, group: u32, physical: PhysRange) -> Result<PhysRange, DeviceError> {
        let mut envelope = physical
            .page_envelope()
            .map_err(|_| DeviceError::InvalidArgument)?;
        loop {
            let previous = envelope;
            for device in self
                .devices
                .iter()
                .filter(|device| device.group == Some(group))
            {
                for range in &device.hardware.mmio_regions {
                    let pages = range
                        .page_envelope()
                        .map_err(|_| DeviceError::InvalidArgument)?;
                    if envelope.overlaps(pages) {
                        envelope.start = envelope.start.min(pages.start);
                        envelope.end = envelope.end.max(pages.end);
                    }
                }
            }
            if envelope == previous {
                return Ok(envelope);
            }
        }
    }
}

impl DeviceState {
    fn grant(&self, reference: &DeviceReference) -> Result<&Grant, DeviceError> {
        if reference.0.revoked.load(Ordering::Acquire) {
            return Err(DeviceError::Closed);
        }
        self.grants
            .iter()
            .find(|grant| grant.token.0.id == reference.0.id)
            .ok_or(DeviceError::Closed)
    }

    fn remove_released(&mut self) {
        self.grants.retain(|grant| {
            !grant.token.0.revoked.load(Ordering::Acquire) || !grant.mappings.is_empty()
        });
    }
}

pub(crate) fn dispatch_interrupt() -> Result<(), DeviceError> {
    let _interrupts = InterruptGuard::new();
    let Some(claim) = hardware::interrupt_controller().claim()? else {
        return Ok(());
    };
    let notification = if let Some(registry) = REGISTRY.get() {
        let mut state = registry.state.lock();
        if let Some(route) = state
            .routes
            .iter_mut()
            .find(|route| InterruptSource::from(route.interrupt) == claim.source())
        {
            hardware::interrupt_controller().mask(claim.source())?;
            claim.complete()?;
            route.pending = true;
            Some((route.event.clone(), route.mask))
        } else {
            None
        }
    } else {
        None
    };
    if let Some((event, mask)) = notification {
        let _ = event.signal(mask);
    }
    Ok(())
}

pub(crate) fn release_space(space: &HeapArc<AddressSpace>) {
    let _interrupts = InterruptGuard::new();
    if let Some(registry) = REGISTRY.get() {
        let mut state = registry.state.lock();
        for grant in &mut state.grants {
            grant
                .mappings
                .retain(|mapped| !mapped.mapping.belongs_to(space));
        }
        state.remove_released();
    }
}

pub(crate) fn close_event(event: &EventReference) {
    let _interrupts = InterruptGuard::new();
    if let Some(registry) = REGISTRY.get() {
        let mut state = registry.state.lock();
        for route in state.routes.iter().filter(|route| route.event.same(event)) {
            if let Err(error) = registry.pci_interrupts(route.group, route.interrupt, false) {
                log::error!("Failed to disable PCI interrupt: {error:?}");
                hardware::cpu().halt();
            }
            if let Err(error) = hardware::interrupt_controller().mask(route.interrupt.into()) {
                log::error!("Failed to detach device interrupt: {error:?}");
                hardware::cpu().halt();
            }
        }
        state.routes.retain(|route| !route.event.same(event));
    }
}

fn registry() -> Result<&'static Registry, DeviceError> {
    REGISTRY.get().ok_or(DeviceError::NotInitialized)
}

fn same_source(left: HardwareInterruptInfo, right: HardwareInterruptInfo) -> bool {
    InterruptSource::from(left) == InterruptSource::from(right)
}

fn shares_resources(left: &Descriptor, right: &Descriptor) -> Result<bool, DeviceError> {
    for a in resource_ranges(left) {
        for b in resource_ranges(right) {
            if a.page_envelope()
                .map_err(|_| DeviceError::InvalidArgument)?
                .overlaps(
                    b.page_envelope()
                        .map_err(|_| DeviceError::InvalidArgument)?,
                )
            {
                return Ok(true);
            }
        }
    }
    Ok(left.hardware.interrupts.iter().any(|a| {
        right
            .hardware
            .interrupts
            .iter()
            .any(|b| same_source(*a, *b))
    }))
}

fn resource_ranges(device: &Descriptor) -> impl Iterator<Item = PhysRange> + '_ {
    let ecam = match &device.hardware.location {
        HardwareDeviceLocation::PciHostBridge { ecam, .. } => Some(*ecam),
        _ => None,
    };
    device
        .hardware
        .mmio_regions
        .iter()
        .copied()
        .chain(ecam)
        .chain(
            device
                .pci
                .iter()
                .flat_map(|config| config.unmapped_bars.iter().copied()),
        )
}
