//! The local named pipe between the app (server) and the engine (client).
//!
//! - The app creates it with [`Listener::create`]: a random name, one instance only
//!   (`FILE_FLAG_FIRST_PIPE_INSTANCE`, so nobody can have created it first), a protected DACL
//!   that grants access to the current user's SID and nobody else, and
//!   `PIPE_REJECT_REMOTE_CLIENTS`.
//! - It spawns the engine with the name and waits in [`Listener::accept`]. The connected
//!   client's process id ([`Pipe::client_pid`]) must be the pid it spawned, or the connection
//!   is dropped. The engine likewise checks [`Pipe::server_pid`] against the pid it was given.
//!
//! I/O is overlapped, so one thread can block reading while another writes (synchronous I/O
//! on one handle is serialised by Windows, which would deadlock a reader and a writer).
//! `&Pipe` implements [`Read`] and [`Write`]; a broken pipe reads as end of stream.
//!
//! Every write has a deadline ([`WRITE_TIMEOUT`] unless [`Pipe::set_write_timeout`] says
//! otherwise): a peer that is alive but stops reading fills the pipe, and neither side may then
//! hang in a write. A write that times out is cancelled and may have sent part of a frame, so
//! the session is over: callers drop the pipe (the app ends the engine, the engine shuts down).

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE,
    HLOCAL, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

const BUFFER: u32 = 64 * 1024;

/// How long one write may wait for the peer to read.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// A fresh pipe name: `\\.\pipe\unflub-engine-<pid>-<64 random bits>`.
pub fn unique_name() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded from the OS random source.
    let mut h = RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!(
        r"\\.\pipe\unflub-engine-{}-{:016x}",
        std::process::id(),
        h.finish()
    )
}

/// An owned pipe handle, connected (or, inside a [`Listener`], waiting for its client).
pub struct Pipe {
    handle: HANDLE,
    /// Milliseconds a write may take; 0 waits forever.
    write_timeout_ms: AtomicU64,
}

// SAFETY: a pipe handle may be used from any thread; overlapped I/O keeps reads and writes
// independent, and every operation waits for its own completion.
unsafe impl Send for Pipe {}
unsafe impl Sync for Pipe {}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// A pipe nobody has connected to yet.
pub struct Listener {
    pipe: Pipe,
}

impl Listener {
    /// Create the only instance of `name`, for the current user only, local clients only.
    pub fn create(name: &str) -> io::Result<Self> {
        let sddl = format!("D:P(A;;GA;;;{})", current_user_sid()?);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: valid string in, descriptor out (freed below with LocalFree).
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(sddl),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(win)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        // SAFETY: valid name and attributes; the descriptor outlives the call.
        let handle = unsafe {
            CreateNamedPipeW(
                &HSTRING::from(name),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                BUFFER,
                BUFFER,
                0,
                Some(&attributes),
            )
        };
        let error = io::Error::last_os_error();
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(HLOCAL(descriptor.0)) };
        if handle == INVALID_HANDLE_VALUE || handle.is_invalid() {
            return Err(error);
        }
        Ok(Self {
            pipe: Pipe::new(handle),
        })
    }

    /// Wait up to `timeout` for the client.
    pub fn accept(self, timeout: Duration) -> io::Result<Pipe> {
        let pipe = self.pipe;
        let handle = pipe.handle;
        // SAFETY: the OVERLAPPED lives on `overlapped`'s stack until the operation completes.
        let connected = pipe.overlapped(Some(timeout), |ov| unsafe {
            ConnectNamedPipe(handle, Some(ov))
        });
        match connected {
            Ok(_) => Ok(pipe),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED.0 as i32) => Ok(pipe),
            Err(e) => Err(e),
        }
    }
}

