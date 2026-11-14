// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Startup and asynchronous event handling for the Linux service.

mod loading;

use alloc::vec::Vec;
use fs::ReadAt;
use runtime::{
    Channel, Handle, Reply, Rights, Task,
    ipc::{self, Message, PEER_CLOSED, READABLE, ReceivedMessage, WaitItem, WaitMode},
};

use crate::{Error, Services, elf::Machine, process::Process, protocol};
use loading::Loading;

pub const REQUIRED_RIGHTS: Rights = runtime::process::ProcessStart::REQUIRED_RIGHTS
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
    let mut startup = control.receive(WaitMode::Forever)?;
    let reply = startup.take_reply().ok_or(Error::InvalidRequest)?;
    let files = bootstrap(task, startup);
    let (files, machine, services) = match files {
        Ok(files) => files,
        Err(error) => {
            reply_error(reply, error)?;
            return Err(error);
        }
    };
    let namespace = alloc::rc::Rc::new(crate::process::namespace::PidNamespace::new());
    let port = alloc::rc::Rc::new(task.trap_port(Rights::READ | Rights::WAIT)?);
    reply.send(&mut Message::new(0, &[])?)?;
    let mut guests = Guests {
        loading: Vec::new(),
        running: Vec::new(),
    };
    loop {
        let mut progress = poll_loading(
            &mut guests,
            task,
            &port,
            &files,
            machine,
            &services,
            &namespace,
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
            wait(&control, &port, &guests.loading, &guests.running)?;
        }
    }
}

fn bootstrap(
    task: &Handle<Task>,
    mut startup: ReceivedMessage,
) -> Result<(fs::Client<'_>, Machine, Services<'_>), Error> {
    let flags = match startup.words() {
        [flags] if flags & !7 == 0 => *flags,
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
    namespace: &alloc::rc::Rc<crate::process::namespace::PidNamespace>,
) -> Result<bool, Error> {
    let mut progress = false;
    let mut index = 0;
    while index < guests.loading.len() {
        match guests.loading[index].poll(task, port, files, machine, services, namespace) {
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
        items.push(WaitItem::new(terminal.resume_event(), 1));
    }
    for pending in loading {
        pending.waits(&mut items);
    }
    for guest in running {
        guest.process.waits(&mut items);
    }
    ipc::wait_many(&items, WaitMode::Forever)?;
    Ok(())
}
