// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::process::image::{Chain, Executable, MAX_INTERPRETERS, parse_script, script_arguments};
use crate::{
    Error, Services,
    elf::{Image, ImageSet, Machine},
    file::terminal::Terminal,
    process::Process,
    protocol::{self, Arguments},
};
use alloc::{rc::Rc, vec::Vec};
use runtime::{
    Handle, Reply, Task, TrapPort,
    ipc::{READABLE, ReceivedMessage, WaitItem},
};

pub(super) struct Loading<'a> {
    pub reply: Handle<Reply>,
    arguments: Arguments,
    terminal: Rc<Terminal>,
    executable: Chain<'a>,
    interpreters: usize,
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
            let stage = Chain::new(Executable::new(files, None, &arguments.path)?);
            Ok((arguments, Rc::new(Terminal::new(output, input)), stage))
        })();
        match decoded {
            Ok((arguments, terminal, stage)) => Ok(Self {
                reply,
                arguments,
                terminal,
                executable: stage,
                interpreters: 0,
            }),
            Err(error) => Err((error, reply)),
        }
    }

    pub fn poll(
        &mut self,
        task: &'a Handle<Task>,
        port: &Rc<Handle<TrapPort>>,
        files: &fs::Client<'a>,
        machine: Machine,
        services: &Services<'_>,
        namespace: &Rc<crate::process::namespace::PidNamespace>,
    ) -> Result<Option<Process<'a>>, Error> {
        if let Some(loaded) = self.executable.poll(files, machine, services)? {
            if loaded.main.bytes.starts_with(b"#!") {
                if self.interpreters >= MAX_INTERPRETERS {
                    return Err(Error::InvalidExecutable);
                }
                let (interpreter, argument) = parse_script(&loaded.main.bytes)?;
                let executable = Executable::new(files, None, interpreter)?;
                script_arguments(&mut self.arguments, interpreter, argument);
                self.interpreters += 1;
                self.executable = Chain::new(executable);
                return Ok(None);
            }
            let image = Image::parse(&loaded.main.bytes, machine)?;
            let interpreter = loaded
                .interpreter
                .as_ref()
                .map(|loaded| Image::parse_at(&loaded.bytes, machine, image.end()))
                .transpose()?;
            if interpreter
                .as_ref()
                .is_some_and(|interpreter| interpreter.interpreter.is_some())
            {
                return Err(Error::InvalidExecutable);
            }
            return Process::start(
                task,
                port,
                &ImageSet { image, interpreter },
                &self.arguments,
                Rc::clone(&self.terminal),
                loaded.main.random.as_ref(),
                Rc::clone(namespace),
            )
            .map(Some);
        }
        Ok(None)
    }

    pub fn waits<'b>(&'b self, items: &mut Vec<WaitItem<'b>>) {
        if let Some(call) = self.executable.call() {
            items.push(WaitItem::new(call, READABLE));
        }
    }
}
