//! Cross-platform ownership of an agent CLI and every descendant it spawns.

use std::io;
use std::process::ExitStatus;
use std::time::Duration;

use tokio::process::{Child, Command};

#[cfg(unix)]
#[path = "process_unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "process_windows.rs"]
mod platform;

#[cfg(not(any(unix, windows)))]
compile_error!("cordy-agent process-tree ownership supports only Unix and Windows");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTreeSignal {
    Terminate,
    Kill,
}

/// A spawned child together with the operating-system ownership boundary for
/// all descendants it creates.
///
/// Unix establishes a new process group atomically at spawn. Windows creates
/// the child suspended, assigns it to a kill-on-close Job Object, and only then
/// resumes its initial thread, closing the escape window in which a shim could
/// otherwise launch the real agent outside the job. Assignment failure is a
/// hard launch error: the still-suspended child is killed rather than resumed
/// unowned.
pub struct OwnedProcessTree {
    child: Child,
    platform: platform::ProcessTree,
}

impl OwnedProcessTree {
    pub async fn spawn(command: &mut Command) -> io::Result<Self> {
        platform::prepare(command);
        let mut child = command.spawn()?;
        let platform = match platform::claim(&mut child).await {
            Ok(platform) => platform,
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error);
            }
        };
        Ok(Self { child, platform })
    }

    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// True only when whole-tree termination and liveness can be confirmed.
    pub fn is_fully_owned(&self) -> bool {
        self.platform.is_fully_owned()
    }

    pub fn child(&self) -> &Child {
        &self.child
    }

    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    pub fn signal(&mut self, signal: ProcessTreeSignal) -> io::Result<()> {
        self.platform.signal(&mut self.child, signal)
    }

    pub fn terminate(&mut self) -> io::Result<()> {
        self.signal(ProcessTreeSignal::Terminate)
    }

    pub fn kill(&mut self) -> io::Result<()> {
        self.signal(ProcessTreeSignal::Kill)
    }

    /// Waits for the ownership boundary, not merely the direct child, to have
    /// no members.
    pub async fn wait_tree_gone(&self, timeout: Duration) -> bool {
        self.platform.wait_gone(timeout).await
    }

    /// Requests graceful whole-tree termination, escalates after the supplied
    /// bound, and confirms the ownership boundary is empty. Drop remains the
    /// final synchronous kill backstop if either bounded wait fails.
    pub async fn shutdown(&mut self, terminate_grace: Duration, kill_grace: Duration) -> bool {
        let _ = self.terminate();
        if tokio::time::timeout(terminate_grace, self.wait())
            .await
            .is_err()
        {
            let _ = self.kill();
            let _ = tokio::time::timeout(kill_grace, self.wait()).await;
        }
        if self.wait_tree_gone(kill_grace).await {
            return true;
        }
        let _ = self.kill();
        self.wait_tree_gone(kill_grace).await
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Stdio;

    use tokio::io::{AsyncBufReadExt, BufReader};

    use super::*;

    struct ProcessGroupCleanup(i32);

    impl Drop for ProcessGroupCleanup {
        fn drop(&mut self) {
            if self.0 > 0 {
                unsafe {
                    libc::kill(-self.0, libc::SIGKILL);
                }
            }
        }
    }

    #[tokio::test]
    async fn kill_reaches_descendant_after_group_leader_exits() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 60 & child=$!; echo $child; exit 0"])
            .stdout(Stdio::piped());
        let mut tree = OwnedProcessTree::spawn(&mut command)
            .await
            .unwrap_or_else(|error| panic!("spawn process tree: {error}"));
        assert!(tree.is_fully_owned());
        let mut cleanup = ProcessGroupCleanup(
            tree.id()
                .and_then(|pid| i32::try_from(pid).ok())
                .unwrap_or_else(|| panic!("spawned process tree must have a valid group id")),
        );

        let Some(stdout) = tree.child_mut().stdout.take() else {
            panic!("child stdout pipe must exist");
        };
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .await
            .unwrap_or_else(|error| panic!("read descendant pid: {error}"));
        let descendant: i32 = line
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("parse descendant pid: {error}"));

        let status = tree
            .wait()
            .await
            .unwrap_or_else(|error| panic!("wait group leader: {error}"));
        assert!(status.success());
        assert_eq!(unsafe { libc::kill(descendant, 0) }, 0);

        tree.kill()
            .unwrap_or_else(|error| panic!("kill process group: {error}"));
        assert!(tree.wait_tree_gone(Duration::from_secs(5)).await);
        cleanup.0 = 0;
        assert_ne!(unsafe { libc::kill(descendant, 0) }, 0);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[tokio::test]
    async fn drop_kills_the_owned_process_group() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 60 & child=$!; echo $child; wait"])
            .stdout(Stdio::piped());
        let mut tree = OwnedProcessTree::spawn(&mut command)
            .await
            .unwrap_or_else(|error| panic!("spawn process tree: {error}"));
        let Some(stdout) = tree.child_mut().stdout.take() else {
            panic!("child stdout pipe must exist");
        };
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .await
            .unwrap_or_else(|error| panic!("read descendant pid: {error}"));
        let descendant: i32 = line
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("parse descendant pid: {error}"));

        drop(tree);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if unsafe { libc::kill(descendant, 0) } != 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "descendant survived process-tree ownership drop"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
