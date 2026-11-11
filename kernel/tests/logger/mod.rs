// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use core::fmt::{self, Write};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use arrayvec::ArrayString;
use kernel::log::{self, LevelFilter, Log, Record};
use kernel::logger::{self, Logger, MAX_SINKS, Sink, SinkError};

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

// SAFETY: Allocation and deallocation must follow the Layout contract, and allocator methods must not unwind the stack.
unsafe impl GlobalAlloc for CountingAllocator {
    /// # Safety
    ///
    /// layout.size() must be greater than zero.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| {
            if let Some(value) = count.get() {
                count.set(Some(value + 1));
            }
        });
        // SAFETY: layout.size() must be greater than zero.
        unsafe { System.alloc(layout) }
    }

    /// # Safety
    ///
    /// pointer and layout must refer to an allocation of this allocator that has not been deallocated.
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer and layout must refer to an allocation of System that has not been deallocated.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct RecordingSink {
    output: Mutex<ArrayString<256>>,
    flushes: AtomicUsize,
    fail_flush: AtomicBool,
}

impl RecordingSink {
    const fn new() -> Self {
        Self {
            output: Mutex::new(ArrayString::new_const()),
            flushes: AtomicUsize::new(0),
            fail_flush: AtomicBool::new(false),
        }
    }
}

impl Sink for RecordingSink {
    fn write(&self, record: &Record<'_>) -> fmt::Result {
        let mut output = self.output.try_lock().map_err(|_| fmt::Error)?;
        writeln!(output, "{} {}", record.level(), record.args())
    }

    fn flush(&self) -> fmt::Result {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        if self.fail_flush.load(Ordering::Relaxed) {
            Err(fmt::Error)
        } else {
            Ok(())
        }
    }
}

struct UnavailableSink;

impl Sink for UnavailableSink {
    fn write(&self, _: &Record<'_>) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[test]
fn facade_manages_independent_sinks_without_allocating() {
    static EARLY: RecordingSink = RecordingSink::new();
    static MIRROR: RecordingSink = RecordingSink::new();
    static LATE: RecordingSink = RecordingSink::new();

    ALLOCATIONS.set(Some(0));
    let early = logger::LOGGER.add_sink(&EARLY).unwrap();
    let unavailable = logger::LOGGER.add_sink(&UnavailableSink).unwrap();
    let mirror = logger::LOGGER.add_sink(&MIRROR).unwrap();
    logger::init(LevelFilter::Info).unwrap();
    log::debug!("filtered");
    log::info!("boot {}", 7);
    log::logger().flush();
    early.replace(&LATE).unwrap();
    logger::LOGGER.set_max_level(LevelFilter::Warn);
    log::info!("filtered after replacement");
    log::warn!("ready");
    unavailable.remove().unwrap();
    early.remove().unwrap();
    assert!(logger::init(LevelFilter::Trace).is_err());
    log::info!("still filtered");
    log::error!("only mirror");
    mirror.remove().unwrap();
    let allocations = ALLOCATIONS.replace(None).unwrap();

    assert_eq!(allocations, 0);
    assert_eq!(EARLY.output.lock().unwrap().as_str(), "INFO boot 7\n");
    assert_eq!(
        MIRROR.output.lock().unwrap().as_str(),
        "INFO boot 7\nWARN ready\nERROR only mirror\n"
    );
    assert_eq!(LATE.output.lock().unwrap().as_str(), "WARN ready\n");
    assert!(EARLY.flushes.load(Ordering::Relaxed) > 0);
    assert!(LATE.flushes.load(Ordering::Relaxed) > 0);
    assert!(MIRROR.flushes.load(Ordering::Relaxed) > 0);
    assert_eq!(logger::LOGGER.failures(), 2);
}

#[test]
fn failed_flush_preserves_registration_and_stale_handles_cannot_remove_reused_slots() {
    static OLD: RecordingSink = RecordingSink::new();
    static LATE: RecordingSink = RecordingSink::new();
    let logger = Logger::new(LevelFilter::Info);
    let handle = logger.add_sink(&OLD).unwrap();

    OLD.fail_flush.store(true, Ordering::Relaxed);
    assert!(matches!(handle.replace(&LATE), Err(SinkError::Flush)));
    assert!(matches!(handle.remove(), Err(SinkError::Flush)));
    emit(&logger, "preserved");
    assert_eq!(OLD.output.lock().unwrap().as_str(), "INFO preserved\n");
    assert!(LATE.output.lock().unwrap().is_empty());

    OLD.fail_flush.store(false, Ordering::Relaxed);
    handle.replace(&LATE).unwrap();
    emit(&logger, "replacement");
    handle.remove().unwrap();
    assert!(matches!(handle.replace(&OLD), Err(SinkError::Detached)));
    let replacement = logger.add_sink(&LATE).unwrap();
    assert!(matches!(handle.remove(), Err(SinkError::Detached)));
    assert!(matches!(handle.replace(&OLD), Err(SinkError::Detached)));
    emit(&logger, "reused");
    assert_eq!(
        LATE.output.lock().unwrap().as_str(),
        "INFO replacement\nINFO reused\n"
    );

    for _ in 1..MAX_SINKS {
        logger.add_sink(&UnavailableSink).unwrap();
    }
    assert!(matches!(logger.add_sink(&OLD), Err(SinkError::Full)));
    replacement.remove().unwrap();
    let broken = logger.add_sink(&OLD).unwrap();
    OLD.fail_flush.store(true, Ordering::Relaxed);
    assert!(matches!(broken.remove(), Err(SinkError::Flush)));
    let flushes = OLD.flushes.load(Ordering::Relaxed);
    broken.discard().unwrap();
    assert_eq!(OLD.flushes.load(Ordering::Relaxed), flushes);
    logger.add_sink(&LATE).unwrap();
}

fn emit(logger: &Logger, message: &str) {
    logger.log(
        &Record::builder()
            .level(log::Level::Info)
            .args(format_args!("{message}"))
            .build(),
    );
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "sink synchronization timed out");
        std::thread::yield_now();
    }
}

