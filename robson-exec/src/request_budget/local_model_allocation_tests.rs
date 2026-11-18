//! Unit-binary allocation accounting, enabled only on the measured test thread.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use super::*;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}
struct MeasuredAllocator;
#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the unchanged valid layout to the system allocator.
        let result = unsafe { System.alloc(layout) };
        let _ = ACTIVE.try_with(|active| {
            if active.get() && !result.is_null() {
                ALLOCS.with(|v| v.set(v.get() + 1));
                BYTES.with(|v| v.set(v.get() + layout.size()));
            }
        });
        result
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = ACTIVE.try_with(|active| {
            if active.get() {
                FREES.with(|v| v.set(v.get() + 1));
            }
        });
        // SAFETY: caller supplies the original allocation and matching layout.
        unsafe { System.dealloc(ptr, layout) };
    }
}
fn measured<T>(f: impl FnOnce() -> T) -> (T, (usize, usize, usize)) {
    ALLOCS.with(|v| v.set(0));
    BYTES.with(|v| v.set(0));
    FREES.with(|v| v.set(0));
    ACTIVE.with(|v| v.set(true));
    let result = f();
    ACTIVE.with(|v| v.set(false));
    (result, (ALLOCS.with(Cell::get), BYTES.with(Cell::get), FREES.with(Cell::get)))
}
#[test]
fn construction_hot_paths_and_retained_weak_identity_have_bounded_allocations() {
    let (mut m, created) = measured(model);
    assert_eq!(created.0, 2, "one marker plus one fixed record allocation");
    let records = m.records.capacity() * std::mem::size_of::<Record>();
    let marker = created.1 - records;
    assert!(marker > 0 && marker <= 64);
    let (weak, hot) = measured(|| {
        let h = m.insert_pending_candidate(tick(100), input(1, 0)).unwrap();
        m.record_handoff_candidate(h, tick(100)).unwrap();
        m.usage_at(tick(160)).unwrap();
        let h = m.insert_pending_candidate(tick(160), input(1, 0)).unwrap();
        m.cancel_pending_candidate(h).unwrap();
        Arc::downgrade(&m.marker)
    });
    assert_eq!(hot, (0, 0, 0), "insert, handoff, prune and cancel allocate nothing");
    let (_, dropped) = measured(|| drop(m));
    assert_eq!(dropped, (0, 0, 1), "weak identity retains only marker allocation");
    assert!(weak.upgrade().is_none());
    let (_, released) = measured(|| drop(weak));
    assert_eq!(released, (0, 0, 1), "last weak reference releases marker");
    eprintln!("local model allocated bytes: records={records} marker={marker}, hot=0");
}
