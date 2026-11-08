// SPDX-License-Identifier: MPL-2.0

//! Linux 服务的启动与异步事件处理。

mod loading;

use alloc::vec::Vec;
use fs::ReadAt;
use runtime::{
    Channel, Handle, Reply, Rights, Task,
    ipc::{self, Message, PEER_CLOSED, READABLE, ReceivedMessage, WaitItem, WaitMode},
};

use crate::{Error, elf::Machine, process::Process, protocol};
use loading::Loading;

pub const REQUIRED_RIGHTS: Rights = runtime::process::ProcessStart::REQUIRED_RIGHTS
    .union(Rights::CREATE_TASK)
    .union(Rights::CREATE_THREAD)
    .union(Rights::CREATE_CHANNEL)
    .union(Rights::SET_TRAP);

struct Running {
    process: Process,
    reply: Handle<Reply>,
}

pub fn run(task: &Handle<Task>, control: Handle<Channel>) -> Result<(), Error> {
    let mut startup = control.receive(WaitMode::Forever)?;
    let reply = startup.take_reply().ok_or(Error::InvalidRequest)?;
    let files = bootstrap(task, startup);
    let (files, machine) = match files {
        Ok(files) => files,
        Err(error) => {
            reply_error(reply, error)?;
            return Err(error);
        }
    };
    let port = task.trap_port(Rights::READ | Rights::WAIT)?;
    reply.send(&mut Message::new(0, &[])?)?;
    let mut loading: Vec<Loading<'_>> = Vec::new();
    let mut running: Vec<Running> = Vec::new();
    loop {
        let mut progress = poll_loading(&mut loading, &mut running, task, &port, machine)?;
        progress |= poll_running(&mut running)?;
        match port.receive(WaitMode::Poll) {
            Ok(trap) => {
                if let Some(owner) = running.iter_mut().find(|running| {
                    running
                        .process
                        .threads
                        .iter()
                        .any(|thread| thread.native_id == trap.info.thread)
                }) {
                    owner.process.trap(trap)?;
                }
                progress = true;
            }
            Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
            Err(error) => return Err(error.into()),
        }
        match control.receive(WaitMode::Poll) {
            Ok(mut request) => {
                if let Some(reply) = request.take_reply() {
                    match Loading::new(&files, request, reply) {
                        Ok(request) => loading.push(request),
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
            let mut items = alloc::vec![
                WaitItem::new(&control, READABLE | PEER_CLOSED),
                WaitItem::new(&port, READABLE)
            ];
            for pending in &loading {
                pending.waits(&mut items);
            }
            for guest in &running {
                guest.process.waits(&mut items);
            }
            ipc::wait_many(&items, WaitMode::Forever)?;
        }
    }
}

fn bootstrap(
    task: &Handle<Task>,
    mut startup: ReceivedMessage,
) -> Result<(fs::Client<'_>, Machine), Error> {
    if startup.tag != protocol::START || !startup.words().is_empty() || startup.handles().len() != 1
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
    Ok((files, machine))
}

fn reply_error(reply: Handle<Reply>, error: Error) -> Result<(), Error> {
    let _ = reply.send(&mut Message::new(protocol::error(error), &[])?);
    Ok(())
}

fn poll_loading(
    loading: &mut Vec<Loading<'_>>,
    running: &mut Vec<Running>,
    task: &Handle<Task>,
    port: &Handle<runtime::TrapPort>,
    machine: Machine,
) -> Result<bool, Error> {
    let mut progress = false;
    let mut index = 0;
    while index < loading.len() {
        match loading[index].poll(task, port, machine) {
            Ok(Some(process)) => {
                let pending = loading.swap_remove(index);
                running.push(Running {
                    process,
                    reply: pending.reply,
                });
            }
            Ok(None) => {
                index += 1;
                continue;
            }
            Err(error) => {
                reply_error(loading.swap_remove(index).reply, error)?;
            }
        }
        progress = true;
    }

    Ok(progress)
}

fn poll_running(running: &mut Vec<Running>) -> Result<bool, Error> {
    let mut progress = false;
    let mut index = 0;
    while index < running.len() {
        progress |= running[index].process.poll()?;
        if running[index].process.threads.is_empty() {
            let finished = running.swap_remove(index);
            let status = finished.process.status();
            drop(finished.process);
            let _ = finished
                .reply
                .send(&mut Message::new(0, &[status.encode() as u64])?);
            progress = true;
        } else {
            index += 1;
        }
    }

    Ok(progress)
}
