// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! CFS execution accounting, placement, and preemption policy.

use super::{Scheduler, Thread, ThreadKind};

const LATENCY_NS: u64 = 6_000_000;
const MIN_GRANULARITY_NS: u64 = 750_000;
const WAKEUP_GRANULARITY_NS: u64 = 1_000_000;
const NORMAL_WEIGHT: u64 = 1024;

pub(in crate::thread) struct Entity {
    pub(in crate::thread) vruntime: i128,
    pub(in crate::thread) slice_start: u64,
    weight: u32,
    remainder: u32,
}

impl Entity {
    pub(in crate::thread) fn new() -> Self {
        Self {
            vruntime: 0,
            slice_start: 0,
            weight: NORMAL_WEIGHT as u32,
            remainder: 0,
        }
    }

    pub(in crate::thread) fn weight(&self) -> u32 {
        self.weight
    }

    pub(super) fn set_weight(&mut self, weight: u32) {
        self.weight = weight;
        self.remainder = 0;
    }

    fn account(&mut self, elapsed: u64) {
        let numerator =
            u128::from(elapsed) * u128::from(NORMAL_WEIGHT) + u128::from(self.remainder);
        let weight = u128::from(self.weight());
        self.vruntime = self.vruntime.saturating_add((numerator / weight) as i128);
        self.remainder = (numerator % weight) as u32;
    }
}

#[derive(Clone, Copy)]
pub(in crate::thread) enum Placement {
    New,
    Wake,
    Requeue,
}

impl Scheduler {
    pub(super) fn account_current(&mut self, now: u64) {
        if let Some(current) = &self.current {
            let mut data = current.0.data.lock();
            if let Some(previous) = data.dispatched_ns {
                // An IPC timer callback can account a newer sample before its enclosing interrupt.
                data.dispatched_ns = Some(previous.max(now));
                let elapsed = now.saturating_sub(previous);
                data.cpu_ns = data.cpu_ns.saturating_add(elapsed);
                data.fair.account(elapsed);
            }
        }
        self.update_min_vruntime();
    }

    pub(super) fn update_min_vruntime(&mut self) {
        let running = self
            .current
            .as_ref()
            .map(|thread| thread.0.data.lock().fair.vruntime);
        let queued = self
            .ready
            .first()
            .map(|thread| thread.0.data.lock().fair.vruntime);
        let minimum = match (running, queued) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(minimum) = minimum {
            self.min_vruntime = self.min_vruntime.max(minimum);
        }
    }

    fn period(&self, count: usize) -> u64 {
        LATENCY_NS.max(count as u64 * MIN_GRANULARITY_NS)
    }

    fn slice(&self, weight: u32, total_weight: u64, count: usize) -> u64 {
        self.period(count) * u64::from(weight) / total_weight
    }

    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/kernel/sched/fair.c?h=v6.5
    pub(in crate::thread) fn enqueue(&mut self, thread: Thread, placement: Placement) {
        let current_weight = self
            .current
            .as_ref()
            .map_or(0, |current| u64::from(current.0.data.lock().fair.weight()));
        let mut data = thread.0.data.lock();
        let count = self.ready.count + 1 + usize::from(self.current.is_some());
        match placement {
            Placement::New => {
                // A batch started on an idle CPU shares one starting point.
                let debit = if self.current.is_some() {
                    self.period(count) * NORMAL_WEIGHT
                        / (self.ready.weight + current_weight + u64::from(data.fair.weight()))
                } else {
                    0
                };
                data.fair.vruntime = self.min_vruntime.saturating_add(i128::from(debit));
            }
            Placement::Wake => {
                let floor = self.min_vruntime.saturating_sub(i128::from(LATENCY_NS / 2));
                data.fair.vruntime = data.fair.vruntime.max(floor);
            }
            Placement::Requeue => {}
        }
        let vruntime = data.fair.vruntime;
        let granularity = WAKEUP_GRANULARITY_NS * NORMAL_WEIGHT / u64::from(data.fair.weight());
        drop(data);
        if matches!(placement, Placement::Wake | Placement::New)
            && let Some(current) = &self.current
            && current.0.data.lock().fair.vruntime - vruntime > i128::from(granularity)
        {
            self.preemption_requested = true;
        }
        self.ready.push(thread);
    }

    pub(super) fn refresh_slice(&mut self, now: u64) {
        self.quantum_deadline = None;
        let Some(current) = &self.current else { return };
        if self.ready.count == 0 || !matches!(current.0.kind, ThreadKind::User(_)) {
            return;
        }
        let data = current.0.data.lock();
        let elapsed = data.cpu_ns.saturating_sub(data.fair.slice_start);
        let slice = self.slice(
            data.fair.weight(),
            self.ready.weight + u64::from(data.fair.weight()),
            self.ready.count + 1,
        );
        let remaining = slice.saturating_sub(elapsed);
        let ahead = self.ready.first().is_some_and(|first| {
            data.fair.vruntime - first.0.data.lock().fair.vruntime
                > i128::from(slice * NORMAL_WEIGHT / u64::from(data.fair.weight()))
        });
        if remaining == 0 || (elapsed >= MIN_GRANULARITY_NS && ahead) {
            self.preemption_requested = true;
        } else {
            self.quantum_deadline = Some(now.saturating_add(remaining));
        }
    }
}
