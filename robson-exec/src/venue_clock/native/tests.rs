use std::collections::VecDeque;

use super::*;

const UUID: &[u8] = b"12345678-1234-4567-8234-123456789abc\n";
const OFFSET: &[u8] = b"monotonic 0 0\nboottime -2 7\n";

struct Fake {
    thread: Vec<u8>,
    expected_path: Vec<u8>,
    boot: VecDeque<Vec<u8>>,
    offsets: VecDeque<Vec<u8>>,
    links: VecDeque<Vec<u8>>,
    active: Option<(Vec<u8>, usize)>,
    calls: Vec<&'static str>,
    fail_at: Option<usize>,
    interrupted: bool,
    chunk: usize,
    time: (i64, i64),
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            thread: b"123/task/456".to_vec(),
            expected_path: b"/proc/456/timens_offsets".to_vec(),
            boot: vec![UUID.to_vec(); 2].into(),
            offsets: vec![OFFSET.to_vec(); 2].into(),
            links: vec![b"time:[42]".to_vec(); 4].into(),
            active: None,
            calls: vec![],
            fail_at: None,
            interrupted: false,
            chunk: usize::MAX,
            time: (100, 5),
        }
    }
}
impl Fake {
    fn call(&mut self, operation: &'static str) -> Result<(), ClockError> {
        self.calls.push(operation);
        if self.fail_at == Some(self.calls.len()) {
            Err(ClockError::Native { operation, errno: libc::EIO })
        } else {
            Ok(())
        }
    }
}
impl NativeIo for Fake {
    fn open(&mut self, path: &CStr) -> Result<i32, ClockError> {
        self.call("open")?;
        assert!(self.active.is_none(), "no overlapping file ownership");
        let bytes = if path == c"/proc/sys/kernel/random/boot_id" {
            self.boot.pop_front()
        } else {
            assert_eq!(path.to_bytes(), self.expected_path, "procfs suffix selects current thread");
            self.offsets.pop_front()
        }
        .unwrap();
        self.active = Some((bytes, 0));
        Ok(9)
    }
    fn read(&mut self, fd: i32, buffer: &mut [u8]) -> Result<usize, ClockError> {
        assert_eq!(fd, 9);
        self.call("read")?;
        if self.interrupted {
            return Err(ClockError::Native { operation: "read", errno: libc::EINTR });
        }
        let (bytes, cursor) = self.active.as_mut().unwrap();
        let n = (bytes.len() - *cursor).min(buffer.len()).min(self.chunk);
        buffer[..n].copy_from_slice(&bytes[*cursor..*cursor + n]);
        *cursor += n;
        Ok(n)
    }
    fn close(&mut self, fd: i32) -> Result<(), ClockError> {
        assert_eq!(fd, 9);
        assert!(self.active.take().is_some(), "descriptor must close exactly once");
        self.call("close")
    }
    fn readlink(&mut self, path: &CStr, buffer: &mut [u8]) -> Result<usize, ClockError> {
        if path == c"/proc/thread-self" {
            self.call("readlink")?;
            let n = self.thread.len().min(buffer.len());
            buffer[..n].copy_from_slice(&self.thread[..n]);
            return Ok(n);
        }
        let remaining = self.links.len();
        assert_eq!(
            path,
            if remaining.is_multiple_of(2) {
                c"/proc/thread-self/ns/time"
            } else {
                c"/proc/thread-self/ns/time_for_children"
            },
            "calling-thread metadata path"
        );
        self.call("readlink")?;
        let link = self.links.pop_front().unwrap();
        let n = link.len().min(buffer.len());
        buffer[..n].copy_from_slice(&link[..n]);
        Ok(n)
    }
    fn clock(&mut self) -> Result<(i64, i64), ClockError> {
        self.call("clock")?;
        Ok(self.time)
    }
}

