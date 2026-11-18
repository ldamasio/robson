use std::ffi::CStr;

use super::{ClockDomain, ClockError, TickCapture};
use crate::clock_records::RawClockCapture;

mod syscalls;
use syscalls::System;

const CURRENT: &CStr = c"/proc/thread-self/ns/time";
const CHILDREN: &CStr = c"/proc/thread-self/ns/time_for_children";
const BOOT: &CStr = c"/proc/sys/kernel/random/boot_id";
const THREAD: &CStr = c"/proc/thread-self";

trait NativeIo {
    fn open(&mut self, path: &CStr) -> Result<i32, ClockError>;
    fn read(&mut self, fd: i32, buffer: &mut [u8]) -> Result<usize, ClockError>;
    fn close(&mut self, fd: i32) -> Result<(), ClockError>;
    fn readlink(&mut self, path: &CStr, buffer: &mut [u8]) -> Result<usize, ClockError>;
    fn clock(&mut self) -> Result<(i64, i64), ClockError>;
}

struct File<'a, I: NativeIo> {
    io: &'a mut I,
    fd: Option<i32>,
}
impl<I: NativeIo> File<'_, I> {
    fn finish(mut self) -> Result<(), ClockError> {
        let fd = self.fd.take().expect("owned file descriptor");
        self.io.close(fd)
    }
}
impl<I: NativeIo> Drop for File<'_, I> {
    fn drop(&mut self) {
        // A failing path is already unable to publish. Never retry close: even
        // an interrupted Linux close may have released this descriptor.
        if let Some(fd) = self.fd.take() {
            let _ = self.io.close(fd);
        }
    }
}

fn read_file(io: &mut impl NativeIo, path: &CStr, buffer: &mut [u8]) -> Result<usize, ClockError> {
    let fd = io.open(path)?;
    let file = File { io, fd: Some(fd) };
    let mut used = 0;
    for _ in 0..4 {
        match file.io.read(fd, &mut buffer[used..]) {
            Ok(0) => {
                file.finish()?;
                return Ok(used);
            },
            Ok(n) if n < buffer.len() - used => used += n,
            Ok(_) => return Err(ClockError::Oversized),
            Err(ClockError::Native { errno: libc::EINTR, .. }) => (),
            Err(error) => return Err(error),
        }
    }
    Err(ClockError::ReadLimit)
}

fn text(bytes: &[u8]) -> Result<&str, ClockError> {
    if !bytes.is_ascii() {
        return Err(ClockError::Metadata("non-ascii"));
    }
    std::str::from_utf8(bytes).map_err(|_| ClockError::Metadata("encoding"))
}

fn offsets_path<'a>(
    io: &mut impl NativeIo,
    path: &'a mut [u8; 64],
) -> Result<&'a CStr, ClockError> {
    let mut link = [0; 64];
    let length = io.readlink(THREAD, &mut link)?;
    if length >= link.len() {
        return Err(ClockError::Oversized);
    }
    let (tgid, tid) = text(&link[..length])?
        .split_once("/task/")
        .ok_or(ClockError::Metadata("thread identity"))?;
    for id in [tgid, tid] {
        if id.is_empty()
            || id.len() > 10
            || !(b'1'..=b'9').contains(&id.as_bytes()[0])
            || !id.bytes().all(|b| b.is_ascii_digit())
            || id.parse::<i32>().is_err()
        {
            return Err(ClockError::Metadata("thread identity"));
        }
    }
    // Both IDs belong to this mounted procfs; a local gettid may not.
    let end = 6 + tid.len();
    path[..6].copy_from_slice(b"/proc/");
    path[6..end].copy_from_slice(tid.as_bytes());
    path[end..end + 16].copy_from_slice(b"/timens_offsets\0");
    CStr::from_bytes_with_nul(&path[..end + 16]).map_err(|_| ClockError::Metadata("thread path"))
}

