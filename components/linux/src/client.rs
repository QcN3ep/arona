// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use runtime::{
    Channel, Handle, Rights, Task,
    ipc::{Message, PendingCall, WaitMode},
};

use crate::{
    Error, ExitStatus,
    protocol::{self, CHANNEL_RIGHTS, transfer},
};

pub struct Program<'a> {
    pub path: &'a [u8],
    pub arguments: &'a [&'a [u8]],
    pub environment: &'a [&'a [u8]],
}

#[derive(Default)]
pub struct Services<'a> {
    pub clock: Option<devmgr::rtc::Session>,
    pub entropy: Option<devmgr::rng::Session<'a>>,
    pub net: Option<net::Client<'a>>,
}

pub struct Control<'a> {
    task: &'a Handle<Task>,
    channel: Handle<Channel>,
}

impl<'a> Control<'a> {
    pub fn start(
        task: &'a Handle<Task>,
        channel: Handle<Channel>,
        files: fs::Client<'_>,
        deadline: u64,
    ) -> Result<Self, Error> {
        Self::start_with_services(task, channel, files, Services::default(), deadline)
    }

    pub fn start_with_services(
        task: &'a Handle<Task>,
        channel: Handle<Channel>,
        files: fs::Client<'_>,
        services: Services<'_>,
        deadline: u64,
    ) -> Result<Self, Error> {
        let flags = u64::from(services.clock.is_some())
            | (u64::from(services.entropy.is_some()) << 1)
            | (u64::from(services.net.is_some()) << 2);
        let mut request = Message::new(protocol::START, &[flags])?;
        transfer(&mut request, files.into_channel(), CHANNEL_RIGHTS)?;
        if let Some(clock) = services.clock {
            transfer(&mut request, clock.into_channel(), CHANNEL_RIGHTS)?;
        }
        if let Some(entropy) = services.entropy {
            transfer(&mut request, entropy.into_channel(), CHANNEL_RIGHTS)?;
        }
        if let Some(net) = services.net {
            transfer(&mut request, net.into_channel(), CHANNEL_RIGHTS)?;
        }
        let response = channel
            .call(&mut request, Some(deadline))
            .map_err(|error| match error {
                runtime::ipc::CallError::NotSubmitted(error)
                | runtime::ipc::CallError::Submitted(error) => error,
            })?;
        if !protocol::response(response)?.words().is_empty() {
            return Err(runtime::Error::Protocol.into());
        }
        Ok(Self { task, channel })
    }

    pub fn start_run(
        &self,
        program: Program<'_>,
        output: devmgr::uart::Session,
    ) -> Result<PendingCall<ExitStatus, (), Error>, Error> {
        self.request_run(program, output, None)
    }

    pub fn start_run_with_input(
        &self,
        program: Program<'_>,
        output: devmgr::uart::Session,
        input: devmgr::uart::Session,
    ) -> Result<PendingCall<ExitStatus, (), Error>, Error> {
        self.request_run(program, output, Some(input))
    }

    fn request_run(
        &self,
        program: Program<'_>,
        output: devmgr::uart::Session,
        input: Option<devmgr::uart::Session>,
    ) -> Result<PendingCall<ExitStatus, (), Error>, Error> {
        if program.path.first() != Some(&b'/') || program.arguments.is_empty() {
            return Err(Error::InvalidRequest);
        }
        let strings = core::iter::once(program.path)
            .chain(program.arguments.iter().copied())
            .chain(program.environment.iter().copied());
        let mut bytes = alloc::vec::Vec::new();
        for string in strings {
            if string.contains(&0) {
                return Err(Error::InvalidRequest);
            }
            bytes
                .try_reserve(string.len().checked_add(1).ok_or(Error::InvalidRequest)?)
                .map_err(|_| Error::NoMemory)?;
            bytes.extend_from_slice(string);
            bytes.push(0);
        }
        let mut frames = self.task.allocate(
            bytes.len().div_ceil(4096) as u64,
            Rights::READ_WRITE | Rights::TRANSFER,
        )?;
        frames.write(0, &bytes)?;
        let mut request = Message::new(
            protocol::RUN,
            &[
                bytes.len() as u64,
                program.arguments.len() as u64,
                program.environment.len() as u64,
            ],
        )?;
        transfer(&mut request, frames, Rights::READ)?;
        transfer(&mut request, output.into_channel(), CHANNEL_RIGHTS)?;
        if let Some(input) = input {
            transfer(&mut request, input.into_channel(), CHANNEL_RIGHTS)?;
        }
        Ok(PendingCall::new(
            self.channel.start_call(&mut request)?,
            (),
            |(), response| {
                let response = protocol::response(response)?;
                let [status] = *response.words() else {
                    return Err(runtime::Error::Protocol.into());
                };
                if status & !0xff7f != 0 || status & 0xff00 != 0 && status & 127 != 0 {
                    return Err(runtime::Error::Protocol.into());
                }
                Ok(ExitStatus::decode(status as usize))
            },
        ))
    }

    pub fn run(
        &self,
        program: Program<'_>,
        output: devmgr::uart::Session,
        mode: WaitMode,
    ) -> Result<ExitStatus, Error> {
        self.start_run(program, output)?.wait(mode)
    }
}
