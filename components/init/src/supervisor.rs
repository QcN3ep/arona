// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Supervision of startup, exit, and restart for configured programs.

// TODO: Support configuration reload and service dependency management.
// TODO: Stop services in dependency order and complete flushing, unmounting, and shutdown or restart.

use crate::{
    InitError,
    config::{Config, Executor, Program, Restart, Terminal},
    service::Service,
};
use alloc::{string::String, vec::Vec};
use runtime::{
    Call, Channel, Error, Handle, KernelError,
    ipc::{self, PendingCall, WaitItem, WaitMode},
    process, thread,
};

type Execution = PendingCall<linux::ExitStatus, (), linux::Error>;

struct ProgramState {
    program: Program,
    phase: Phase,
    started: u64,
    failures: u32,
}

enum Phase {
    Ready(u64),
    Output(devmgr::uart::PendingOpen),
    Input {
        output: devmgr::uart::Session,
        request: devmgr::uart::PendingOpen,
    },
    Running(Execution),
    Finished,
}

pub fn run(
    services: &[&Service],
    config: Config,
    linux: &linux::Control<'_>,
    devices: &Handle<Channel>,
    console: u32,
) -> Result<usize, InitError> {
    let mut programs: Vec<_> = config
        .program
        .into_iter()
        .map(|program| ProgramState {
            program,
            phase: Phase::Ready(0),
            started: 0,
            failures: 0,
        })
        .collect();
    loop {
        let now = thread::now_ns()?;
        for state in &mut programs {
            state.poll(linux, devices, console, now)?;
        }
        let mut items: Vec<_> = services.iter().map(|service| service.wait_item()).collect();
        for state in &programs {
            if let Some(call) = state.phase.call() {
                items.push(WaitItem::new(call, ipc::READABLE));
            }
        }
        let next = programs
            .iter()
            .filter_map(|state| match state.phase {
                Phase::Ready(at) => Some(at),
                _ => None,
            })
            .min();
        match ipc::wait_many(&items, next.map_or(WaitMode::Forever, WaitMode::Until)) {
            Ok(ready) if ready.index < services.len() => {
                // TODO: Restart the base service and rebuild dependent sessions.
                return Ok(services[ready.index].wait()?.max(1));
            }
            Ok(_) | Err(Error::Kernel(KernelError::TimedOut)) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

impl Phase {
    fn call(&self) -> Option<&Handle<Call>> {
        match self {
            Self::Output(request) | Self::Input { request, .. } => Some(request.call()),
            Self::Running(execution) => Some(execution.call()),
            Self::Ready(_) | Self::Finished => None,
        }
    }
}

impl ProgramState {
    fn poll(
        &mut self,
        linux: &linux::Control<'_>,
        devices: &Handle<Channel>,
        console: u32,
        now: u64,
    ) -> Result<(), Error> {
        let phase = core::mem::replace(&mut self.phase, Phase::Finished);
        self.phase = match self.advance(phase, linux, devices, console, now) {
            Ok(phase) => phase,
            Err(error) => {
                logger::log::error!(target: "init", "Program {} failed: {error:?}", self.program.name);
                self.finished(thread::now_ns()?, false)
            }
        };
        Ok(())
    }

    fn advance(
        &mut self,
        phase: Phase,
        linux: &linux::Control<'_>,
        devices: &Handle<Channel>,
        console: u32,
        now: u64,
    ) -> Result<Phase, InitError> {
        Ok(match phase {
            Phase::Ready(at) if at <= now => {
                self.started = now;
                Phase::Output(devmgr::uart::Session::start_open(
                    process::task(),
                    devices,
                    console,
                    devmgr::uart::Access::Output,
                )?)
            }
            Phase::Output(mut request) => match request.try_complete() {
                Ok(output) => match self.program.terminal {
                    Some(Terminal::Console) => Phase::Input {
                        output,
                        request: devmgr::uart::Session::start_open(
                            process::task(),
                            devices,
                            console,
                            devmgr::uart::Access::InputOutput,
                        )?,
                    },
                    None => self.execute(linux, output, None)?,
                },
                Err(devmgr::Error::Runtime(Error::Kernel(KernelError::WouldBlock))) => {
                    Phase::Output(request)
                }
                Err(error) => return Err(error.into()),
            },
            Phase::Input {
                output,
                mut request,
            } => match request.try_complete() {
                Ok(input) => self.execute(linux, output, Some(input))?,
                Err(devmgr::Error::Runtime(Error::Kernel(KernelError::WouldBlock))) => {
                    Phase::Input { output, request }
                }
                Err(error) => return Err(error.into()),
            },
            Phase::Running(mut execution) => match execution.try_complete() {
                Err(linux::Error::Runtime(Error::Kernel(KernelError::WouldBlock))) => {
                    Phase::Running(execution)
                }
                Ok(status) => {
                    logger::log::info!(target: "init", "Program {} exited: {} signal {}", self.program.name, status.code, status.signal);
                    self.finished(thread::now_ns()?, status.code == 0 && status.signal == 0)
                }
                Err(error) => return Err(error.into()),
            },
            phase => phase,
        })
    }

    fn execute(
        &self,
        linux: &linux::Control<'_>,
        output: devmgr::uart::Session,
        input: Option<devmgr::uart::Session>,
    ) -> Result<Phase, InitError> {
        let program = &self.program;
        let mut arguments = alloc::vec![program.path.as_bytes()];
        arguments.extend(program.args.iter().map(|argument| argument.as_bytes()));
        let environment: Vec<String> = program
            .env
            .iter()
            .map(|(key, value)| alloc::format!("{key}={value}"))
            .collect();
        let environment: Vec<_> = environment.iter().map(|value| value.as_bytes()).collect();
        let request = linux::Program {
            path: program.path.as_bytes(),
            arguments: &arguments,
            environment: &environment,
        };
        let execution = match program.executor {
            Executor::Linux => match input {
                Some(input) => linux.start_run_with_input(request, output, input)?,
                None => linux.start_run(request, output)?,
            },
        };
        logger::log::info!(target: "init", "Starting program {}", program.name);
        Ok(Phase::Running(execution))
    }

    fn finished(&mut self, now: u64, success: bool) -> Phase {
        let restart = match self.program.restart {
            Restart::Never => false,
            Restart::OnFailure => !success,
            Restart::Always => true,
        };
        if !restart {
            return Phase::Finished;
        }
        if now.saturating_sub(self.started) >= 30_000_000_000 {
            self.failures = 0;
        }
        let delay = (100_000_000u64 << self.failures.min(9)).min(30_000_000_000);
        self.failures = self.failures.saturating_add(1);
        Phase::Ready(now.saturating_add(delay))
    }
}
