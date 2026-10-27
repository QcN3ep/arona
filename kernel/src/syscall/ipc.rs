// SPDX-License-Identifier: MPL-2.0

use crate::{
    ipc::{Channel, Event, MESSAGE_HANDLES, Message, ReceivedMessage, Timer, WaitItem, WaitMode},
    memory::MemoryBuffer,
    task::{Object, ObjectReference, Task, TaskError, TaskState, TransferRequest},
};
use abi::{Error, Handle, ObjectType, Return, Rights, method};
use arrayvec::ArrayVec;

pub(super) fn create(
    caller: &Task,
    target: &Task,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    if !matches!(target.state(), TaskState::Created | TaskState::Active) {
        return Err(Error::InvalidState);
    }
    match method {
        method::task::CHANNEL => {
            let first = rights(args[0], ObjectType::Channel)?;
            let second = rights(args[1], ObjectType::Channel)?;
            let [first, second] = super::object::create(caller, [first, second], || {
                let (first, second) = Channel::pair()?;
                Ok([Object::Channel(first), Object::Channel(second)])
            })?;
            Ok([first.raw(), second.raw()])
        }
        method::task::EVENT => {
            let rights = rights(args[0], ObjectType::Event)?;
            let [event] =
                super::object::create(caller, [rights], || Ok([Object::Event(Event::new()?)]))?;
            Ok([event.raw(), 0])
        }
        method::task::TIMER => {
            let rights = rights(args[2], ObjectType::Timer)?;
            let ObjectReference::Event(event) =
                caller.get(Handle::from_raw(args[0]), ObjectType::Event, Rights::SIGNAL)?
            else {
                unreachable!()
            };
            let [timer] = super::object::create(caller, [rights], || {
                Ok([Object::Timer(Timer::new(event, args[1])?)])
            })?;
            Ok([timer.raw(), 0])
        }
        _ => Err(Error::Unsupported),
    }
}
fn rights(raw: u64, kind: ObjectType) -> Result<Rights, Error> {
    let rights = Rights::from_bits(raw).ok_or(Error::InvalidArgument)?;
    if !kind.supported_rights().contains(rights) {
        return Err(Error::InvalidArgument);
    }
    Ok(rights)
}

