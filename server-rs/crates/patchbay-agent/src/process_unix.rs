use std::io;
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};

use super::ProcessTreeSignal;

pub(crate) struct ProcessTree {
    process_group_id: i32,
}

pub(crate) fn prepare(command: &mut Command) {
    command.process_group(0);
}

pub(crate) async fn claim(child: &mut Child) -> io::Result<ProcessTree> {
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("spawned agent has no process id"))?;
    let process_group_id = i32::try_from(pid)
        .map_err(|_| io::Error::other(format!("agent process id {pid} exceeds i32")))?;
    Ok(ProcessTree { process_group_id })
}

impl ProcessTree {
    pub(crate) fn is_fully_owned(&self) -> bool {
        true
    }

    pub(crate) fn signal(&self, _child: &mut Child, signal: ProcessTreeSignal) -> io::Result<()> {
        let signal = match signal {
            ProcessTreeSignal::Terminate => libc::SIGTERM,
            ProcessTreeSignal::Kill => libc::SIGKILL,
        };
        if unsafe { libc::kill(-self.process_group_id, signal) } == 0 {
            return Ok(());
        }
        let group_error = io::Error::last_os_error();
        if group_error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        if unsafe { libc::kill(self.process_group_id, signal) } == 0 {
            return Ok(());
        }
        let direct_error = io::Error::last_os_error();
        if direct_error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(io::Error::new(
            direct_error.kind(),
            format!(
                "signal process group {}: {group_error}; direct child fallback: {direct_error}",
                self.process_group_id
            ),
        ))
    }

    pub(crate) async fn wait_gone(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if unsafe { libc::kill(-self.process_group_id, 0) } != 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
