// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Device matching, driver instance lifecycle, and session dispatch.

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};
use devmgr::{
    Error,
    block::{self, Block},
    device::{DeviceDescription, DiscoveryRequest, DriverKind, OpenRequest},
    power::{self, Power},
    rng::{self, Rng},
    rtc::{self, Rtc},
    uart::{self, Settings, Uart},
};
use runtime::{
    Channel, Device, DeviceSet, Event, Handle, KernelError, Rights,
    ipc::{self, WaitItem, WaitMode},
    process, thread,
};

const START_TIMEOUT_NS: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug)]
enum Configuration {
    Uart(uart::Configuration),
    Block(block::Configuration),
    Rng(rng::Configuration),
    Rtc(rtc::Configuration),
    Power(power::Configuration),
}

pub struct DeviceManager {
    catalog: Handle<DeviceSet>,
    wake: Arc<Handle<Event>>,
    instances: Vec<Instance>,
}

struct Instance {
    id: u32,
    configuration: Result<(u32, Configuration), Error>,
    state: InstanceState,
}

enum InstanceState {
    Running(Worker),
    Failed(Error),
}

struct Worker {
    control: Option<Handle<Channel>>,
    thread: thread::JoinHandle<Result<(), Error>>,
    ready: Arc<AtomicBool>,
    grant: Arc<Handle<Device>>,
}

impl DeviceManager {
    pub fn new(catalog: Handle<DeviceSet>) -> Result<Self, Error> {
        let mut manager = Self {
            catalog,
            wake: Arc::new(process::task().event(Rights::SIGNAL | Rights::WAIT)?),
            instances: Vec::new(),
        };
        let mut cursor = 0;
        while let Some(id) = manager.catalog.next(cursor)? {
            cursor = id;
            let device = DeviceDescription::new(&manager.catalog, id)?;
            let configuration = match probe(&device) {
                Ok(Some(configuration)) => device
                    .info()
                    .group
                    .ok_or(Error::InvalidConfiguration)
                    .map(|group| (group, configuration)),
                Ok(None) => continue,
                Err(error) => Err(error),
            };
            manager.instances.push(Instance {
                id,
                configuration,
                state: InstanceState::Failed(Error::Unavailable),
            });
            let index = manager.instances.len() - 1;
            manager.start(index);
        }
        loop {
            manager.reap()?;
            if manager
                .instances
                .iter()
                .all(|instance| match &instance.state {
                    InstanceState::Running(worker) => worker.ready.load(Ordering::Acquire),
                    InstanceState::Failed(_) => true,
                })
            {
                break;
            }
            manager.wait(None)?;
        }
        if manager
            .instances
            .iter()
            .any(|instance| matches!(instance.state, InstanceState::Running(_)))
        {
            Ok(manager)
        } else {
            Err(manager
                .instances
                .iter()
                .find_map(|instance| match instance.state {
                    InstanceState::Failed(error) => Some(error),
                    _ => None,
                })
                .unwrap_or(Error::Unsupported))
        }
    }

    pub fn run(mut self, control: Option<&Handle<Channel>>) -> Result<(), Error> {
        loop {
            self.reap()?;
            if control.is_none()
                && self
                    .instances
                    .iter()
                    .all(|instance| matches!(instance.state, InstanceState::Failed(_)))
            {
                return Err(Error::Unavailable);
            }
            if let Some(control) = control {
                match control.receive(WaitMode::Poll) {
                    Ok(mut message) => {
                        if let Some(request) = DiscoveryRequest::decode(&mut message)? {
                            self.next(request)?;
                            continue;
                        }
                        if let Some(request) = OpenRequest::decode(message)? {
                            self.open(request)?;
                        }
                        continue;
                    }
                    Err(runtime::Error::Kernel(KernelError::WouldBlock)) => {}
                    Err(runtime::Error::Kernel(KernelError::EndOfStream)) => return Ok(()),
                    Err(error) => return Err(error.into()),
                }
            }
            self.wait(control)?;
        }
    }