fn namespace(io: &mut impl NativeIo, path: &CStr) -> Result<u64, ClockError> {
    let mut buffer = [0; 128];
    let length = io.readlink(path, &mut buffer)?;
    if length >= buffer.len() {
        return Err(ClockError::Oversized);
    }
    let digits = text(&buffer[..length])?
        .strip_prefix("time:[")
        .and_then(|s| s.strip_suffix(']'))
        .ok_or(ClockError::Metadata("namespace"))?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ClockError::Metadata("namespace"));
    }
    let id = digits.parse().map_err(|_| ClockError::Metadata("namespace"))?;
    if id == 0 {
        return Err(ClockError::Metadata("namespace"));
    }
    Ok(id)
}

fn namespaces(io: &mut impl NativeIo) -> Result<u64, ClockError> {
    let current = namespace(io, CURRENT)?;
    if current != namespace(io, CHILDREN)? {
        return Err(ClockError::NamespaceMismatch);
    }
    Ok(current)
}

fn domain(io: &mut impl NativeIo, path: &CStr) -> Result<ClockDomain, ClockError> {
    let mut boot = [0; 129];
    let count = read_file(io, BOOT, &mut boot)?;
    let boot = text(&boot[..count])?
        .trim()
        .parse()
        .map_err(|_| ClockError::Metadata("boot uuid"))?;
    let mut offsets = [0; 1025];
    let count = read_file(io, path, &mut offsets)?;
    let mut monotonic = None;
    let mut boottime = None;
    for line in text(&offsets[..count])?.lines() {
        let mut fields = line.split_ascii_whitespace();
        let target = match fields.next() {
            Some("monotonic") => &mut monotonic,
            Some("boottime") => &mut boottime,
            _ => return Err(ClockError::Metadata("offset clock")),
        };
        let seconds = fields.next().and_then(|s| s.parse::<i64>().ok());
        let nanos = fields.next().and_then(|s| s.parse::<u32>().ok());
        let (Some(seconds), Some(nanos)) = (seconds, nanos) else {
            return Err(ClockError::Metadata("offset value"));
        };
        if nanos >= 1_000_000_000 || fields.next().is_some() || target.is_some() {
            return Err(ClockError::Metadata("offset shape"));
        }
        *target = Some((seconds, nanos));
    }
    if monotonic.is_none() {
        return Err(ClockError::Metadata("missing monotonic"));
    }
    let (seconds, nanos) = boottime.ok_or(ClockError::Metadata("missing boottime"))?;
    ClockDomain::new(boot, seconds, nanos)
}

fn initialize_with(io: &mut impl NativeIo) -> Result<TickCapture, ClockError> {
    let mut storage = [0; 64];
    let path = offsets_path(io, &mut storage)?;
    let before_ns = namespaces(io)?;
    let before = domain(io, path)?;
    let (seconds, nanos) = io.clock()?;
    let capture = TickCapture::new(before, seconds, nanos)?;
    let after = domain(io, path)?;
    let after_ns = namespaces(io)?;
    if before != after || before_ns != after_ns {
        return Err(ClockError::DomainChanged);
    }
    Ok(capture)
}

