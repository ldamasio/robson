//! Measurement executable only; never part of the daemon or an age authority.
#[cfg(all(
    target_os = "linux",
    target_env = "gnu",
    target_pointer_width = "64",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod supported {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::Instant,
    };

    use robson_exec::venue_clock::NativeClock;

    struct Counting;
    static TRACK: AtomicBool = AtomicBool::new(false);
    static ALLOCS: AtomicUsize = AtomicUsize::new(0);
    #[global_allocator]
    static ALLOCATOR: Counting = Counting;
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if TRACK.load(Ordering::Relaxed) {
                ALLOCS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: transparently forwards the allocator contract.
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: ptr/layout are forwarded unchanged to their allocator.
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            if TRACK.load(Ordering::Relaxed) {
                ALLOCS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: transparently forwards the allocator contract.
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            if TRACK.load(Ordering::Relaxed) {
                ALLOCS.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: transparently forwards the allocator contract.
            unsafe { System.realloc(ptr, layout, size) }
        }
    }
    fn marker(bytes: &[u8]) {
        // SAFETY: borrowed bytes remain readable for this synchronous write.
        let n = unsafe { libc::write(2, bytes.as_ptr().cast(), bytes.len()) };
        assert_eq!(n, bytes.len() as isize, "trace marker must be complete");
    }
    fn fds() -> usize {
        std::fs::read_dir("/proc/self/fd").unwrap().count()
    }
    pub fn run() {
        let mode = std::env::args().nth(1).expect("explicit measurement mode");
        if mode == "default-feature" {
            assert!(
                !std::hint::black_box(cfg!(feature = "crash-points")),
                "default build enabled crash points"
            );
            return;
        }
        assert!(mode == "cold" || mode == "latency");
        let fd_before = fds();
        // No clock capture or initialization precedes this marker in a fresh exec.
        marker(b"NATIVE_BEGIN\n");
        TRACK.store(true, Ordering::Relaxed);
        let initialized = NativeClock::initialize();
        TRACK.store(false, Ordering::Relaxed);
        marker(b"NATIVE_INITIALIZED\n");
        let clock = initialized.expect("native cold source must execute");
        let mut previous = clock.initial_capture();
        for _ in 0..2 {
            TRACK.store(true, Ordering::Relaxed);
            let captured = clock.capture();
            TRACK.store(false, Ordering::Relaxed);
            marker(b"NATIVE_CAPTURED\n");
            let next = captured.unwrap();
            assert!(next.elapsed_since(&previous).is_ok());
            previous = next;
        }
        marker(b"NATIVE_END\n");
        assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "userspace helper allocated");
        assert_eq!(fds(), fd_before, "native helper leaked a descriptor");
        if mode == "latency" {
            let mut nanos = [0u128; 10_000];
            for n in &mut nanos {
                let start = Instant::now();
                let tick = clock.capture().unwrap();
                *n = start.elapsed().as_nanos();
                assert!(tick.elapsed_since(&previous).is_ok());
                previous = tick;
            }
            nanos.sort_unstable();
            println!(
                "capture_ns count=10000 min={} p50={} p95={} p99={} max={}",
                nanos[0], nanos[4999], nanos[9499], nanos[9899], nanos[9999]
            );
        }
        println!("native_control userspace_allocations=0 fd_delta=0");
    }
}
#[cfg(all(
    target_os = "linux",
    target_env = "gnu",
    target_pointer_width = "64",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn main() {
    supported::run();
}
#[cfg(not(all(
    target_os = "linux",
    target_env = "gnu",
    target_pointer_width = "64",
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]
fn main() {
    panic!("native profiling is unsupported on this target");
}