    fn next(&self, request: DiscoveryRequest) -> Result<(), Error> {
        let id = self.instances.iter()
            .filter(|instance| instance.id > request.after)
            .filter(|instance| matches!(&instance.state, InstanceState::Running(worker) if worker.ready.load(Ordering::Acquire)))
            .filter(|instance| {
                matches!(
                    (instance.configuration, request.kind),
                    (Ok((_, Configuration::Uart(_))), DriverKind::Uart)
                        | (Ok((_, Configuration::Block(_))), DriverKind::Block)
                        | (Ok((_, Configuration::Rng(_))), DriverKind::Rng)
                        | (Ok((_, Configuration::Rtc(_))), DriverKind::Rtc)
                        | (Ok((_, Configuration::Power(_))), DriverKind::Power)
                )
            })
            .map(|instance| instance.id).min();
        request.respond(id)
    }

    fn start(&mut self, index: usize) {
        self.instances[index].state = match self.spawn(index) {
            Ok(worker) => InstanceState::Running(worker),
            Err(error) => InstanceState::Failed(error),
        };
    }

    fn spawn(&self, index: usize) -> Result<Worker, Error> {
        let instance = &self.instances[index];
        let (group, configuration) = instance.configuration?;
        let grant = self.instances.iter().find_map(|other| match &other.state {
            InstanceState::Running(worker)
                if other
                    .configuration
                    .is_ok_and(|(other_group, _)| other_group == group) =>
            {
                Some(Arc::clone(&worker.grant))
            }
            _ => None,
        });
        let grant = match grant {
            Some(grant) => grant,
            None => Arc::new(self.catalog.claim_group(
                group,
                Rights::MAP | Rights::DEVICE_CONTROL | Rights::IRQ_ACK,
            )?),
        };
        let event = process::task().event(Rights::SIGNAL | Rights::WAIT)?;
        let rights = Rights::READ_WRITE | Rights::WAIT;
        let (control, server) = process::task().channel(rights, rights)?;
        let ready = Arc::new(AtomicBool::new(false));
        let worker_ready = Arc::clone(&ready);
        let wake = Arc::clone(&self.wake);
        let worker_grant = Arc::clone(&grant);
        let id = instance.id;
        let thread = thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || {
                let result = serve(
                    id,
                    configuration,
                    &worker_grant,
                    &event,
                    &server,
                    &worker_ready,
                    &wake,
                );
                worker_ready.store(false, Ordering::Release);
                if let Err(error) = result {
                    while let Ok(message) = server.receive(WaitMode::Poll) {
                        if let Ok(Some(request)) = OpenRequest::decode(message) {
                            let _ = request.reject(error);
                        }
                    }
                }
                result
            })?;
        Ok(Worker {
            control: Some(control),
            thread,
            ready,
            grant,
        })
    }

    fn reap(&mut self) -> Result<(), Error> {
        for instance in &mut self.instances {
            let InstanceState::Running(worker) = &instance.state else {
                continue;
            };
            if !worker.thread.is_finished()? {
                continue;
            }
            let InstanceState::Running(worker) = core::mem::replace(
                &mut instance.state,
                InstanceState::Failed(Error::Unavailable),
            ) else {
                unreachable!()
            };
            let result = worker.thread.join().map_err(|_| Error::Unavailable)?;
            instance.state = InstanceState::Failed(result.err().unwrap_or(Error::Unavailable));
        }
        Ok(())
    }

    fn open(&mut self, request: OpenRequest) -> Result<(), Error> {
        let Some(index) = self
            .instances
            .iter()
            .position(|instance| instance.id == request.device)
        else {
            return request.reject(Error::Unsupported);
        };
        if matches!(self.instances[index].state, InstanceState::Failed(_)) {
            self.start(index);
        }
        match &self.instances[index].state {
            InstanceState::Running(worker) if worker.ready.load(Ordering::Acquire) => {
                request.forward(worker.control.as_ref().unwrap())
            }
            InstanceState::Running(_) => request.reject(Error::Busy),
            InstanceState::Failed(error) => request.reject(*error),
        }
    }

    fn wait(&self, control: Option<&Handle<Channel>>) -> Result<(), Error> {
        let mut items = Vec::with_capacity(self.instances.len() + 2);
        items.push(WaitItem::new(&self.wake, 1));
        if let Some(control) = control {
            items.push(WaitItem::new(control, ipc::READABLE | ipc::PEER_CLOSED));
        }
        for instance in &self.instances {
            if let InstanceState::Running(worker) = &instance.state {
                items.push(WaitItem::new(worker.thread.thread(), ipc::EXITED));
            }
        }
        ipc::wait_many(&items, WaitMode::Forever)?;
        Ok(())
    }
}