pub(super) fn initialize() -> Result<TickCapture, ClockError> {
    initialize_with(&mut System)
}
pub(super) fn tick() -> Result<(i64, i64), ClockError> {
    System.clock()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadClock {
    Boottime,
    Realtime,
}

fn wall_bracket_with(
    domain: ClockDomain,
    epoch: uuid::Uuid,
    mut read: impl FnMut(ReadClock) -> Result<(i64, i64), ClockError>,
) -> Result<RawClockCapture, ClockError> {
    let (seconds, nanos) = read(ReadClock::Boottime)?;
    let before = TickCapture::new(domain, seconds, nanos)?;
    let (wall_seconds, wall_nanos) = read(ReadClock::Realtime)?;
    if !(0..1_000_000_000).contains(&wall_nanos) {
        return Err(ClockError::InvalidTick);
    }
    let (seconds, nanos) = read(ReadClock::Boottime)?;
    let after = TickCapture::new(domain, seconds, nanos)?;
    after.elapsed_since(&before)?;
    let raw = RawClockCapture {
        epoch,
        wall_seconds,
        wall_nanoseconds: u32::try_from(wall_nanos).map_err(|_| ClockError::Range)?,
        domain,
        before_ns: i128::try_from(before.nanoseconds()).map_err(|_| ClockError::Range)?,
        after_ns: i128::try_from(after.nanoseconds()).map_err(|_| ClockError::Range)?,
    };
    raw.check()?;
    Ok(raw)
}

pub(super) fn wall_bracket(
    domain: ClockDomain,
    epoch: uuid::Uuid,
) -> Result<RawClockCapture, ClockError> {
    wall_bracket_with(domain, epoch, |clock| match clock {
        ReadClock::Boottime => System.clock(),
        ReadClock::Realtime => syscalls::realtime(),
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod wall_tests {
    use super::*;

    #[test]
    fn bracket_retains_original_epoch_domain_wall_and_exact_ticks_in_order() {
        let epoch = uuid::Uuid::from_u128(17);
        let domain = ClockDomain::new(uuid::Uuid::from_u128(3), -4, 9).unwrap();
        let mut calls = Vec::new();
        let mut values = [(i64::MAX, 7), (-5, 999_999_999), (i64::MAX, 8)].into_iter();
        let raw = wall_bracket_with(domain, epoch, |clock| {
            calls.push(clock);
            Ok(values.next().unwrap())
        })
        .unwrap();
        assert_eq!(calls, [
            ReadClock::Boottime,
            ReadClock::Realtime,
            ReadClock::Boottime
        ]);
        assert!(values.next().is_none(), "exactly three native reads");
        assert_eq!(raw.epoch, epoch);
        assert_eq!(raw.domain, domain);
        assert_eq!((raw.wall_seconds, raw.wall_nanoseconds), (-5, 999_999_999));
        assert_eq!(raw.before_ns, i128::from(i64::MAX) * 1_000_000_000 + 7);
        assert_eq!(raw.after_ns, raw.before_ns + 1);
    }

    #[test]
    fn bracket_propagates_each_read_error_without_retry_or_later_reads() {
        let domain = ClockDomain::new(uuid::Uuid::from_u128(3), 0, 0).unwrap();
        for failed in 0..3 {
            let mut calls = 0;
            let error = ClockError::Native { operation: "fixture", errno: libc::EIO };
            let actual = wall_bracket_with(domain, uuid::Uuid::from_u128(1), |_| {
                let index = calls;
                calls += 1;
                if index == failed {
                    Err(error)
                } else {
                    Ok((10, 0))
                }
            });
            assert_eq!(actual, Err(error));
            assert_eq!(calls, failed + 1);
        }
    }

    #[test]
    fn bracket_refuses_bad_nanos_negative_ticks_reversal_and_nil_epoch() {
        let domain = ClockDomain::new(uuid::Uuid::from_u128(3), 0, 0).unwrap();
        for values in [
            [(-1, 0), (10, 0), (11, 0)],
            [(10, -1), (10, 0), (11, 0)],
            [(10, 0), (10, -1), (11, 0)],
            [(10, 0), (10, 1_000_000_000), (11, 0)],
            [(10, 0), (10, 0), (11, 1_000_000_000)],
            [(10, 1), (10, 0), (10, 0)],
        ] {
            let mut values = values.into_iter();
            assert!(wall_bracket_with(domain, uuid::Uuid::from_u128(1), |_| {
                Ok(values.next().unwrap())
            })
            .is_err());
        }
        assert!(wall_bracket_with(domain, uuid::Uuid::nil(), |_| Ok((0, 0))).is_err());
    }
}
