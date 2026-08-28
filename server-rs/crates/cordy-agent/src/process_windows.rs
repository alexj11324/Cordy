use std::io;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_SET_QUOTA,
    PROCESS_TERMINATE, THREAD_SUSPEND_RESUME,
};

use super::ProcessTreeSignal;

pub(crate) struct ProcessTree {
    job: JobObject,
}

pub(crate) fn prepare(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

pub(crate) async fn claim(child: &mut Child) -> io::Result<ProcessTree> {
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawned agent has no process id"))?;

    let job = match JobObject::new().and_then(|job| job.assign(pid).map(|()| job)) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(io::Error::new(
                error.kind(),
                format!("could not assign agent process {pid} to a job object: {error}"),
            ));
        }
    };

    if let Err(error) = resume_process(pid) {
        let _ = job.terminate();
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(io::Error::new(
            error.kind(),
            format!("resume suspended agent process {pid}: {error}"),
        ));
    }

    Ok(ProcessTree { job })
}

impl ProcessTree {
    pub(crate) fn is_fully_owned(&self) -> bool {
        true
    }

    pub(crate) fn signal(&self, child: &mut Child, _signal: ProcessTreeSignal) -> io::Result<()> {
        if self.job.terminate().is_ok() {
            return Ok(());
        }
        child.start_kill()
    }

    pub(crate) async fn wait_gone(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.job.active_processes() {
                Ok(0) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.job.terminate();
    }
}

struct JobObject {
    handle: HANDLE,
}

// A Job Object handle is an owned kernel reference whose documented APIs are
// thread-safe. This type never exposes or aliases ownership of the raw handle.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    fn new() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(information).cast(),
                std::mem::size_of_val(&information) as u32,
            )
        };
        if configured == 0 {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        Ok(Self { handle })
    }

    fn assign(&self, pid: u32) -> io::Result<()> {
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let assigned = unsafe { AssignProcessToJobObject(self.handle, process) };
        unsafe { CloseHandle(process) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn terminate(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.handle, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn active_processes(&self) -> io::Result<u32> {
        let mut information = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                std::ptr::addr_of_mut!(information).cast(),
                std::mem::size_of_val(&information) as u32,
                std::ptr::null_mut(),
            )
        };
        if queried == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(information.ActiveProcesses)
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

fn resume_process(pid: u32) -> io::Result<()> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..THREADENTRY32::default()
    };
    let mut resumed = 0_u32;
    let mut found = unsafe { Thread32First(snapshot, std::ptr::addr_of_mut!(entry)) } != 0;
    while found {
        if entry.th32OwnerProcessID == pid {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                let error = io::Error::last_os_error();
                unsafe { CloseHandle(snapshot) };
                return Err(error);
            }
            let previous_count = unsafe { ResumeThread(thread) };
            unsafe { CloseHandle(thread) };
            if previous_count == u32::MAX {
                let error = io::Error::last_os_error();
                unsafe { CloseHandle(snapshot) };
                return Err(error);
            }
            resumed += 1;
        }
        found = unsafe { Thread32Next(snapshot, std::ptr::addr_of_mut!(entry)) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    if resumed == 0 {
        return Err(io::Error::other(format!(
            "no suspended thread found for agent process {pid}"
        )));
    }
    Ok(())
}