#[test]
fn handoff_requires_quiescence_and_draining_without_blocking_other_sinks() {
    struct PausingSink {
        phase: AtomicUsize,
        release_write: AtomicBool,
        release_flush: AtomicBool,
    }

    impl Sink for PausingSink {
        fn write(&self, _: &Record<'_>) -> fmt::Result {
            self.phase.store(1, Ordering::Release);
            wait_until(|| self.release_write.load(Ordering::Acquire));
            self.phase.store(2, Ordering::Release);
            Ok(())
        }

        fn flush(&self) -> fmt::Result {
            self.phase.store(3, Ordering::Release);
            wait_until(|| self.release_flush.load(Ordering::Acquire));
            self.phase.store(4, Ordering::Release);
            Ok(())
        }
    }

    static OLD: PausingSink = PausingSink {
        phase: AtomicUsize::new(0),
        release_write: AtomicBool::new(false),
        release_flush: AtomicBool::new(false),
    };
    static MIRROR: RecordingSink = RecordingSink::new();
    static LATE: RecordingSink = RecordingSink::new();
    let logger = Logger::new(LevelFilter::Info);
    let handle = logger.add_sink(&OLD).unwrap();
    logger.add_sink(&MIRROR).unwrap();

    std::thread::scope(|threads| {
        let writer = threads.spawn(|| emit(&logger, "in flight"));
        wait_until(|| OLD.phase.load(Ordering::Acquire) == 1);
        assert!(matches!(handle.replace(&LATE), Err(SinkError::Busy)));
        assert!(matches!(handle.remove(), Err(SinkError::Busy)));
        assert!(matches!(handle.discard(), Err(SinkError::Busy)));
        OLD.release_write.store(true, Ordering::Release);
        writer.join().unwrap();

        let replacing = threads.spawn(|| handle.replace(&LATE));
        wait_until(|| OLD.phase.load(Ordering::Acquire) == 3);
        assert!(matches!(handle.remove(), Err(SinkError::Busy)));
        emit(&logger, "during drain");
        logger.flush();
        assert!(LATE.output.lock().unwrap().is_empty());
        assert_eq!(logger.failures(), 2);
        OLD.release_flush.store(true, Ordering::Release);
        replacing.join().unwrap().unwrap();
    });

    assert_eq!(OLD.phase.load(Ordering::Acquire), 4);
    emit(&logger, "after drain");
    assert_eq!(LATE.output.lock().unwrap().as_str(), "INFO after drain\n");
    assert_eq!(
        MIRROR.output.lock().unwrap().as_str(),
        "INFO in flight\nINFO during drain\nINFO after drain\n"
    );
}