#[test]
fn exact_protocol_closes_every_file_and_preserves_tick_domain() {
    let mut io = Fake::default();
    let capture = initialize_with(&mut io).unwrap();
    assert_eq!(capture.domain().offset(), (-2, 7));
    assert_eq!(capture.nanoseconds(), 100_000_000_005);
    assert_eq!(io.calls, [
        "readlink", "readlink", "readlink", "open", "read", "read", "close", "open", "read",
        "read", "close", "clock", "open", "read", "read", "close", "open", "read", "read", "close",
        "readlink", "readlink"
    ]);
    assert!(io.active.is_none());
    let mut bounded = Fake { chunk: 13, ..Fake::default() };
    initialize_with(&mut bounded).unwrap();
    assert_eq!(bounded.calls.len(), 30, "maximum successful native work");
}

#[test]
fn every_native_error_refuses_and_releases_owned_descriptors() {
    for at in 1..=22 {
        let mut io = Fake { fail_at: Some(at), ..Fake::default() };
        assert!(initialize_with(&mut io).is_err(), "native failure at {at} was hidden");
        assert!(io.active.is_none(), "descriptor leaked after failure at {at}");
        assert!(io.calls.len() <= 30);
    }
}

#[test]
fn stable_child_mismatch_and_observed_domain_changes_refuse() {
    for at in [1, 3] {
        let mut io = Fake::default();
        io.links[at] = b"time:[43]".to_vec();
        assert!(matches!(initialize_with(&mut io), Err(ClockError::NamespaceMismatch)));
    }
    let mut io = Fake::default();
    io.links[2] = b"time:[43]".to_vec();
    io.links[3] = b"time:[43]".to_vec();
    assert_eq!(initialize_with(&mut io), Err(ClockError::DomainChanged));
    let mut io = Fake::default();
    io.offsets[1] = b"monotonic 0 0\nboottime -2 8\n".to_vec();
    assert_eq!(initialize_with(&mut io), Err(ClockError::DomainChanged));
    let mut io = Fake::default();
    io.boot[1] = b"22345678-1234-4567-8234-123456789abc\n".to_vec();
    assert_eq!(initialize_with(&mut io), Err(ClockError::DomainChanged));
}

#[test]
fn malformed_metadata_never_becomes_a_domain() {
    for bytes in [
        b"".as_slice(),
        b"boottime 0 0\n",
        b"monotonic 0 0\nboottime 0 0\nboottime 0 0\n",
        b"monotonic 0 0\nboottime 0 1000000000\n",
        b"monotonic 0 0\nboottime 9223372036854775808 0\n",
        b"monotonic 0 0\nboottime 0 0 trailing\n",
        b"unknown 0 0\n",
        b"\xff",
    ] {
        // Keep both reads identical: domain-change rejection must not mask parser
        // defects.
        let mut io = Fake {
            offsets: vec![bytes.to_vec(); 2].into(),
            ..Fake::default()
        };
        assert!(initialize_with(&mut io).is_err(), "invalid offsets accepted: {bytes:?}");
        assert!(io.active.is_none());
    }
    for bytes in [
        b"not-a-uuid".as_slice(),
        b"00000000-0000-0000-0000-000000000000",
        b"12345678-1234-4567-8234-123456789abc x",
    ] {
        let mut io = Fake::default();
        io.boot[0] = bytes.to_vec();
        assert!(initialize_with(&mut io).is_err());
    }
    for bytes in [
        b"time:[0]".as_slice(),
        b"time:[+42]",
        b"other:[42]",
        b"time:[42]x",
        b"",
    ] {
        let mut io = Fake::default();
        io.links[0] = bytes.to_vec();
        assert!(initialize_with(&mut io).is_err());
    }
}

