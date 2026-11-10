// SPDX-License-Identifier: MPL-2.0

use crate::{
    Error, Services,
    elf::{Image, Machine, PAGE_SIZE},
    file::terminal::Terminal,
    process::Process,
    protocol::{self, Arguments},
};
use alloc::{rc::Rc, vec::Vec};
use fs::ReadAt;
use runtime::{
    Handle, Reply, Task, TrapPort,
    ipc::{READABLE, ReceivedMessage, WaitItem},
};

pub(super) struct Loading<'a> {
    pub reply: Handle<Reply>,
    arguments: Arguments,
    terminal: Rc<Terminal>,
    stage: Stage<'a>,
}

enum Stage<'a> {
    Open(fs::client::PendingOpen<'a>),
    Read(Reading<'a>),
    Entropy {
        bytes: Vec<u8>,
        entropy: Entropy,
    },
    Ready {
        bytes: Vec<u8>,
        random: Option<[u8; 16]>,
    },
}

struct Reading<'a> {
    file: fs::File<'a>,
    pending: fs::client::PendingRead,
    bytes: Vec<u8>,
}

struct Entropy {
    request: devmgr::rng::PendingRead,
    bytes: Vec<u8>,
}

impl<'a> Loading<'a> {
    pub fn new(
        files: &fs::Client<'a>,
        mut request: ReceivedMessage,
        reply: Handle<Reply>,
    ) -> Result<Self, (Error, Handle<Reply>)> {
        let decoded = (|| {
            if request.tag != protocol::RUN || !matches!(request.handles().len(), 2 | 3) {
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
            let mut session = || {
                devmgr::uart::Session::from_channel(
                    request
                        .take_handle(0)
                        .unwrap()
                        .downcast()
                        .map_err(|_| Error::InvalidRequest)?,
                )
                .map_err(Error::from)
            };
            let output = session()?;
            let input = if request.handles().is_empty() {
                None
            } else {
                Some(devmgr::uart::Session::from_channel(
                    request
                        .take_handle(0)
                        .unwrap()
                        .downcast()
                        .map_err(|_| Error::InvalidRequest)?,
                )?)
            };
            let stage = Stage::Open(files.start_open(&arguments.path)?);
            Ok((arguments, Rc::new(Terminal::new(output, input)), stage))
        })();
        match decoded {
            Ok((arguments, terminal, stage)) => Ok(Self {
                reply,
                arguments,
                terminal,
                stage,
            }),
            Err(error) => Err((error, reply)),
        }
    }

    pub fn poll(
        &mut self,
        task: &'a Handle<Task>,
        port: &Handle<TrapPort>,
        machine: Machine,
        services: &Services<'_>,
    ) -> Result<Option<Process<'a>>, Error> {
        match &mut self.stage {
            Stage::Open(pending) => match pending.try_complete() {
                Ok(file) => self.stage = Stage::Read(Reading::new(file)?),
                Err(fs::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => {}
                Err(error) => return Err(error.into()),
            },
            Stage::Read(read) => {
                if read.poll()? {
                    let bytes = core::mem::take(&mut read.bytes);
                    self.stage = if let Some(entropy) = &services.entropy {
                        Stage::Entropy {
                            bytes,
                            entropy: Entropy {
                                request: entropy.start_read(16)?,
                                bytes: Vec::new(),
                            },
                        }
                    } else {
                        Stage::Ready {
                            bytes,
                            random: None,
                        }
                    };
                }
            }
            Stage::Entropy { bytes, entropy } => {
                if let Some(random) = entropy.poll(services)? {
                    self.stage = Stage::Ready {
                        bytes: core::mem::take(bytes),
                        random: Some(random),
                    };
                }
            }
            Stage::Ready { .. } => {}
        }
        if let Stage::Ready { bytes, random } = &self.stage {
            let image = Image::parse(bytes, machine)?;
            return Process::start(
                task,
                port,
                machine,
                &image,
                &self.arguments,
                Rc::clone(&self.terminal),
                random.as_ref(),
            )
            .map(Some);
        }
        Ok(None)
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<WaitItem<'b>>) {
        let call = match &self.stage {
            Stage::Open(pending) => pending.call(),
            Stage::Read(read) => read.pending.call(),
            Stage::Entropy { entropy, .. } => entropy.request.call(),
            Stage::Ready { .. } => return,
        };
        items.push(WaitItem::new(call, READABLE));
    }
}

impl<'a> Reading<'a> {
    fn new(file: fs::File<'a>) -> Result<Self, Error> {
        if file.is_empty() {
            return Err(Error::InvalidExecutable);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(file.len())
            .map_err(|_| Error::NoMemory)?;
        let pending = file.start_read_at(0, file.len().min(PAGE_SIZE))?;
        Ok(Self {
            file,
            pending,
            bytes,
        })
    }

    fn poll(&mut self) -> Result<bool, Error> {
        match self.pending.try_complete() {
            Ok(chunk) => {
                if chunk.is_empty() {
                    return Err(Error::InvalidExecutable);
                }
                self.bytes.extend_from_slice(&chunk);
                if self.bytes.len() == self.file.len() {
                    return Ok(true);
                }
                self.pending = self.file.start_read_at(
                    self.bytes.len(),
                    (self.file.len() - self.bytes.len()).min(PAGE_SIZE),
                )?;
            }
            Err(fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => {}
            Err(error) => return Err(error.into()),
        }
        Ok(false)
    }
}

impl Entropy {
    fn poll(&mut self, services: &Services<'_>) -> Result<Option<[u8; 16]>, Error> {
        match self.request.try_complete() {
            Ok(bytes) => {
                self.bytes.extend_from_slice(&bytes);
                if self.bytes.len() == 16 {
                    return Ok(Some(self.bytes.as_slice().try_into().unwrap()));
                }
                self.request = services
                    .entropy
                    .as_ref()
                    .unwrap()
                    .start_read(16 - self.bytes.len())?;
            }
            Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                runtime::KernelError::WouldBlock,
            ))) => {}
            Err(error) => return Err(error.into()),
        }
        Ok(None)
    }
}
