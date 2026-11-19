// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Startup and asynchronous event handling for the Linux service.

mod load;
mod loading;

use alloc::{rc::Rc, vec::Vec};
use core::cell::RefCell;
use fs::ReadAt;
use runtime::{
    Channel, Handle, Reply, Rights, Task,
    ipc::{self, Message, PEER_CLOSED, READABLE, ReceivedMessage, WaitItem, WaitMode},
};

use crate::{Error, Services, elf::Machine, process::Process, protocol};
use loading::Loading;

pub const REQUIRED_RIGHTS: Rights = runtime::process::REQUIRED_RIGHTS
    .union(Rights::CREATE_TASK)
    .union(Rights::CREATE_THREAD)
    .union(Rights::CREATE_CHANNEL)
    .union(Rights::CREATE_EVENT)
    .union(Rights::CREATE_TIMER)
    .union(Rights::SET_TRAP);

struct Running<'a> {
    process: Process<'a>,
    reply: Option<Handle<Reply>>,
}

struct Guests<'a> {
    loading: Vec<Loading<'a>>,
    running: Vec<Running<'a>>,
}

pub fn run(task: &Handle<Task>, control: Handle<Channel>) -> Result<(), Error> {
    let native = native_resources(control.receive(WaitMode::Forever)?)?;
    let mut startup = control.receive(WaitMode::Forever)?;
    let reply = startup.take_reply().ok_or(Error::InvalidRequest)?;
    let files = bootstrap(task, startup);
    let (files, machine, mut services) = match files {
        Ok(files) => files,
        Err(error) => {
            reply_error(reply, error)?;
            return Err(error);
        }
    };
    services.system = native.system;
    services.generator = native.generator;
    services.initialize(task)?;
    let mut sampler = services
        .system
        .as_ref()
        .map(|system| load::Sampler::new(task, system))
        .transpose()?;
    let shared = crate::process::Shared {
        namespace: Rc::new(crate::process::namespace::PidNamespace::new()),
        shm: Rc::new(RefCell::new(crate::shm::Table::new())),
        mappings: Rc::default(),
    };
    let port = alloc::rc::Rc::new(task.trap_port(Rights::READ | Rights::WAIT)?);
    reply.send(&mut Message::new(0, &[])?)?;
    let mut guests = Guests {
        loading: Vec::new(),
        running: Vec::new(),
    };
    loop {
        if let Some(sampler) = &mut sampler {
            sampler.poll(services.system.as_ref().unwrap(), &services.loads)?;
        }
        let mut progress = poll_loading(
            &mut guests,
            task,
            &port,
            &files,
            machine,
            &services,
            &shared,
        )?;
        progress |= poll_running(&mut guests.running, &files, &services)?;
        match port.receive(WaitMode::Poll) {
            Ok(trap) => {
                if let Some(owner) = guests.running.iter_mut().find(|running| {
                    running
                        .process
                        .threads
                        .iter()
                        .any(|thread| thread.native_id == trap.info.thread)
                }) {
                    owner.process.trap(trap, &files, &services)?;
                }
                progress = true;
            }
            Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
            Err(error) => return Err(error.into()),
        }
        match control.receive(WaitMode::Poll) {
            Ok(mut request) => {
                if let Some(reply) = request.take_reply() {
                    match Loading::new(task, &files, request, reply) {
                        Ok(request) => guests.loading.push(request),
                        Err((error, reply)) => reply_error(reply, error)?,
                    }
                }
                progress = true;
            }
            Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
            Err(runtime::Error::Kernel(
                runtime::KernelError::PeerClosed | runtime::KernelError::EndOfStream,
            )) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        if !progress {
            wait(
                &control,
                &port,
                &guests.loading,
                &guests.running,
                sampler.as_ref().map(load::Sampler::event),
            )?;
        }
    }
}

fn bootstrap(
    task: &Handle<Task>,
    mut startup: ReceivedMessage,
) -> Result<(fs::Client<'_>, Machine, Services<'_>), Error> {
    let (flags, reserved) = match startup.words() {
        [flags, reserved] if flags & !63 == 0 && *reserved <= u64::from(u32::MAX) + 1 => {
            (*flags, *reserved)
        }
        _ => return Err(Error::InvalidRequest),
    };
    if startup.tag != protocol::START || startup.handles().len() != 1 + flags.count_ones() as usize
    {
        return Err(Error::InvalidRequest);
    }
    let channel = startup
        .take_handle(0)
        .unwrap()
        .downcast()
        .map_err(|_| Error::InvalidRequest)?;
    let files = fs::Client::from_channel(task, channel);
    let deadline = runtime::thread::now_ns()?
        .checked_add(10_000_000_000)
        .ok_or(Error::InvalidRequest)?;
    let native = files.open("/bin/linux", deadline)?;
    let mut header = [0; 64];
    native.read_exact_at(0, &mut header)?;
    let machine = Machine::from_header(&header)?;
    let mut services = Services::default();
    if flags & 1 != 0 {
        services.clock = Some(devmgr::rtc::Session::from_channel(
            startup
                .take_handle(0)
                .unwrap()
                .downcast()
                .map_err(|_| Error::InvalidRequest)?,
        )?);
    }
    if flags & 2 != 0 {
        services.entropy = Some(devmgr::rng::Session::from_channel(
            task,
            startup
                .take_handle(0)
                .unwrap()
                .downcast()
                .map_err(|_| Error::InvalidRequest)?,
        )?);
    }
    if flags & 4 != 0 {
        services.net = Some(
            net::Client::from_channel(
                task,
                startup
                    .take_handle(0)
                    .unwrap()
                    .downcast()
                    .map_err(|_| Error::InvalidRequest)?,
            )
            .map_err(|_| Error::InvalidRequest)?,
        );
    }
    let kernel_log = if flags & 8 != 0 {
        Some(
            startup
                .take_handle(0)
                .unwrap()
                .downcast()
                .map_err(|_| Error::InvalidRequest)?,
        )
    } else {
        None
    };
    services.log = crate::Log::new(kernel_log);
    if flags & 16 != 0 {
        services.mounts = Some(fs::MountControl::from_channel(
            task,
            startup
                .take_handle(0)
                .unwrap()
                .downcast()
                .map_err(|_| Error::InvalidRequest)?,
        )?);
    }
    if reserved != 0 {
        services.reserved = Some((reserved - 1) as u32);
    }
    if flags & 32 != 0 {
        services.devices = Some(
            startup
                .take_handle(0)
                .unwrap()
                .downcast()
                .map_err(|_| Error::InvalidRequest)?,
        );
    }
    Ok((files, machine, services))
}

fn reply_error(reply: Handle<Reply>, error: Error) -> Result<(), Error> {
    let _ = reply.send(&mut Message::new(protocol::error(error), &[])?);
    Ok(())
}

fn poll_loading<'a>(
    guests: &mut Guests<'a>,
    task: &'a Handle<Task>,
    port: &alloc::rc::Rc<Handle<runtime::TrapPort>>,
    files: &fs::Client<'a>,
    machine: Machine,
    services: &Services<'a>,
    shared: &crate::process::Shared,
) -> Result<bool, Error> {
    let mut progress = false;
    let mut index = 0;
    while index < guests.loading.len() {
        match guests.loading[index].poll(task, port, files, machine, services, shared) {
            Ok(Some(process)) => {
                let pending = guests.loading.swap_remove(index);
                guests.running.push(Running {
                    process,
                    reply: Some(pending.reply),
                });
            }
            Ok(None) => {
                index += 1;
                continue;
            }
            Err(error) => {
                reply_error(guests.loading.swap_remove(index).reply, error)?;
            }
        }
        progress = true;
    }

    Ok(progress)
}

