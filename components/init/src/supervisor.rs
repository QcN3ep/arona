// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Supervision of startup, exit, and restart for configured programs and ordered base service shutdown.

// TODO: Support configuration reload and service dependency management.

use crate::{
    InitError,
    config::{Config, Executor, PowerAction, Program, Restart, Terminal},
    service::{self, Service},
};
use alloc::{string::String, vec::Vec};
use core::fmt::{self, Write as _};
use runtime::{
    Call, Channel, Error, Handle, KernelError,
    ipc::{self, PendingCall, WaitItem, WaitMode},
    process, thread,
};

type Execution = PendingCall<linux::ExitStatus, (), linux::Error>;

/// Base service tasks and the control endpoints owned by init.
pub struct System<'a> {
    pub filesystem: &'a Service,
    pub devmgr: &'a Service,
    pub logger: &'a Service,
    pub linux: &'a Service,
    pub net: &'a Service,
    pub net_control: net::Control<'a>,
    pub linux_control: linux::Control<'a>,
    pub logger_control: logger::Control<'a>,
    pub mounts: fs::Control<'a>,
    pub devices: Handle<Channel>,
    pub console: u32,
    pub mounted: bool,
}

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

pub fn run(system: System<'_>, config: Config) -> Result<usize, InitError> {
    let services = [
        system.filesystem,
        system.devmgr,
        system.logger,
        system.linux,
        system.net,
    ];
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
            state.poll(&system.linux_control, &system.devices, system.console, now)?;
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

/// Stop base services in reverse dependency order and apply a configured power action.
///
/// The Linux compatibility service stops first; closing its control channel makes it
/// return and drop its supervised processes, which terminates the guest Tasks because
/// the service has no guest stop protocol. The log service flushes and then stops so
/// that its persistent log file is released before the file service unmounts. The file
/// service flushes and unmounts every mounted filesystem through its control protocol
/// before it stops, and the device management service remains until it performs the
/// shutdown or reboot through the platform power controller.
pub fn shutdown(system: System<'_>, action: PowerAction) -> Result<usize, InitError> {
    let System {
        filesystem,
        devmgr,
        logger,
        linux,
        linux_control,
        net,
        net_control,
        logger_control,
        mounts,
        devices,
        console,
        mounted,
    } = system;
    let mut report = Report::open(&devices, console)?;
    logger::log::logger().flush();
    report.info(format_args!("Stopping Linux programs"));
    drop(linux_control);
    stop_service(linux, &mut report, "Linux service")?;
    report.info(format_args!("Stopping network service"));
    drop(net_control);
    stop_service(net, &mut report, "network service")?;
    report.info(format_args!("Stopping logger service"));
    logger::log::logger().flush();
    drop(logger_control);
    stop_service(logger, &mut report, "logger service")?;
    report.info(format_args!("Flushing file service"));
    if mounted {
        report.info(format_args!("Unmounting /var"));
        mounts.unmount(b"/var", service::deadline()?)?;
        report.info(format_args!("Unmounted /var"));
    }
    drop(mounts);
    stop_service(filesystem, &mut report, "file service")?;
    match action {
        PowerAction::Shutdown => report.info(format_args!("Shutting down")),
        PowerAction::Reboot => report.info(format_args!("Rebooting")),
    }
    report.flush();
    if let Err(error) = service::apply_power_action(&devices, action) {
        report.info(format_args!("Power action failed: {error:?}"));
        drop(devices);
        let _ = devmgr.stop(service::deadline()?);
        return Err(error.into());
    }
    Ok(0)
}

fn stop_service(base: &Service, report: &mut Report, name: &str) -> Result<(), InitError> {
    if base.stop(service::deadline()?)? {
        report.info(format_args!("Stopped {name}"));
    } else {
        report.info(format_args!("Terminated {name} after a stop timeout"));
    }
    Ok(())
}

/// Ordered shutdown messages written directly to the console.
///
/// The log service stops before the file service, so later shutdown messages cannot
/// use the log facade; this output session keeps them visible on the console and does
/// not depend on any remaining service except the device management service.
struct Report {
    output: devmgr::uart::Session,
    failed: bool,
}

impl Report {
    fn open(devices: &Handle<Channel>, console: u32) -> Result<Self, InitError> {
        Ok(Self {
            output: devmgr::uart::Session::open(
                process::task(),
                devices,
                console,
                devmgr::uart::Access::Output,
            )?,
            failed: false,
        })
    }

    fn info(&mut self, message: fmt::Arguments<'_>) {
        if self.failed {
            return;
        }
        let now = thread::now_ns().unwrap_or(0);
        let mut line = String::new();
        let _ = writeln!(
            line,
            "[{:>5}.{:06}] INFO init: {message}",
            now / 1_000_000_000,
            now % 1_000_000_000 / 1000
        );
        self.failed = self.output.write_all(line.as_bytes()).is_err();
    }

    fn flush(&mut self) {
        if !self.failed {
            self.failed = self.output.flush().is_err();
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
