// SPDX-License-Identifier: MPL-2.0

use core::{
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};
use spin::Mutex;

use super::{EventError, EventReference};
use crate::{
    hardware::{self, cpu::InterruptGuard, timer::TimerError as HardwareTimerError},
    memory::{self, HeapArc, MemoryError},
    thread::{ThreadError, scheduler},
};

static TIMERS: Mutex<Option<TimerReference>> = Mutex::new(None);

/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
pub struct Timer {
    reference: TimerReference,
}

/// 保持对象存储存活，不计入开放引用。
///
/// # Safety
///
/// 操作及析构期间须保留内存管理器所需的内核映射。
#[derive(Clone)]
pub struct TimerReference {
    timer: HeapArc<Mutex<TimerState>>,
}

struct TimerState {
    credits: usize,
    event: EventReference,
    mask: u64,
    phase: TimerPhase,
    next: Option<TimerReference>,
}

static TIMER_STATES: AtomicUsize = AtomicUsize::new(0);

pub fn active_timers() -> usize {
    TIMER_STATES.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimerPhase {
    Disarmed,
    Armed(u64),
    Fired,
    Closed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    InvalidArgument,
    Closed,
    Event(EventError),
    Memory(MemoryError),
    Thread(ThreadError),
    Hardware(HardwareTimerError),
}

impl Timer {
    pub fn new(event: EventReference, mask: u64) -> Result<Self, TimerError> {
        let _interrupts = InterruptGuard::new();
        if mask == 0 {
            return Err(TimerError::InvalidArgument);
        }
        if event.is_closed() {
            return Err(TimerError::Closed);
        }
        let timer = memory::manager().allocate_arc(Mutex::new(TimerState {
            credits: 1,
            event,
            mask,
            phase: TimerPhase::Disarmed,
            next: None,
        }))?;
        TIMER_STATES.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            reference: TimerReference { timer },
        })
    }

    pub fn reference(&self) -> TimerReference {
        self.reference.clone()
    }
}

impl Drop for TimerState {
    fn drop(&mut self) {
        TIMER_STATES.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Clone for Timer {
    fn clone(&self) -> Self {
        let _interrupts = InterruptGuard::new();
        self.reference.timer.lock().credits += 1;
        Self {
            reference: self.reference(),
        }
    }
}

impl Deref for Timer {
    type Target = TimerReference;

    fn deref(&self) -> &Self::Target {
        &self.reference
    }
}

impl TimerReference {
    pub fn arm(&self, deadline_ns: u64) -> Result<(), TimerError> {
        if deadline_ns == u64::MAX {
            return Err(TimerError::InvalidArgument);
        }
        let _interrupts = InterruptGuard::new();
        let now = hardware::timer().now_ns()?;
        let phase = {
            let state = self.timer.lock();
            if state.phase == TimerPhase::Closed || state.event.is_closed() {
                return Err(TimerError::Closed);
            }
            if deadline_ns <= now {
                TimerPhase::Fired
            } else {
                TimerPhase::Armed(deadline_ns)
            }
        };
        self.replace(phase)?;
        if phase == TimerPhase::Fired {
            self.signal();
        }
        Ok(())
    }

    pub fn cancel(&self) -> Result<(), TimerError> {
        let _interrupts = InterruptGuard::new();
        if self.timer.lock().phase == TimerPhase::Closed {
            return Err(TimerError::Closed);
        }
        self.replace(TimerPhase::Disarmed)
    }

    fn replace(&self, phase: TimerPhase) -> Result<(), TimerError> {
        let previous = self.timer.lock().phase;
        remove(self);
        self.timer.lock().phase = phase;
        if matches!(phase, TimerPhase::Armed(_)) {
            insert(self);
        }
        if let Err(error) = scheduler::refresh_timer() {
            remove(self);
            self.timer.lock().phase = previous;
            if matches!(previous, TimerPhase::Armed(_)) {
                insert(self);
            }
            return Err(error.into());
        }
        Ok(())
    }

    fn signal(&self) {
        let (event, mask) = {
            let state = self.timer.lock();
            (state.event.clone(), state.mask)
        };
        match event.signal(mask) {
            Ok(()) | Err(EventError::Closed) => {}
            Err(error) => unreachable!("invalid timer event signal: {error:?}"),
        }
    }

    fn same(&self, other: &Self) -> bool {
        HeapArc::ptr_eq(&self.timer, &other.timer)
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let closed = {
            let mut state = self.reference.timer.lock();
            state.credits -= 1;
            if state.credits == 0 {
                state.phase = TimerPhase::Closed;
                true
            } else {
                false
            }
        };
        if closed {
            remove(&self.reference);
        }
    }
}

fn insert(timer: &TimerReference) {
    let mut head = TIMERS.lock();
    let target = deadline(timer);
    let mut previous: Option<TimerReference> = None;
    let mut cursor = head.clone();
    while let Some(current) = &cursor {
        if deadline(current) > target {
            break;
        }
        let next = current.timer.lock().next.clone();
        previous = cursor;
        cursor = next;
    }
    timer.timer.lock().next = cursor;
    if let Some(previous) = previous {
        previous.timer.lock().next = Some(timer.clone());
    } else {
        *head = Some(timer.clone());
    }
}

fn remove(timer: &TimerReference) {
    let mut head = TIMERS.lock();
    let mut previous: Option<TimerReference> = None;
    let mut cursor = head.clone();
    while let Some(current) = cursor {
        let next = current.timer.lock().next.clone();
        if current.same(timer) {
            current.timer.lock().next = None;
            if let Some(previous) = previous {
                previous.timer.lock().next = next;
            } else {
                *head = next;
            }
            return;
        }
        previous = Some(current);
        cursor = next;
    }
}

fn deadline(timer: &TimerReference) -> u64 {
    match timer.timer.lock().phase {
        TimerPhase::Armed(deadline) => deadline,
        _ => unreachable!("unarmed timer in deadline queue"),
    }
}

pub(crate) fn next_deadline() -> Option<u64> {
    TIMERS.lock().as_ref().map(deadline)
}

pub(crate) fn interrupt(now: u64) {
    loop {
        let timer = {
            let mut head = TIMERS.lock();
            let Some(timer) = head.as_ref() else { return };
            if deadline(timer) > now {
                return;
            }
            let timer = head.take().unwrap();
            let mut state = timer.timer.lock();
            *head = state.next.take();
            state.phase = TimerPhase::Fired;
            drop(state);
            timer
        };
        timer.signal();
    }
}

pub(crate) fn cancel_event(event: &EventReference) {
    let mut cursor = TIMERS.lock().clone();
    while let Some(timer) = cursor {
        let state = timer.timer.lock();
        cursor = state.next.clone();
        let matches = state.event.same(event);
        drop(state);
        if matches {
            remove(&timer);
            timer.timer.lock().phase = TimerPhase::Disarmed;
        }
    }
}

impl From<EventError> for TimerError {
    fn from(error: EventError) -> Self {
        Self::Event(error)
    }
}

impl From<MemoryError> for TimerError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}

impl From<ThreadError> for TimerError {
    fn from(error: ThreadError) -> Self {
        Self::Thread(error)
    }
}

impl From<HardwareTimerError> for TimerError {
    fn from(error: HardwareTimerError) -> Self {
        Self::Hardware(error)
    }
}
