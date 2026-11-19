// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Periodic load averages derived from native runnable time measurements.

use crate::Error;
use core::cell::Cell;
use runtime::{Event, Handle, Rights, SystemInfo, Task, Timer, statistics::RunnableIntegral};

const INTERVAL: u64 = 5_000_000_000;

pub(super) struct Sampler {
    event: Handle<Event>,
    timer: Handle<Timer>,
    previous: RunnableIntegral,
    deadline: u64,
}

impl Sampler {
    pub fn new(task: &Handle<Task>, system: &Handle<SystemInfo>) -> Result<Self, Error> {
        let event = task.event(Rights::WAIT | Rights::SIGNAL)?;
        let timer = task.timer(&event, 1, Rights::WRITE)?;
        let previous = system.runnable_integral()?;
        let deadline = previous.sample_ns.saturating_add(INTERVAL);
        timer.arm(deadline)?;
        Ok(Self {
            event,
            timer,
            previous,
            deadline,
        })
    }

    pub fn event(&self) -> &Handle<Event> {
        &self.event
    }

    pub fn poll(
        &mut self,
        system: &Handle<SystemInfo>,
        loads: &Cell<[u32; 3]>,
    ) -> Result<(), Error> {
        if runtime::thread::now_ns()? < self.deadline {
            return Ok(());
        }
        let sample = system.runnable_integral()?;
        let elapsed = u128::from(sample.sample_ns.saturating_sub(self.previous.sample_ns));
        if elapsed != 0 {
            let integral = sample.integral.saturating_sub(self.previous.integral);
            let target = integral.saturating_mul(1 << 16) / elapsed;
            let mut averages = loads.get();
            for (value, period) in
                averages
                    .iter_mut()
                    .zip([60_000_000_000u128, 300_000_000_000, 900_000_000_000])
            {
                let current = u128::from(*value);
                let change = current.abs_diff(target).saturating_mul(elapsed) / (period + elapsed);
                let next = if target >= current {
                    current.saturating_add(change)
                } else {
                    current.saturating_sub(change)
                };
                *value = next.min(u128::from(u32::MAX)) as u32;
            }
            loads.set(averages);
        }
        self.previous = sample;
        self.deadline = sample.sample_ns.saturating_add(INTERVAL);
        // The event may already have been consumed by the service wait.
        match runtime::ipc::wait_many(
            &[runtime::ipc::WaitItem::new(&self.event, 1)],
            runtime::ipc::WaitMode::Poll,
        ) {
            Ok(_) | Err(runtime::Error::Kernel(runtime::KernelError::WouldBlock)) => {}
            Err(error) => return Err(error.into()),
        }
        self.timer.arm(self.deadline)?;
        Ok(())
    }
}
