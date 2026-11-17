//! A Job Object that ends the engine when the app goes away, however it goes (crash, Task
//! Manager, the installer killing `unflub.exe`). The engine also exits when its pipe breaks;
//! the job covers a hung engine and the moment before the pipe is connected.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// Closing the last handle (dropping this, or the app exiting) ends every process in it.
pub struct KillOnClose {
    handle: HANDLE,
}

// SAFETY: a job handle may be used and closed from any thread.
unsafe impl Send for KillOnClose {}
unsafe impl Sync for KillOnClose {}

impl KillOnClose {
    pub fn new() -> io::Result<Self> {
        // SAFETY: an anonymous job; the handle is owned by Self.
        let handle = unsafe { CreateJobObjectW(None, None) }.map_err(io::Error::other)?;
        let job = Self { handle };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: limits is a valid local of the size given.
        unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(io::Error::other)?;
        Ok(job)
    }

    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are valid while `self` and `child` are.
        unsafe { AssignProcessToJobObject(self.handle, HANDLE(child.as_raw_handle())) }
            .map_err(io::Error::other)
    }
}

impl Drop for KillOnClose {
    fn drop(&mut self) {
        // SAFETY: owned handle, closed once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}
