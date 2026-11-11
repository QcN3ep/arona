// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::process::image::Executable;
use crate::{
    Error, Services,
    elf::{Image, Machine},
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
    executable: Executable<'a>,
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
            let stage = Executable::new(files, None, &arguments.path)?;
            Ok((arguments, Rc::new(Terminal::new(output, input)), stage))
        })();
        match decoded {
            Ok((arguments, terminal, stage)) => Ok(Self {
                reply,
                arguments,
                terminal,
                executable: stage,
            }),
            Err(error) => Err((error, reply)),
        }
    }

    pub fn poll(
        &mut self,
        task: &'a Handle<Task>,
        port: &Rc<Handle<TrapPort>>,
        machine: Machine,
        services: &Services<'_>,
        namespace: &Rc<crate::process::namespace::PidNamespace>,
    ) -> Result<Option<Process<'a>>, Error> {
        if let Some(loaded) = self.executable.poll(services)? {
            let bytes = &loaded.bytes;
            let random = &loaded.random;
            let image = Image::parse(bytes, machine)?;
            return Process::start(
                task,
                port,
                &image,
                &self.arguments,
                Rc::clone(&self.terminal),
                random.as_ref(),
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