impl Pipe {
    /// Connect to the app's pipe.
    pub fn connect(name: &str) -> io::Result<Self> {
        // SAFETY: valid name; the returned handle is owned by the Pipe. SECURITY_IDENTIFICATION
        // keeps the server from impersonating us beyond identifying who we are.
        let handle = unsafe {
            CreateFileW(
                &HSTRING::from(name),
                (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED
                    | FILE_FLAGS_AND_ATTRIBUTES(
                        SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0,
                    ),
                None,
            )
        }
        .map_err(win)?;
        Ok(Self::new(handle))
    }

    fn new(handle: HANDLE) -> Self {
        Self {
            handle,
            write_timeout_ms: AtomicU64::new(WRITE_TIMEOUT.as_millis() as u64),
        }
    }

    /// How long each write may wait for the peer (`None`: forever).
    pub fn set_write_timeout(&self, timeout: Option<Duration>) {
        let ms = timeout.map_or(0, |t| t.as_millis().clamp(1, u128::from(u64::MAX)) as u64);
        self.write_timeout_ms.store(ms, Ordering::Relaxed);
    }

    /// Cancel every read and write in progress on this pipe from this process: a blocked
    /// reader returns an error, which ends its session.
    pub fn cancel_io(&self) {
        // SAFETY: cancels our own operations on our own handle; each waits for its completion.
        let _ = unsafe { CancelIoEx(self.handle, None) };
    }

    /// Process id of the connected client.
    pub fn client_pid(&self) -> io::Result<u32> {
        let mut pid = 0;
        // SAFETY: valid handle, pid is a local.
        unsafe { GetNamedPipeClientProcessId(self.handle, &mut pid) }.map_err(win)?;
        Ok(pid)
    }

    /// Process id of the server end.
    pub fn server_pid(&self) -> io::Result<u32> {
        let mut pid = 0;
        // SAFETY: valid handle, pid is a local.
        unsafe { GetNamedPipeServerProcessId(self.handle, &mut pid) }.map_err(win)?;
        Ok(pid)
    }

    /// Read with a deadline (for the handshake). `Ok(0)` at end of stream.
    pub fn read_timeout(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.read_inner(buf, Some(timeout))
    }

    fn read_inner(&self, buf: &mut [u8], timeout: Option<Duration>) -> io::Result<usize> {
        let handle = self.handle;
        let len = buf.len().min(u32::MAX as usize);
        let ptr = buf.as_mut_ptr();
        // SAFETY: buf outlives the operation, which `overlapped` waits for.
        let read = self.overlapped(timeout, |ov| unsafe {
            ReadFile(
                handle,
                Some(std::slice::from_raw_parts_mut(ptr, len)),
                None,
                Some(ov),
            )
        });
        match read {
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE.0 as i32) => Ok(0),
            other => other.map(|n| n as usize),
        }
    }

    /// Start an overlapped operation and wait for it (up to `timeout`, then cancel it).
    fn overlapped(
        &self,
        timeout: Option<Duration>,
        start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
    ) -> io::Result<u32> {
        // SAFETY: a manual-reset event, closed below.
        let event = unsafe { CreateEventW(None, true, false, None) }.map_err(win)?;
        let mut ov = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let result = (|| {
            if let Err(e) = start(&mut ov) {
                if e.code() != ERROR_IO_PENDING.to_hresult() {
                    return Err(win(e));
                }
            }
            if let Some(timeout) = timeout {
                let ms = timeout.as_millis().min(u128::from(INFINITE - 1)) as u32;
                // SAFETY: valid event handle.
                let waited = unsafe { WaitForSingleObject(event, ms) };
                if waited == WAIT_TIMEOUT {
                    // SAFETY: cancels only this operation; we still wait for it below so the
                    // OVERLAPPED isn't freed while the kernel holds it.
                    unsafe {
                        let _ = CancelIoEx(self.handle, Some(&ov));
                        let mut n = 0;
                        let _ = GetOverlappedResult(self.handle, &ov, &mut n, true);
                    }
                    return Err(io::ErrorKind::TimedOut.into());
                }
                if waited != WAIT_OBJECT_0 {
                    return Err(io::Error::last_os_error());
                }
            }
            let mut n = 0;
            // SAFETY: ov belongs to an operation started on this handle.
            unsafe { GetOverlappedResult(self.handle, &ov, &mut n, true) }.map_err(win)?;
            Ok(n)
        })();
        // SAFETY: the operation is complete; nothing refers to the event any more.
        let _ = unsafe { CloseHandle(event) };
        result
    }
}

impl Read for &Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_inner(buf, None)
    }
}

impl Write for &Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let handle = self.handle;
        let len = buf.len().min(u32::MAX as usize);
        let ptr = buf.as_ptr();
        let timeout = match self.write_timeout_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(Duration::from_millis(ms)),
        };
        // SAFETY: buf outlives the operation, which `overlapped` waits for.
        self.overlapped(timeout, |ov| unsafe {
            WriteFile(
                handle,
                Some(std::slice::from_raw_parts(ptr, len)),
                None,
                Some(ov),
            )
        })
        .map(|n| n as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The current user's SID as `S-1-5-21-...`.
fn current_user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle needs no closing; token is closed below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(win)?;
    let result = (|| {
        let mut len = 0;
        // SAFETY: size query; it fails with "insufficient buffer" and sets len.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
        // u64s keep the buffer aligned for TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: buf holds at least len bytes.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr() as *mut c_void),
                len,
                &mut len,
            )
        }
        .map_err(win)?;
        // SAFETY: GetTokenInformation(TokenUser) wrote a TOKEN_USER at the start of buf.
        let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
        let mut text = PWSTR::null();
        // SAFETY: the SID points into buf, alive here; text is freed with LocalFree.
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }.map_err(win)?;
        // SAFETY: a null-terminated string from ConvertSidToStringSidW.
        let sid = unsafe { text.to_string() }.map_err(io::Error::other);
        // SAFETY: allocated by ConvertSidToStringSidW.
        unsafe { LocalFree(HLOCAL(text.0 as *mut c_void)) };
        sid
    })();
    // SAFETY: opened above.
    let _ = unsafe { CloseHandle(token) };
    result
}