impl Drop for DeviceManager {
    fn drop(&mut self) {
        for instance in &mut self.instances {
            if let InstanceState::Running(worker) = &mut instance.state {
                worker.control.take();
            }
        }
        for instance in self.instances.drain(..) {
            if let InstanceState::Running(worker) = instance.state {
                let _ = worker.thread.join();
            }
        }
    }
}

fn serve(
    id: u32,
    configuration: Configuration,
    grant: &Handle<Device>,
    event: &Handle<Event>,
    server: &Handle<Channel>,
    ready: &AtomicBool,
    wake: &Handle<Event>,
) -> Result<(), Error> {
    match configuration {
        Configuration::Rtc(configuration) => {
            let mut rtc = Rtc::new(configuration, grant)?;
            ready.store(true, Ordering::Release);
            wake.signal(1)?;
            return rtc::service::run(&mut rtc, server, id);
        }
        Configuration::Power(configuration) => {
            let mut power = Power::new(configuration, grant)?;
            ready.store(true, Ordering::Release);
            wake.signal(1)?;
            return power::service::run(&mut power, server, id);
        }
        Configuration::Block(configuration) => {
            let domain = grant.create_dma(
                id,
                Rights::READ | Rights::DMA_MAP | Rights::DEVICE_CONTROL | Rights::REVOKE,
            )?;
            let mut block = Block::new(configuration, grant, &domain, event, process::task())?;
            ready.store(true, Ordering::Release);
            wake.signal(1)?;
            block::service::run(&mut block, event, server, id)?;
            return Ok(domain.revoke()?);
        }
        Configuration::Rng(configuration) => {
            let domain = grant.create_dma(
                id,
                Rights::READ | Rights::DMA_MAP | Rights::DEVICE_CONTROL | Rights::REVOKE,
            )?;
            let mut rng = Rng::new(configuration, grant, &domain, event, process::task())?;
            ready.store(true, Ordering::Release);
            wake.signal(1)?;
            rng::service::run(&mut rng, event, server, id)?;
            return Ok(domain.revoke()?);
        }
        Configuration::Uart(_) => {}
    }
    let Configuration::Uart(configuration) = configuration else {
        unreachable!()
    };
    let deadline = thread::now_ns()?.saturating_add(START_TIMEOUT_NS);
    let mut uart = loop {
        match Uart::new(configuration, grant, event, 1, Settings::default()) {
            Ok(uart) => break uart,
            Err(Error::Busy) if thread::now_ns()? < deadline => thread::yield_now()?,
            Err(error) => return Err(error),
        }
    };
    ready.store(true, Ordering::Release);
    wake.signal(1)?;
    uart::service::run(&mut uart, event, Some(server), id)
}

fn probe(device: &DeviceDescription<'_>) -> Result<Option<Configuration>, Error> {
    if let Some(configuration) = uart::probe(device)? {
        return Ok(Some(Configuration::Uart(configuration)));
    }
    if let Some(configuration) = block::probe(device)? {
        return Ok(Some(Configuration::Block(configuration)));
    }
    if let Some(configuration) = rng::probe(device)? {
        return Ok(Some(Configuration::Rng(configuration)));
    }
    if let Some(configuration) = rtc::probe(device)? {
        return Ok(Some(Configuration::Rtc(configuration)));
    }
    Ok(power::probe(device)?.map(Configuration::Power))
}