#[test]
fn buffered_records_survive_failed_reads_and_overwrite_as_whole_records() {
    use abi::logger::{MAX_RECORD_SIZE, Record as BufferedRecord};
    use kernel::logger::ReadError;

    static LOGGER: Logger = Logger::new(LevelFilter::Info);
    let reader = LOGGER.kernel_log();
    let other = reader.clone();
    let mut bytes = [0; MAX_RECORD_SIZE];
    assert_eq!(reader.read(0, &mut bytes), Ok(0));
    ALLOCATIONS.set(Some(0));
    emit(&LOGGER, "early record");
    assert_eq!(reader.read(0, &mut []), Err(ReadError::BufferTooSmall));
    let length = reader.read(0, &mut bytes).unwrap();
    let record = BufferedRecord::decode(&bytes[..length]).unwrap();
    assert_eq!(record.sequence, 1);
    assert_eq!(record.timestamp_ns, None);
    assert_eq!(record.message, "early record");
    let mut snapshot = [0; MAX_RECORD_SIZE];
    assert_eq!(other.read(0, &mut snapshot), Ok(length));
    assert_eq!(&snapshot[..length], &bytes[..length]);
    assert_eq!(reader.read(2, &mut bytes), Ok(0));

    let mut message = ArrayString::<4096>::new();
    for _ in 0..1000 {
        message.push_str("日志");
        if message.len() > 4000 {
            break;
        }
    }
    for _ in 0..128 {
        emit(&LOGGER, &message);
    }
    let mut cursor = 2;
    let mut count = 0;
    loop {
        let length = reader.read(cursor, &mut bytes).unwrap();
        if length == 0 {
            break;
        }
        let record = BufferedRecord::decode(&bytes[..length]).unwrap();
        if count == 0 {
            assert!(record.sequence > cursor);
        }
        assert!(record.sequence >= cursor);
        assert!(record.truncated);
        assert!(message.starts_with(record.message));
        assert!(!record.message.is_empty());
        assert_eq!(other.read(cursor, &mut snapshot), Ok(length));
        assert_eq!(&snapshot[..length], &bytes[..length]);
        cursor = record.sequence + 1;
        count += 1;
    }
    assert_eq!(cursor, 130);
    assert!(count > 1 && count < 128);
    assert_eq!(ALLOCATIONS.replace(None), Some(0));
}

#[test]
fn concurrent_and_reentrant_logging_publishes_ordered_records() {
    use abi::logger::{MAX_RECORD_SIZE, Record as BufferedRecord};

    static LOGGER: Logger = Logger::new(LevelFilter::Info);
    struct Nested;
    impl fmt::Display for Nested {
        fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
            emit(&LOGGER, "nested");
            output.write_str("outer")
        }
    }
    LOGGER.log(
        &Record::builder()
            .level(log::Level::Info)
            .args(format_args!("{Nested}"))
            .build(),
    );
    let reader = LOGGER.kernel_log();
    let mut bytes = [0; MAX_RECORD_SIZE];
    for (cursor, message) in [(1, "nested"), (2, "outer")] {
        let length = reader.read(cursor, &mut bytes).unwrap();
        let record = BufferedRecord::decode(&bytes[..length]).unwrap();
        assert_eq!(record.sequence, cursor);
        assert_eq!(record.message, message);
    }
    std::thread::scope(|threads| {
        for _ in 0..4 {
            threads.spawn(|| {
                for _ in 0..2000 {
                    emit(&LOGGER, "concurrent");
                }
            });
        }
    });
    let mut cursor = 3;
    let mut count = 0;
    loop {
        let length = reader.read(cursor, &mut bytes).unwrap();
        if length == 0 {
            break;
        }
        let record = BufferedRecord::decode(&bytes[..length]).unwrap();
        assert!(record.sequence >= cursor);
        assert_eq!(record.message, "concurrent");
        cursor = record.sequence + 1;
        count += 1;
    }
    assert!(count > 0);
}
