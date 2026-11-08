// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use fs::ReadAt;
use runtime::{
    Handle, Reply, Task, TrapPort,
    ipc::{READABLE, ReceivedMessage, WaitItem},
};

use crate::{
    Error,
    elf::{Image, Machine, PAGE_SIZE},
    process::Process,
    protocol::{self, Arguments},
};

pub(super) struct Loading<'a> {
    pub reply: Handle<Reply>,
    arguments: Arguments,
    output: Option<devmgr::uart::Session>,
    stage: Stage<'a>,
}

enum Stage<'a> {
    Open(fs::client::PendingOpen<'a>),
    Read {
        file: fs::File<'a>,
        pending: fs::client::PendingRead,
        bytes: Vec<u8>,
    },
}

impl<'a> Loading<'a> {
    pub fn new(
        files: &fs::Client<'a>,
        mut request: ReceivedMessage,
        reply: Handle<Reply>,
    ) -> Result<Self, (Error, Handle<Reply>)> {
        let decoded = (|| {
            if request.tag != protocol::RUN || request.handles().len() != 2 {
                return Err(Error::InvalidRequest);
            }
            let [length, argc, envc] = *request.words() else {
                return Err(Error::InvalidRequest);
            };
            let convert = |value| usize::try_from(value).map_err(|_| Error::InvalidRequest);
            let frames = request
                .take_handle(0)
                .unwrap()
                .downcast()
                .map_err(|_| Error::InvalidRequest)?;
            let arguments =
                Arguments::decode(&frames, convert(length)?, convert(argc)?, convert(envc)?)?;
            let output = devmgr::uart::Session::from_channel(
                request
                    .take_handle(0)
                    .unwrap()
                    .downcast()
                    .map_err(|_| Error::InvalidRequest)?,
            )?;
            let stage = Stage::Open(files.start_open(&arguments.path)?);
            Ok((arguments, output, stage))
        })();
        match decoded {
            Ok((arguments, output, stage)) => Ok(Self {
                reply,
                arguments,
                output: Some(output),
                stage,
            }),
            Err(error) => Err((error, reply)),
        }
    }

    pub fn poll(
        &mut self,
        task: &Handle<Task>,
        port: &Handle<TrapPort>,
        machine: Machine,
    ) -> Result<Option<Process>, Error> {
        match &mut self.stage {
            Stage::Open(pending) => match pending.try_complete() {
                Ok(file) => {
                    if file.is_empty() {
                        return Err(Error::InvalidExecutable);
                    }
                    let mut bytes = Vec::new();
                    bytes
                        .try_reserve_exact(file.len())
                        .map_err(|_| Error::NoMemory)?;
                    let pending = file.start_read_at(0, file.len().min(PAGE_SIZE))?;
                    self.stage = Stage::Read {
                        file,
                        pending,
                        bytes,
                    };
                }
                Err(fs::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => {}
                Err(error) => return Err(error.into()),
            },
            Stage::Read {
                file,
                pending,
                bytes,
            } => match pending.try_complete() {
                Ok(chunk) => {
                    if chunk.is_empty() {
                        return Err(Error::InvalidExecutable);
                    }
                    bytes.extend_from_slice(&chunk);
                    if bytes.len() == file.len() {
                        let image = Image::parse(bytes, machine)?;
                        return Process::start(
                            task,
                            port,
                            machine,
                            &image,
                            &self.arguments,
                            self.output.take().unwrap(),
                        )
                        .map(Some);
                    }
                    *pending =
                        file.start_read_at(bytes.len(), (file.len() - bytes.len()).min(PAGE_SIZE))?;
                }
                Err(fs::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => {}
                Err(error) => return Err(error.into()),
            },
        }
        Ok(None)
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<WaitItem<'b>>) {
        let call = match &self.stage {
            Stage::Open(pending) => pending.call(),
            Stage::Read { pending, .. } => pending.call(),
        };
        items.push(WaitItem::new(call, READABLE));
    }
}
