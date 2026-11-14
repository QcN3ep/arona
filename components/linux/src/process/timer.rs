// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Per-process interval timers and their signal generation.

use super::{
    Process,
    signal::{self, Info},
};
use crate::Error;
use alloc::vec::Vec;
use runtime::{Event, Handle, Rights, Task, Timer, ipc::WaitItem};

/// One expiry of a process interval timer.
pub(crate) struct Expiries {
    pub real: bool,
    pub virt: bool,
    pub prof: bool,
}

/// A one-shot native timer whose expiry is observed through an event.
struct Alarm {
    end: u64,
    event: Handle<Event>,
    _timer: Handle<Timer>,
}

impl Alarm {
    fn new(task: &Handle<Task>, duration: u64) -> Result<Self, runtime::Error> {
        let now = runtime::thread::now_ns()?;
        let end = now.saturating_add(duration).min(u64::MAX - 1);
        let event = task.event(Rights::WAIT | Rights::SIGNAL)?;
        let timer = task.timer(&event, 1, Rights::WRITE)?;
        timer.arm(end)?;
        Ok(Self {
            end,
            event,
            _timer: timer,
        })
    }

    fn expired(&self, now: u64) -> bool {
        now >= self.end
    }

    fn remaining(&self, now: u64) -> u64 {
        self.end.saturating_sub(now)
    }

    fn waits<'a>(&'a self, items: &mut Vec<WaitItem<'a>>) {
        items.push(WaitItem::new(&self.event, 1));
    }
}

#[derive(Default)]
struct RealTimer {
    interval: u64,
    alarm: Option<Alarm>,
}

#[derive(Default)]
struct CpuTimer {
    armed: bool,
    interval: u64,
    remaining: u64,
    alarm: Option<Alarm>,
}

/// The three Linux interval timers owned by one process.
#[derive(Default)]
pub(crate) struct Timers {
    real: RealTimer,
    virt: CpuTimer,
    prof: CpuTimer,
    cpu_seen: u64,
}

impl Timers {
    pub fn waits<'a>(&'a self, items: &mut Vec<WaitItem<'a>>) {
        for alarm in [
            self.real.alarm.as_ref(),
            self.virt.alarm.as_ref(),
            self.prof.alarm.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            alarm.waits(items);
        }
    }

    /// Record the process CPU time already accounted for.
    pub fn rebase_cpu(&mut self, cpu: u64) {
        self.cpu_seen = cpu;
    }

    /// Report the remaining time and interval of one timer.
    pub fn get(&self, which: usize, now: u64, cpu: u64) -> (u64, u64) {
        match which {
            0 => (
                self.real
                    .alarm
                    .as_ref()
                    .map_or(0, |alarm| alarm.remaining(now)),
                self.real.interval,
            ),
            1 => self.cpu_value(&self.virt, cpu),
            _ => self.cpu_value(&self.prof, cpu),
        }
    }

    fn cpu_value(&self, timer: &CpuTimer, cpu: u64) -> (u64, u64) {
        if !timer.armed {
            return (0, timer.interval);
        }
        let consumed = cpu.saturating_sub(self.cpu_seen);
        (timer.remaining.saturating_sub(consumed), timer.interval)
    }

    /// Install a timer, returning nothing because the caller captured the old value.
    pub fn set(
        &mut self,
        which: usize,
        value: u64,
        interval: u64,
        task: &Handle<Task>,
        cpu: u64,
    ) -> Result<(), runtime::Error> {
        let consumed = cpu.saturating_sub(self.cpu_seen);
        self.cpu_seen = cpu;
        self.consume_cpu(consumed);
        match which {
            0 => {
                self.real.interval = if value == 0 { 0 } else { interval };
                self.real.alarm = if value == 0 {
                    None
                } else {
                    Some(Alarm::new(task, value)?)
                };
            }
            1 | 2 => {
                let timer = if which == 1 {
                    &mut self.virt
                } else {
                    &mut self.prof
                };
                timer.armed = value != 0;
                timer.interval = if value == 0 { 0 } else { interval };
                timer.remaining = value;
                timer.alarm = if value == 0 {
                    None
                } else {
                    Some(Alarm::new(task, value)?)
                };
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn consume_cpu(&mut self, consumed: u64) {
        for timer in [&mut self.virt, &mut self.prof] {
            if timer.armed {
                timer.remaining = timer.remaining.saturating_sub(consumed);
            }
        }
    }

    /// Account CPU time and check expiry, returning which timers fired.
    pub fn poll(
        &mut self,
        now: u64,
        cpu: u64,
        runnable: bool,
        task: &Handle<Task>,
    ) -> Result<Expiries, Error> {
        let real = if self
            .real
            .alarm
            .as_ref()
            .is_some_and(|alarm| alarm.expired(now))
        {
            self.real.alarm = if self.real.interval == 0 {
                None
            } else {
                Some(Alarm::new(task, self.real.interval)?)
            };
            true
        } else {
            false
        };
        let consumed = cpu.saturating_sub(self.cpu_seen);
        self.cpu_seen = cpu;
        let virt = poll_cpu(&mut self.virt, consumed, now, runnable, task)?;
        let prof = poll_cpu(&mut self.prof, consumed, now, runnable, task)?;
        Ok(Expiries { real, virt, prof })
    }
}

fn poll_cpu(
    timer: &mut CpuTimer,
    consumed: u64,
    now: u64,
    runnable: bool,
    task: &Handle<Task>,
) -> Result<bool, Error> {
    if !timer.armed {
        timer.alarm = None;
        return Ok(false);
    }
    let fired = consumed >= timer.remaining;
    if fired {
        if timer.interval == 0 {
            timer.armed = false;
            timer.remaining = 0;
            timer.alarm = None;
            return Ok(true);
        }
        let excess = consumed - timer.remaining;
        timer.remaining = timer.interval - excess % timer.interval;
        timer.alarm = None;
    } else {
        timer.remaining -= consumed;
    }
    if !runnable {
        timer.alarm = None;
        return Ok(fired);
    }
    if timer.alarm.as_ref().is_none_or(|alarm| alarm.expired(now)) {
        timer.alarm = Some(Alarm::new(task, timer.remaining)?);
    }
    Ok(fired)
}

/// Account elapsed CPU time, expire CPU timers, and queue their signals.
pub(crate) fn poll(process: &mut Process<'_>) -> Result<bool, Error> {
    let now = runtime::thread::now_ns()?;
    let cpu = process.cpu_time_ns();
    let runnable = process.runnable();
    let expiries = process
        .timers
        .poll(now, cpu, runnable, process.supervisor)?;
    for (expired, number) in [
        (expiries.real, signal::ALRM),
        (expiries.virt, signal::VTALRM),
        (expiries.prof, signal::PROF),
    ] {
        if expired {
            let _ = process.namespace.signal_process(
                process.pid,
                Info {
                    number,
                    code: 128,
                    sender: 0,
                    uid: 0,
                    address: 0,
                    value: 0,
                },
            );
        }
    }
    Ok(expiries.real || expiries.virt || expiries.prof)
}