pub(super) fn invoke(
    caller: &Task,
    handle: Handle,
    object: ObjectReference,
    method: u64,
    args: [u64; 5],
) -> Result<Return, Error> {
    match object {
        ObjectReference::Channel(_) => match method {
            method::channel::SEND => {
                let (message, transfers) = read_message(caller, args[0])?;
                caller.send_with(handle, message, &transfers, mode(args[1]))?;
            }
            method::channel::RECEIVE => return receive(caller, handle, args),
            method::channel::CALL => return request(caller, handle, args),
            _ => return Err(Error::Unsupported),
        },
        ObjectReference::Reply(_) => {
            let (message, transfers) = read_message(caller, args[0])?;
            caller.reply(handle, message, &transfers)?;
        }
        ObjectReference::Event(event) => event.signal(args[0])?,
        ObjectReference::Timer(timer) => match method {
            method::timer::ARM => timer.arm(args[0])?,
            method::timer::CANCEL => timer.cancel()?,
            _ => return Err(Error::Unsupported),
        },
        _ => return Err(Error::Unsupported),
    }
    Ok(Return::success([0; 2]))
}
fn read_message(
    caller: &Task,
    address: u64,
) -> Result<(Message, ArrayVec<TransferRequest, MESSAGE_HANDLES>), Error> {
    let input = caller.pin_user_memory(address as usize, abi::ipc::Message::SIZE, false)?;
    let mut bytes = [0; abi::ipc::Message::SIZE];
    input.read(0, &mut bytes)?;
    let wire = abi::ipc::Message::decode(&bytes)?;
    if wire.has_reply {
        return Err(Error::InvalidArgument);
    }
    let mut transfers = ArrayVec::new();
    for descriptor in wire.handles[..wire.handle_count].iter().flatten() {
        let (kind, _) = caller.handle_info(descriptor.handle)?;
        if kind != descriptor.kind {
            return Err(Error::WrongType);
        }
        transfers.push(TransferRequest {
            handle: descriptor.handle,
            rights: descriptor.rights,
        });
    }
    Ok((
        Message::new(wire.tag, &wire.words[..wire.word_count])?,
        transfers,
    ))
}
fn receive(caller: &Task, handle: Handle, args: [u64; 5]) -> Result<Return, Error> {
    if args[1] > MESSAGE_HANDLES as u64 {
        return Err(Error::InvalidArgument);
    }
    let output = caller.pin_user_memory(args[0] as usize, abi::ipc::Message::SIZE, true)?;
    caller.receive_deliver(
        handle,
        args[1] as usize,
        mode(args[2]),
        |response, entries| {
            write_message(&output, response, |index| {
                Ok((entries[index].object.kind(), entries[index].rights))
            })
        },
    )?;
    Ok(Return::success([abi::ipc::Message::SIZE as u64, 0]))
}
fn request(caller: &Task, handle: Handle, args: [u64; 5]) -> Result<Return, Error> {
    let (message, transfers) = read_message(caller, args[0])?;
    let output = caller.pin_user_memory(args[1] as usize, abi::ipc::Message::SIZE, true)?;
    let mut committed = false;
    let result = caller.call_deliver(
        handle,
        message,
        &transfers,
        (args[2] != u64::MAX).then_some(args[2]),
        &mut committed,
        |response| {
            write_message(&output, response, |index| {
                caller.handle_info(response.handles()[index])
            })
        },
    );
    Ok(match result {
        Ok(_) => Return::success([abi::ipc::Message::SIZE as u64, 1]),
        Err(error) => Return {
            values: [0, u64::from(committed)],
            ..Return::failure(error.into())
        },
    })
}
fn write_message(
    output: &MemoryBuffer,
    response: &ReceivedMessage,
    mut descriptor: impl FnMut(usize) -> Result<(ObjectType, Rights), TaskError>,
) -> Result<(), TaskError> {
    let mut wire = abi::ipc::Message::new(response.message.tag);
    wire.word_count = response.message.words().len();
    wire.words[..wire.word_count].copy_from_slice(response.message.words());
    wire.handle_count = response.handles().len();
    wire.has_reply = response.has_reply();
    for (index, handle) in response.handles().iter().enumerate() {
        let (kind, rights) = descriptor(index)?;
        wire.handles[index] = Some(abi::ipc::HandleDescriptor {
            handle: *handle,
            kind,
            rights,
        });
    }
    let bytes = wire
        .encode()
        .map_err(|_| crate::ipc::ChannelError::InvalidMessage)?;
    Ok(output.write(0, &bytes)?)
}
fn mode(deadline: u64) -> WaitMode {
    match deadline {
        0 => WaitMode::NonBlocking,
        u64::MAX => WaitMode::Blocking,
        deadline => WaitMode::Deadline(deadline),
    }
}

pub(super) fn wait_many(caller: &Task, args: [u64; 5]) -> Result<[u64; 2], Error> {
    if args[1] == 0 || args[1] > abi::ipc::WAIT_ITEMS as u64 {
        return Err(Error::InvalidArgument);
    }
    let input = caller.pin_user_memory(
        args[0] as usize,
        args[1] as usize * abi::ipc::WaitItem::SIZE,
        false,
    )?;
    let mut items = ArrayVec::<_, { abi::ipc::WAIT_ITEMS }>::new();
    for index in 0..args[1] as usize {
        let mut bytes = [0; abi::ipc::WaitItem::SIZE];
        input.read(index * bytes.len(), &mut bytes)?;
        let wire = abi::ipc::WaitItem::decode(&bytes);
        items.push(WaitItem {
            handle: wire.handle,
            condition_mask: wire.conditions,
        });
    }
    drop(input);
    let result = match mode(args[2]) {
        WaitMode::NonBlocking => caller.try_wait_many(&items)?,
        WaitMode::Blocking => caller.wait_many(&items)?,
        WaitMode::Deadline(deadline) => caller.wait_many_until(&items, deadline)?,
    };
    Ok([result.index as u64, result.ready])
}