#[test]
fn byte_and_attempt_bounds_are_not_silent_truncation_or_retry() {
    let mut exact = Fake::default();
    let mut boot = UUID.to_vec();
    boot.resize(128, b' ');
    let mut offsets = b"monotonic".to_vec();
    offsets.resize(1024 - b"0 0\nboottime -2 7\n".len(), b' ');
    offsets.extend_from_slice(b"0 0\nboottime -2 7\n");
    exact.boot = vec![boot; 2].into();
    exact.offsets = vec![offsets; 2].into();
    initialize_with(&mut exact).expect("metadata exactly at both byte caps remains valid");
    let mut over_boot = Fake::default();
    let mut boot = UUID.to_vec();
    boot.resize(129, b' ');
    over_boot.boot = vec![boot; 2].into();
    assert_eq!(initialize_with(&mut over_boot), Err(ClockError::Oversized));
    let mut over_offsets = Fake::default();
    let mut offsets = b"monotonic".to_vec();
    offsets.resize(1025 - b"0 0\nboottime -2 7\n".len(), b' ');
    offsets.extend_from_slice(b"0 0\nboottime -2 7\n");
    over_offsets.offsets = vec![offsets; 2].into();
    assert_eq!(initialize_with(&mut over_offsets), Err(ClockError::Oversized));
    for which in 0..3 {
        let mut io = Fake::default();
        match which {
            0 => io.boot[0] = vec![b'0'; 129],
            1 => io.offsets[0] = vec![b' '; 1025],
            _ => io.links[0] = vec![b'x'; 128],
        }
        assert_eq!(initialize_with(&mut io), Err(ClockError::Oversized));
        assert!(io.active.is_none());
    }
    for interrupted in [false, true] {
        let mut io = Fake { interrupted, chunk: 1, ..Fake::default() };
        assert_eq!(initialize_with(&mut io), Err(ClockError::ReadLimit));
        assert_eq!(io.calls.iter().filter(|&&c| c == "read").count(), 4);
        assert_eq!(io.calls.iter().filter(|&&c| c == "close").count(), 1);
        assert!(io.active.is_none());
    }
}

#[test]
fn actual_linux_capture_executes_and_native_abi_is_lp64() {
    assert_eq!(std::mem::size_of::<libc::timespec>(), 16);
    assert_eq!(std::mem::align_of::<libc::timespec>(), 8);
    std::thread::Builder::new()
        .stack_size(1_048_576)
        .spawn(|| {
            let target = std::fs::read_link("/proc/thread-self").unwrap();
            let (tgid, tid) = target.to_str().unwrap().split_once("/task/").unwrap();
            assert_ne!(tgid, tid, "real smoke runs in a nonleader thread");
            let clock = crate::venue_clock::NativeClock::initialize()
                .expect("real Linux source must execute");
            let a = clock.initial_capture();
            let b = clock.capture().unwrap();
            assert!(b.elapsed_since(&a).is_ok());
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn procfs_coordinates_are_canonical_and_independent_of_local_virtual_ids() {
    // An ancestor-procfs TID can differ from the task's local virtual ID.
    // This injection checks selection, not an actual changed namespace.
    let mut io = Fake::default();
    initialize_with(&mut io).unwrap(); // open checks literal /proc/456, never 7 or123.
    let mut maximum = Fake {
        thread: b"2147483647/task/2147483647".to_vec(),
        expected_path: b"/proc/2147483647/timens_offsets".to_vec(),
        ..Fake::default()
    };
    initialize_with(&mut maximum).unwrap();
    for invalid in [
        "",
        "0/task/2",
        "1/task/0",
        "01/task/2",
        "1/task/02",
        "+1/task/2",
        "1/task/+2",
        "1/task/-2",
        "1/task/2147483648",
        "2147483648/task/2",
        "12345678901/task/2",
        "1/task/2/x",
        "1/task/2\0",
        "1/task/2 ",
        "1/task/2/task/3",
        "1/task/",
        "/task/2",
        "1//2",
    ] {
        let mut bad = Fake {
            thread: invalid.as_bytes().to_vec(),
            ..Fake::default()
        };
        assert!(initialize_with(&mut bad).is_err(), "invalid thread link: {invalid:?}");
        assert_eq!(bad.calls, ["readlink"], "invalid identity must precede every file open");
    }
    let mut truncated = Fake {
        thread: vec![b'1'; 64],
        ..Fake::default()
    };
    assert_eq!(initialize_with(&mut truncated), Err(ClockError::Oversized));
}