/// A Windows error as an `io::Error` with its Win32 code, so `ErrorKind`s work.
fn win(e: windows::core::Error) -> io::Error {
    let code = e.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        io::Error::other(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame;
    use crate::protocol::{FromEngine, ToEngine, PROTOCOL};
    use std::thread;

    /// Both ends in this process: the client is us, so its pid is ours.
    fn pair() -> (Pipe, Pipe) {
        let name = unique_name();
        let listener = Listener::create(&name).unwrap();
        let client = thread::spawn(move || Pipe::connect(&name).unwrap());
        let server = listener.accept(Duration::from_secs(5)).unwrap();
        (server, client.join().unwrap())
    }

    #[test]
    fn names_are_unique_and_local() {
        let (a, b) = (unique_name(), unique_name());
        assert_ne!(a, b);
        assert!(a.starts_with(r"\\.\pipe\unflub-engine-"));
    }

    #[test]
    fn handshake_and_messages_both_ways() {
        let (server, client) = pair();
        assert_eq!(server.client_pid().unwrap(), std::process::id());
        assert_eq!(client.server_pid().unwrap(), std::process::id());

        frame::write(
            &mut &client,
            &FromEngine::Hello {
                protocol: PROTOCOL,
                version: "test".into(),
            },
        )
        .unwrap();
        let hello: FromEngine =
            frame::decode(&frame::read(&mut &server).unwrap().unwrap()).unwrap();
        assert!(matches!(
            hello,
            FromEngine::Hello {
                protocol: PROTOCOL,
                ..
            }
        ));
        frame::write(&mut &server, &ToEngine::Shutdown).unwrap();
        let bye: ToEngine = frame::decode(&frame::read(&mut &client).unwrap().unwrap()).unwrap();
        assert_eq!(bye, ToEngine::Shutdown);
    }

    /// A blocked reader doesn't block a writer on the same handle (overlapped I/O).
    #[test]
    fn reading_and_writing_at_once() {
        let (server, client) = pair();
        let server = std::sync::Arc::new(server);
        let reader = {
            let server = server.clone();
            thread::spawn(move || frame::read(&mut &*server).unwrap())
        };
        thread::sleep(Duration::from_millis(50));
        // The server writes while its own read is pending.
        frame::write(&mut &*server, &ToEngine::Shutdown).unwrap();
        let got: ToEngine = frame::decode(&frame::read(&mut &client).unwrap().unwrap()).unwrap();
        assert_eq!(got, ToEngine::Shutdown);
        frame::write(&mut &client, &FromEngine::HookReinstalled { reason: None }).unwrap();
        let body = reader.join().unwrap().unwrap();
        assert_eq!(
            frame::decode::<FromEngine>(&body).unwrap(),
            FromEngine::HookReinstalled { reason: None }
        );
    }

    #[test]
    fn a_closed_peer_reads_as_end_of_stream() {
        let (server, client) = pair();
        drop(client);
        assert!(frame::read(&mut &server).unwrap().is_none());
    }

    #[test]
    fn the_name_can_only_be_created_once() {
        let name = unique_name();
        let _first = Listener::create(&name).unwrap();
        assert!(Listener::create(&name).is_err());
    }

    #[test]
    fn accept_and_read_time_out() {
        let listener = Listener::create(&unique_name()).unwrap();
        let err = listener.accept(Duration::from_millis(50)).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);

        let (server, _client) = pair();
        let mut buf = [0u8; 4];
        let err = server
            .read_timeout(&mut buf, Duration::from_millis(50))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    /// Review finding 2: a peer that stops reading can't block a writer for good.
    #[test]
    fn a_write_to_a_peer_that_stops_reading_times_out() {
        let (server, client) = pair();
        server.set_write_timeout(Some(Duration::from_millis(300)));
        let started = std::time::Instant::now();
        let big = vec![b'x'; 1024 * 1024];
        let err = (&server).write_all(&big).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(client);
    }

    #[test]
    fn cancelling_ends_a_blocked_read() {
        let (server, _client) = pair();
        let server = std::sync::Arc::new(server);
        let reader = {
            let server = server.clone();
            thread::spawn(move || frame::read(&mut &*server))
        };
        thread::sleep(Duration::from_millis(100));
        server.cancel_io();
        assert!(reader.join().unwrap().is_err());
    }

    #[test]
    fn the_dacl_names_the_current_user() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"), "{sid}");
    }
}
