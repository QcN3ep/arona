// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Asynchronous reads of executable files and startup entropy.

use crate::{Error, Services, elf::PAGE_SIZE};
use alloc::vec::Vec;
use fs::ReadAt;
use runtime::{Call, Handle};

pub(crate) struct Executable<'a> {
    stage: Stage<'a>,
}

enum Stage<'a> {
    Open(fs::client::PendingNode<'a>),
    Check {
        file: Option<fs::File<'a>>,
        pending: fs::client::PendingMetadata,
    },
    Read(Reading<'a>),
    Entropy {
        bytes: Vec<u8>,
        request: devmgr::rng::PendingRead,
        random: Vec<u8>,
    },
    Ready(Loaded),
}

pub(crate) struct Loaded {
    pub bytes: Vec<u8>,
    pub random: Option<[u8; 16]>,
    pub script: Option<(Vec<u8>, Vec<u8>)>,
}

struct Reading<'a> {
    file: fs::File<'a>,
    pending: fs::client::PendingRead,
    bytes: Vec<u8>,
}

impl<'a> Executable<'a> {
    pub fn new(
        files: &fs::Client<'a>,
        directory: Option<&fs::Directory<'a>>,
        path: &[u8],
    ) -> Result<Self, Error> {
        let request = files.start_open_at(
            directory,
            path,
            fs::OpenOptions {
                write: false,
                create: false,
                exclusive: false,
                truncate: false,
                permissions: 0,
            },
            true,
            false,
        )?;
        Ok(Self {
            stage: Stage::Open(request),
        })
    }

    pub fn call(&self) -> Option<&Handle<Call>> {
        match &self.stage {
            Stage::Open(request) => Some(request.call()),
            Stage::Check { pending, .. } => Some(pending.call()),
            Stage::Read(read) => Some(read.pending.call()),
            Stage::Entropy { request, .. } => Some(request.call()),
            Stage::Ready(_) => None,
        }
    }

    pub fn poll(&mut self, services: &Services<'_>) -> Result<Option<Loaded>, Error> {
        match &mut self.stage {
            Stage::Open(request) => {
                if let Some(node) = completed(request.try_complete())? {
                    let fs::client::OpenedNode::File(file) = node else {
                        return Err(Error::PermissionDenied);
                    };
                    let pending = file.start_metadata()?;
                    self.stage = Stage::Check {
                        file: Some(file),
                        pending,
                    };
                }
            }
            Stage::Check { file, pending } => {
                if let Some(metadata) = completed(pending.try_complete())? {
                    if metadata.mode & 0o111 == 0 {
                        return Err(Error::PermissionDenied);
                    }
                    self.stage = Stage::Read(Reading::new(file.take().unwrap())?);
                }
            }
            Stage::Read(read) => {
                if read.poll()? {
                    let bytes = core::mem::take(&mut read.bytes);
                    self.stage = next_stage(bytes, services)?;
                }
            }
            Stage::Entropy {
                bytes,
                request,
                random,
            } => match request.try_complete() {
                Ok(data) => {
                    random.extend_from_slice(&data);
                    if random.len() == 16 {
                        self.stage = Stage::Ready(Loaded {
                            bytes: core::mem::take(bytes),
                            random: Some(random.as_slice().try_into().unwrap()),
                            script: None,
                        });
                    } else {
                        *request = services
                            .entropy
                            .as_ref()
                            .unwrap()
                            .start_read(16 - random.len())?;
                    }
                }
                Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => {}
                Err(error) => return Err(error.into()),
            },
            Stage::Ready(_) => {}
        }
        if matches!(self.stage, Stage::Ready(_)) {
            let Stage::Ready(loaded) = core::mem::replace(
                &mut self.stage,
                Stage::Ready(Loaded {
                    bytes: Vec::new(),
                    random: None,
                    script: None,
                }),
            ) else {
                unreachable!()
            };
            return Ok(Some(loaded));
        }
        Ok(None)
    }
}

fn completed<T>(result: Result<T, fs::Error>) -> Result<Option<T>, Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(fs::Error::Runtime(runtime::Error::Kernel(runtime::KernelError::WouldBlock))) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
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
            bytes,
            pending,
        })
    }

    fn poll(&mut self) -> Result<bool, Error> {
        let Some(chunk) = completed(self.pending.try_complete())? else {
            return Ok(false);
        };
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
        Ok(false)
    }
}

fn next_stage<'a>(bytes: Vec<u8>, services: &Services<'_>) -> Result<Stage<'a>, Error> {
    let script = parse_script(&bytes);
    if script.is_some() {
        return Ok(Stage::Ready(Loaded {
            bytes,
            random: None,
            script,
        }));
    }
    if let Some(source) = &services.entropy {
        return Ok(Stage::Entropy {
            bytes,
            request: source.start_read(16)?,
            random: Vec::new(),
        });
    }
    Ok(Stage::Ready(Loaded {
        bytes,
        random: None,
        script: None,
    }))
}

fn parse_script(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let line = bytes.strip_prefix(b"#!")?;
    let end = line
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(line.len())
        .min(256);
    let mut line = &line[..end];
    while line
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        line = &line[1..];
    }
    while line
        .last()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r'))
    {
        line = &line[..line.len() - 1];
    }
    let split = line
        .iter()
        .position(|byte| matches!(byte, b' ' | b'\t'))
        .unwrap_or(line.len());
    let (interpreter, argument) = line.split_at(split);
    if interpreter.is_empty() {
        return None;
    }
    Some((interpreter.to_vec(), argument.to_vec()))
}