fn poll_running<'a>(
    running: &mut Vec<Running<'a>>,
    files: &fs::Client<'a>,
    services: &Services<'a>,
) -> Result<bool, Error> {
    let mut progress = false;
    let mut index = 0;
    while index < running.len() {
        progress |= running[index].process.poll(files, services)?;
        let children = core::mem::take(&mut running[index].process.children);
        for process in children {
            running.push(Running {
                process,
                reply: None,
            });
            progress = true;
        }
        if running[index].process.threads.is_empty() {
            let finished = running.swap_remove(index);
            let status = finished.process.status();
            finished
                .process
                .namespace
                .exit(finished.process.pid, status, finished.process.cpu_ns);
            drop(finished.process);
            if let Some(reply) = finished.reply {
                let _ = reply.send(&mut Message::new(0, &[status.encode() as u64])?);
            }
            progress = true;
        } else {
            index += 1;
        }
    }

    Ok(progress)
}

fn wait(
    control: &Handle<Channel>,
    port: &Handle<runtime::TrapPort>,
    loading: &[Loading<'_>],
    running: &[Running<'_>],
    load_event: Option<&Handle<runtime::Event>>,
) -> Result<(), Error> {
    let mut items = alloc::vec![
        WaitItem::new(control, READABLE | PEER_CLOSED),
        WaitItem::new(port, READABLE)
    ];
    let terminals: alloc::collections::BTreeMap<_, _> = running
        .iter()
        .map(|guest| {
            let terminal = &guest.process.terminal;
            (alloc::rc::Rc::as_ptr(terminal), terminal)
        })
        .collect();
    if let Some(event) = load_event {
        items.push(WaitItem::new(event, 1));
    }
    let inputs: Vec<_> = terminals
        .values()
        .filter_map(|terminal| terminal.input())
        .collect();
    for input in &inputs {
        items.push(WaitItem::new(input.call(), READABLE));
        if let Some(call) = input.echo_call() {
            items.push(WaitItem::new(call, READABLE));
        }
    }
    for terminal in terminals.values() {
        // One wake covers every writer the terminal holds stopped; the loop re-polls
        // all pendings and each writer re-checks the stop state itself.
        if let Some(event) = terminal.resume_event() {
            items.push(WaitItem::new(event, 1));
        }
    }
    for pending in loading {
        pending.waits(&mut items);
    }
    for guest in running {
        guest.process.waits(&mut items);
    }
    let deadline = running
        .iter()
        .filter_map(|guest| guest.process.deadline())
        .min();
    match ipc::wait_many(&items, deadline.map_or(WaitMode::Forever, WaitMode::Until)) {
        // A deadline only wakes the loop so the owning pending operation can
        // report its own timeout; it must not stop the service.
        Ok(_) | Err(runtime::Error::Kernel(runtime::KernelError::TimedOut)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn native_resources(mut message: ReceivedMessage) -> Result<Services<'static>, Error> {
    let flags = match message.words() {
        [flags] if flags & !3 == 0 => *flags,
        _ => return Err(Error::InvalidRequest),
    };
    if message.tag != protocol::RESOURCES
        || message.take_reply().is_some()
        || message.handles().len() != flags.count_ones() as usize
    {
        return Err(Error::InvalidRequest);
    }
    let system = if flags & 1 != 0 {
        Some(
            message
                .take_handle(0)
                .unwrap()
                .downcast::<runtime::SystemInfo>()
                .map_err(|_| Error::InvalidRequest)?,
        )
    } else {
        None
    };
    let generator = if flags & 2 != 0 {
        Some(
            message
                .take_handle(0)
                .unwrap()
                .downcast::<runtime::Entropy>()
                .map_err(|_| Error::InvalidRequest)?,
        )
    } else {
        None
    };
    if system.as_ref().is_some_and(|handle| {
        !handle
            .info()
            .is_ok_and(|info| info.rights.contains(Rights::READ))
    }) || generator.as_ref().is_some_and(|handle| {
        !handle
            .info()
            .is_ok_and(|info| info.rights.contains(Rights::READ))
    }) {
        return Err(Error::InvalidRequest);
    }
    Ok(Services {
        system,
        generator,
        ..Services::default()
    })
}
