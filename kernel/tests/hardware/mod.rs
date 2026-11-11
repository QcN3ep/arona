// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use kernel::hardware::{BindError, Slot};

#[test]
fn missing_and_duplicate_bindings_do_not_change_the_slot() {
    let slot = Slot::new();
    assert_eq!(slot.try_get(), None);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| slot.get())).is_err());
    assert_eq!(slot.bind(7), Ok(()));
    assert_eq!(slot.get(), 7);
    assert_eq!(slot.bind(19), Err(BindError::AlreadyBound));
    assert_eq!(slot.get(), 7);
}

#[test]
fn concurrent_bindings_publish_exactly_one_complete_value() {
    let slot = Slot::new();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|threads| {
        let results: Vec<_> = (0..8)
            .map(|id| {
                let slot = &slot;
                let barrier = &barrier;
                threads.spawn(move || {
                    barrier.wait();
                    let result = slot.bind((id, !id));
                    let (published, inverse) = slot.get();
                    assert_eq!(published, !inverse);
                    (id, result)
                })
            })
            .collect();
        let winners: Vec<_> = results
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter_map(|(id, result)| result.ok().map(|()| id))
            .collect();
        assert_eq!(winners, [slot.get().0]);
    });
}
