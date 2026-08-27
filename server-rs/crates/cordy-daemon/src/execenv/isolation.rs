//! Port of execenv/isolation.go (+ isolation_unix.go).
//!
//! Symbol map:
//! - PreparationHelperArg          → PREPARATION_HELPER_ARG
//! - preparationActionPrepare/Reuse → PREPARATION_ACTION_PREPARE / _REUSE
//! - preparationWaitDelay          → PREPARATION_WAIT_DELAY
//! - preparationRequest            → PreparationRequest
//! - preparationPrepareParams /
//!   preparationReuseParams        → (folded: the gateway token is carried
//!   plainly on the wire structs; see note)
//! - preparationResponse           → PreparationResponse
//! - preparationErrorKindOpenclawCLITimeout → PREPARATION_ERROR_KIND_OPENCLAW_CLI_TIMEOUT
//! - preparationErrorKind          → preparation_error_kind
//! - rehydratePreparationError     → rehydrate_preparation_error
//! - PrepareIsolated / ReuseIsolated → prepare_isolated / reuse_isolated
//! - runPreparationProcess         → run_preparation_process
//! - marshalPreparationRequest     → (serde handles the payload directly)
//! - decodePreparationRequest      → decode_preparation_request
//! - RunPreparationHelper          → run_preparation_helper
//! - preparationProcessController  → Unix process group / Windows Job Object;
//!   cancellation terminates the complete helper process tree
//!
//! Deviations:
//! - Go's DisallowUnknownFields is approximated with serde deny_unknown_fields.
//! - The OpenclawGateway token-masking dance exists in Go because the public
//!   type's MarshalJSON redacts Token. Our stand-in type serializes plainly
//!   already, so the private view types collapse into the request structs.
//! - slog logger dropped (tracing).
//! - WaitDelay semantics: cancellation terminates the platform process-tree
//!   boundary before awaiting the child and pipe readers.
//!
//! The parent-side entry points (prepare_isolated / reuse_isolated) are wired
//! into the production task launcher. The private helper path remains in this
//! module so the launcher and helper share one exact wire contract.
#![allow(dead_code)]

use std::ffi::OsString;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::execenv::{Environment, PrepareParams, ReuseParams};
use super::local_worktree::acquire_repository_lock_for_path;

/// Selection flag for the private execution-environment helper mode in the
/// cordy binary. The daemon runs Prepare/Reuse in that subprocess so a blocked
/// filesystem syscall can be terminated without leaving an in-process task
/// that may resume writing after the task has already been retried.
pub const PREPARATION_HELPER_ARG: &str = "__cordy_execenv_prepare";

pub(crate) const PREPARATION_ACTION_PREPARE: &str = "prepare";
pub(crate) const PREPARATION_ACTION_REUSE: &str = "reuse";
/// Grace period matching Go's cmd.WaitDelay before the group kill escalates.
pub(crate) const PREPARATION_WAIT_DELAY: Duration = Duration::from_secs(2);

/// Marks a helper failure caused by the local openclaw CLI missing its
/// deadline (ErrOpenclawCLITimeout).
pub(crate) const PREPARATION_ERROR_KIND_OPENCLAW_CLI_TIMEOUT: &str = "openclaw_cli_timeout";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparationRequest {
    pub action: String,
    #[serde(rename = "prepare", skip_serializing_if = "Option::is_none")]
    pub prepare: Option<PreparationWireParams>,
    #[serde(rename = "reuse", skip_serializing_if = "Option::is_none")]
    pub reuse: Option<ReuseWireParams>,
}

// The helper-protocol views carry the gateway pin plainly over this trusted
// local stdin pipe (Go's preparationOpenclawGatewayPin rationale).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparationWireParams {
    #[serde(flatten)]
    pub params: PrepareParams,
    /// The parent owns this lock until the returned worktree is finalized or
    /// discarded. The helper keeps the process-local mutex but must not block
    /// on a second kernel lock held by its parent.
    #[serde(default)]
    pub local_worktree_lock_held: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReuseWireParams {
    #[serde(flatten)]
    pub params: ReuseParams,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PreparationResponse {
    #[serde(rename = "environment", skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
    #[serde(rename = "error", skip_serializing_if = "String::is_empty", default)]
    pub error: String,
    /// ErrorKind names the error class the parent must be able to recognise
    /// structurally. Error itself only crosses the pipe as text.
    #[serde(
        rename = "error_kind",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub error_kind: String,
}

/// Sentinel mirroring execenv.ErrOpenclawCLITimeout (defined by lane E2's
/// openclaw_config port; until then only the wire kind survives the boundary).
#[derive(Debug, thiserror::Error)]
#[error("execenv: openclaw CLI timed out")]
pub struct ErrOpenclawCliTimeout;

/// preparation_error_kind names the class of err for the wire, or "" when it
/// has none. Recognizes both the local sentinel and an in-chain context match
/// so a helper on a newer build can classify without the sentinel defined.
pub(crate) fn preparation_error_kind(err: &anyhow::Error) -> &'static str {
    if err.is::<ErrOpenclawCliTimeout>() || format!("{err:#}").contains("openclaw CLI timed out") {
        return PREPARATION_ERROR_KIND_OPENCLAW_CLI_TIMEOUT;
    }
    ""
}

/// rehydrate_preparation_error rebuilds a typed error from the wire pair. An
/// unknown kind (helper newer than the daemon) degrades to a plain error with
/// the original message rather than being dropped.
pub(crate) fn rehydrate_preparation_error(message: &str, kind: &str) -> anyhow::Error {
    match kind {
        PREPARATION_ERROR_KIND_OPENCLAW_CLI_TIMEOUT => {
            anyhow::Error::new(ErrOpenclawCliTimeout).context(message.to_string())
        }
        _ => anyhow!(message.to_string()),
    }
}

/// prepare_isolated executes prepare() in a killable helper process. command
/// must name the current cordy binary followed by PREPARATION_HELPER_ARG in
/// production; accepting a slice also lets tests use the test binary as the
/// helper without installing a CLI binary.
#[allow(unused_imports)]
pub(crate) async fn prepare_isolated(
    ctx: &crate::repocache::Ctx,
    command: &[OsString],
    params: PrepareParams,
) -> anyhow::Result<Environment> {
    let repository_lock = match params.local_worktree.as_ref() {
        Some(local_worktree) => Some(
            acquire_repository_lock_for_path(ctx, &local_worktree.local_path).await?,
        ),
        None => None,
    };
    let mut environment = run_preparation_process(
        ctx,
        command,
        PreparationRequest {
            action: PREPARATION_ACTION_PREPARE.to_string(),
            prepare: Some(PreparationWireParams {
                params,
                local_worktree_lock_held: repository_lock.is_some(),
            }),
            reuse: None,
        },
    )
    .await?
    .ok_or_else(|| anyhow!("execenv: preparation helper returned no environment"))?;
    if let Some(lock) = repository_lock {
        let worktree = environment
            .local_worktree
            .as_mut()
            .ok_or_else(|| anyhow!("execenv: worktree preparation returned no worktree"))?;
        worktree.attach_repository_lock(lock);
    }
    Ok(environment)
}

/// reuse_isolated executes Reuse under the same killable-helper contract.
pub(crate) async fn reuse_isolated(
    ctx: &crate::repocache::Ctx,
    command: &[OsString],
    params: ReuseParams,
) -> anyhow::Result<Option<Environment>> {
    run_preparation_process(
        ctx,
        command,
        PreparationRequest {
            action: PREPARATION_ACTION_REUSE.to_string(),
            prepare: None,
            reuse: Some(ReuseWireParams { params }),
        },
    )
    .await
}

async fn run_preparation_process(
    ctx: &crate::repocache::Ctx,
    command: &[OsString],
    request: PreparationRequest,
) -> anyhow::Result<Option<Environment>> {
    if command.is_empty() || command[0].as_os_str().is_empty() {
        bail!("execenv: preparation helper command is empty");
    }
    if let Some(cause) = ctx.err() {
        return Err(anyhow!(cause.to_string()));
    }
    let payload = serde_json::to_vec(&request).context("execenv: encode preparation request")?;

    let mut cmd = tokio::process::Command::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Own process group (controller_unix.go): descendants die with the helper.
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    #[cfg(windows)]
    let process_job =
        WindowsProcessJob::new().context("execenv: create preparation process controller")?;

    let mut child = cmd.spawn().context("execenv: start preparation helper")?;
    let pid = child.id().unwrap_or_default();
    #[cfg(windows)]
    if let Err(err) = process_job.attach(pid) {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(err).context("execenv: attach preparation helper process");
    }

    let mut stdin = child
        .stdin
        .take()
        .context("execenv: create preparation stdin")?;
    let mut stdout = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    // Attach-to-kill ordering from Go: the group is set at spawn time, so the
    // payload can be released immediately afterwards.
    let mut write_task = tokio::spawn(async move {
        // Write then shutdown so the helper's decoder sees EOF. Both failures
        // ride back joined, mirroring Go's errors.Join at this boundary.
        let write_res = stdin.write_all(&payload).await.map(|_| ());
        let close_res = stdin.shutdown().await;
        match (write_res.err(), close_res.err()) {
            (None, None) => Ok(()),
            (Some(a), None) => Err(a.to_string()),
            (None, Some(b)) => Err(b.to_string()),
            (Some(a), Some(b)) => Err(format!("{a}; {b}")),
        }
    });

    let stderr_buf = Vec::new();
    let mut stderr_pipe = stderr_pipe;
    let mut stderr_task = tokio::spawn(async move {
        let mut buf = stderr_buf;
        if let Some(s) = stderr_pipe.as_mut() {
            use tokio::io::AsyncReadExt as _;
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });
    let mut stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(s) = stdout.as_mut() {
            use tokio::io::AsyncReadExt as _;
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });

    // Wait for exit or cancellation; on cancellation SIGKILL the whole group.
    let wait_task = tokio::spawn(async move { child.wait().await });
    tokio::pin!(wait_task);

    // Keep going through all joins after cancellation so the writer/readers do
    // not outlive this call with pipes owned by a killed helper.
    let mut cancelled = false;
    let mut lifecycle_error: Option<anyhow::Error> = None;
    let status_result = tokio::select! {
        r = &mut wait_task => r.ok(),
        _ = ctx.cancelled() => {
            cancelled = true;
            #[cfg(unix)]
            if let Err(error) = stop_process_group(pid as i32) {
                lifecycle_error = Some(error);
            }
            #[cfg(windows)]
            if let Err(error) = process_job.terminate() {
                lifecycle_error = Some(error.context("execenv: stop preparation process tree"));
            }
            wait_task.await.ok()
        }
    };

    let (status, status_error) = match status_result {
        Some(Ok(status)) => (Some(status), None),
        Some(Err(error)) => (
            None,
            Some(anyhow!("execenv: preparation helper failed: {error}")),
        ),
        None => (
            None,
            Some(anyhow!("execenv: preparation helper wait task failed")),
        ),
    };
    #[cfg(windows)]
    if let Err(error) = process_job.finish().await {
        if lifecycle_error.is_none() {
            lifecycle_error = Some(error.context("execenv: finish preparation process tree"));
        }
    }

    let write_result = match tokio::time::timeout(PREPARATION_WAIT_DELAY, &mut write_task).await {
        Ok(Ok(result)) => {
            result.map_err(|error| anyhow!("execenv: write preparation request: {error}"))
        }
        Ok(Err(error)) => Err(anyhow!("execenv: join preparation writer: {error}")),
        Err(_) => {
            // A blocked stdin write means the helper (or one of its
            // descendants) is not consuming the request. Kill the whole
            // process boundary before dropping the writer, otherwise a
            // descendant can keep the pipe alive after this function returns.
            #[cfg(unix)]
            if lifecycle_error.is_none() {
                if let Err(error) = stop_process_group(pid as i32) {
                    lifecycle_error = Some(error);
                }
            }
            #[cfg(windows)]
            if lifecycle_error.is_none() {
                if let Err(error) = process_job.terminate() {
                    lifecycle_error = Some(error.context("execenv: stop preparation process tree"));
                }
            }
            write_task.abort();
            let _ = write_task.await;
            Err(anyhow!(
                "execenv: preparation request did not close within {:?}",
                PREPARATION_WAIT_DELAY
            ))
        }
    };

    // Match Go's cmd.WaitDelay: a descendant that inherited stdout/stderr
    // must not keep the daemon blocked forever after the helper exits.
    let output_result = tokio::time::timeout(PREPARATION_WAIT_DELAY, async {
        let (detail, stdout_bytes) = tokio::join!(&mut stderr_task, &mut stdout_task);
        let detail = detail.map_err(|error| anyhow!("execenv: join stderr reader: {error}"))?;
        let stdout_bytes =
            stdout_bytes.map_err(|error| anyhow!("execenv: join stdout reader: {error}"))?;
        Ok::<_, anyhow::Error>((detail, stdout_bytes))
    })
    .await;
    let (detail, stdout_bytes, output_error) = match output_result {
        Ok(Ok((detail, stdout_bytes))) => (detail, stdout_bytes, None),
        Ok(Err(error)) => (Vec::new(), Vec::new(), Some(error)),
        Err(_) => {
            // The helper may have exited while a grandchild inherited one of
            // the stdio handles. Enforce the wait deadline by terminating the
            // complete process tree before aborting the readers.
            #[cfg(unix)]
            if lifecycle_error.is_none() {
                if let Err(error) = stop_process_group(pid as i32) {
                    lifecycle_error = Some(error);
                }
            }
            #[cfg(windows)]
            if lifecycle_error.is_none() {
                if let Err(error) = process_job.terminate() {
                    lifecycle_error = Some(error.context("execenv: stop preparation process tree"));
                }
            }
            stderr_task.abort();
            stdout_task.abort();
            let _ = stderr_task.await;
            let _ = stdout_task.await;
            (
                Vec::new(),
                Vec::new(),
                Some(anyhow!(
                    "execenv: preparation helper output did not close within {:?}",
                    PREPARATION_WAIT_DELAY
                )),
            )
        }
    };

    if let Some(error) = lifecycle_error {
        return Err(error);
    }
    if cancelled || ctx.err().is_some() {
        return Err(anyhow!(ctx.cause().to_string()));
    }
    if let Some(error) = status_error {
        return Err(error);
    }
    if let Some(error) = output_error {
        return Err(error);
    }
    let status = status.expect("status is present when status_error is absent");
    if !status.success() {
        let detail = String::from_utf8_lossy(&detail).trim().to_string();
        if !detail.is_empty() {
            return Err(anyhow!(
                "execenv: preparation helper failed: exit status {}: {}",
                status,
                detail
            ));
        }
        return Err(anyhow!(
            "execenv: preparation helper failed: exit status {}",
            status
        ));
    }
    if let Err(error) = write_result {
        return Err(error);
    }
    if ctx.err().is_some() {
        return Err(anyhow!(ctx.cause().to_string()));
    }

    let response: PreparationResponse =
        serde_json::from_slice(&stdout_bytes).context("execenv: decode preparation response")?;
    if !response.error.is_empty() {
        return Err(rehydrate_preparation_error(
            &response.error,
            &response.error_kind,
        ));
    }
    Ok(response.environment)
}

/// stop_process_group kills the helper and any CLI it spawned. After SIGKILL
/// is pending, a helper blocked in a kernel filesystem call cannot return and
/// perform another write when that call eventually unblocks. ESRCH tolerated.
#[cfg(unix)]
fn stop_process_group(pid: i32) -> anyhow::Result<()> {
    if pid <= 0 {
        return Ok(());
    }
    let rc = unsafe { libc::kill(-pid, libc::SIGKILL) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(anyhow!(err)).context("execenv: kill preparation process group");
    }
    Ok(())
}

#[cfg(windows)]
struct WindowsProcessJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl WindowsProcessJob {
    fn new() -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        };
        if configured == 0 {
            let err = std::io::Error::last_os_error();
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            return Err(err);
        }
        Ok(Self { handle })
    }

    fn attach(&self, pid: u32) -> anyhow::Result<()> {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(std::io::Error::last_os_error()).context("open helper process");
        }
        let assigned = unsafe { AssignProcessToJobObject(self.handle, process) };
        unsafe { windows_sys::Win32::Foundation::CloseHandle(process) };
        if assigned == 0 {
            return Err(std::io::Error::last_os_error()).context("assign helper to job object");
        }
        Ok(())
    }

    fn terminate(&self) -> anyhow::Result<()> {
        let terminated =
            unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.handle, 1) };
        if terminated == 0 {
            let err = std::io::Error::last_os_error();
            if self.active_processes()? > 0 {
                return Err(err).context("terminate helper job object");
            }
        }
        Ok(())
    }

    async fn finish(&self) -> anyhow::Result<()> {
        if self.active_processes()? > 0 {
            self.terminate()?;
        }
        while self.active_processes()? > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    fn active_processes(&self) -> anyhow::Result<u32> {
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };

        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectBasicAccountingInformation,
                std::ptr::addr_of_mut!(info).cast(),
                std::mem::size_of_val(&info) as u32,
                std::ptr::null_mut(),
            )
        };
        if queried == 0 {
            return Err(std::io::Error::last_os_error()).context("query helper job object");
        }
        Ok(info.ActiveProcesses)
    }
}

#[cfg(windows)]
impl Drop for WindowsProcessJob {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// decode_preparation_request reads the parent→helper payload strictly.
pub(crate) fn decode_preparation_request<R: std::io::Read>(
    input: R,
) -> anyhow::Result<PreparationRequest> {
    serde_json::from_reader(input).context("decode preparation request")
}

/// run_preparation_helper serves the private helper protocol on stdin/stdout.
/// Operational errors from prepare/reuse are encoded in the response so the
/// parent can preserve them; malformed protocol input/output is returned as a
/// process error because the parent cannot safely interpret the result.
///
/// Both actions call the real execenv implementations. A reuse cache miss is
/// encoded as a successful response without an environment so the parent can
/// fall back to a fresh prepare, matching the Go contract.
pub async fn run_preparation_helper<I, O>(input: I, output: &mut O) -> anyhow::Result<()>
where
    I: std::io::Read,
    O: std::io::Write,
{
    // The private helper runs before the daemon's normal log subscriber is
    // installed. Keep its diagnostics on stderr (the parent already drains
    // that pipe) so filesystem/git failures are not silently lost.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();

    let request = decode_preparation_request(input)?;

    let mut response = PreparationResponse::default();
    match request.action.as_str() {
        PREPARATION_ACTION_PREPARE => {
            let Some(wire) = request.prepare else {
                bail!("invalid prepare request");
            };
            if request.reuse.is_some() {
                bail!("invalid prepare request");
            }
            match super::execenv::prepare_with_local_worktree_lock(
                wire.params,
                wire.local_worktree_lock_held,
            )
            .await
            {
                Ok(env) => response.environment = Some(env),
                Err(err) => {
                    response.error = format!("{err:#}");
                    response.error_kind = preparation_error_kind(&err).to_string();
                }
            }
        }
        PREPARATION_ACTION_REUSE => {
            let Some(wire) = request.reuse else {
                bail!("invalid reuse request");
            };
            if request.prepare.is_some() {
                bail!("invalid reuse request");
            }
            response.environment = super::execenv::reuse(wire.params);
        }
        other => bail!("unknown preparation action {other:?}"),
    }

    serde_json::to_writer(&mut *output, &response)
        .map_err(|e| anyhow!("encode preparation response: {e}"))?;
    writeln!(output).context("encode preparation response")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::execenv::{OpenclawGatewayPin, ReuseParams};
    use super::*;

    fn sample_prepare_params() -> PrepareParams {
        PrepareParams {
            workspaces_root: "/tmp/ws".into(),
            workspace_id: "ws1".into(),
            task_id: "01a01ec0-e69d-7000-8000-0123456789ab".into(),
            ..Default::default()
        }
    }

    fn sample_reuse_params(work_dir: impl Into<String>) -> ReuseParams {
        ReuseParams {
            workspaces_root: "/tmp/ws".into(),
            work_dir: work_dir.into(),
            provider: "codex".into(),
            openclaw_gateway: OpenclawGatewayPin {
                host: "gw".into(),
                port: 7420,
                token: "reuse-secret".into(),
                tls: true,
            },
            ..ReuseParams::default()
        }
    }

    // Port of TestDecodePreparationRequestRejectsUnknownFields.
    #[test]
    fn test_decode_rejects_unknown_fields() {
        let good = serde_json::json!({
            "action": "prepare",
            "prepare": {"params": sample_prepare_params()},
        });
        let _ = good; // shape exercised through full round-trip below

        let bad = br#"{"action":"prepare","bogus":1}"#;
        assert!(serde_json::from_slice::<PreparationRequest>(bad).is_err());
    }

    // Round-trip: encode → decode preserves action and payloads.
    #[test]
    fn test_request_round_trip() {
        let req = PreparationRequest {
            action: PREPARATION_ACTION_PREPARE.into(),
            prepare: Some(PreparationWireParams {
                params: PrepareParams {
                    openclaw_gateway: OpenclawGatewayPin {
                        host: "gw".into(),
                        port: 7420,
                        token: "sekrit".into(),
                        tls: true,
                    },
                    ..sample_prepare_params()
                },
                local_worktree_lock_held: false,
            }),
            reuse: None,
        };
        let bytes = serde_json::to_vec(&req).unwrap();
        let back: PreparationRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.action, "prepare");
        let gw = back.prepare.unwrap().params.openclaw_gateway;
        assert_eq!(
            gw.token, "sekrit",
            "the trusted boundary carries the real token"
        );
    }

    #[test]
    fn reuse_request_round_trip_preserves_real_params() {
        let req = PreparationRequest {
            action: PREPARATION_ACTION_REUSE.into(),
            prepare: None,
            reuse: Some(ReuseWireParams {
                params: sample_reuse_params("/tmp/ws/task/worktree"),
            }),
        };

        let bytes = serde_json::to_vec(&req).unwrap();
        let back: PreparationRequest = serde_json::from_slice(&bytes).unwrap();
        let params = back.reuse.unwrap().params;
        assert_eq!(params.work_dir, "/tmp/ws/task/worktree");
        assert_eq!(params.provider, "codex");
        assert_eq!(params.openclaw_gateway.token, "reuse-secret");
    }

    #[tokio::test]
    async fn reuse_helper_preserves_cache_miss_as_successful_none() {
        let missing = tempfile::tempdir()
            .unwrap()
            .path()
            .join("missing-workdir")
            .to_string_lossy()
            .into_owned();
        let request = PreparationRequest {
            action: PREPARATION_ACTION_REUSE.into(),
            prepare: None,
            reuse: Some(ReuseWireParams {
                params: sample_reuse_params(missing),
            }),
        };
        let input = serde_json::to_vec(&request).unwrap();
        let mut output = Vec::new();

        run_preparation_helper(input.as_slice(), &mut output)
            .await
            .unwrap();

        let response: PreparationResponse = serde_json::from_slice(&output).unwrap();
        assert!(response.environment.is_none());
        assert!(response.error.is_empty());
        assert!(response.error_kind.is_empty());
    }

    #[tokio::test]
    async fn reuse_helper_returns_the_real_reused_environment() {
        let root = tempfile::tempdir().unwrap();
        let work_dir = root.path().join("task-root").join("worktree");
        std::fs::create_dir_all(&work_dir).unwrap();
        let mut params = sample_reuse_params(work_dir.to_string_lossy());
        params.workspaces_root = root.path().to_string_lossy().into_owned();
        params.provider = "pi".into();
        let request = PreparationRequest {
            action: PREPARATION_ACTION_REUSE.into(),
            prepare: None,
            reuse: Some(ReuseWireParams { params }),
        };
        let input = serde_json::to_vec(&request).unwrap();
        let mut output = Vec::new();

        run_preparation_helper(input.as_slice(), &mut output)
            .await
            .unwrap();

        let response: PreparationResponse = serde_json::from_slice(&output).unwrap();
        let environment = response.environment.unwrap();
        assert_eq!(environment.work_dir, work_dir.to_string_lossy().as_ref());
        assert!(response.error.is_empty());
    }

    // Port of TestPreparationErrorKindRoundTrip.
    #[test]
    fn test_error_kind_round_trip() {
        let plain: anyhow::Error = anyhow!("something else");
        assert_eq!(preparation_error_kind(&plain), "");

        let timeout: anyhow::Error = ErrOpenclawCliTimeout.into();
        assert_eq!(
            preparation_error_kind(&timeout),
            PREPARATION_ERROR_KIND_OPENCLAW_CLI_TIMEOUT
        );

        let back = rehydrate_preparation_error("boom", PREPARATION_ERROR_KIND_OPENCLAW_CLI_TIMEOUT);
        assert!(format!("{back:#}").contains("boom"), "message preserved");

        let degraded = rehydrate_preparation_error("mystery", "future-kind");
        assert_eq!(format!("{degraded}"), "mystery");
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn repository_lock_is_exclusive_until_the_owner_drops() {
        let root = tempfile::tempdir().unwrap();
        let git_root = root.path().to_string_lossy().into_owned();
        let first = super::super::local_worktree::RepositoryLock::acquire(&git_root, None)
            .await
            .unwrap();
        let path = super::super::local_worktree::repository_lock_path(&git_root);
        let second = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();

        assert!(!super::super::local_worktree::try_lock_file(&second).unwrap());
        drop(first);
        assert!(super::super::local_worktree::try_lock_file(&second).unwrap());
        super::super::local_worktree::unlock_file(&second);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preparation_accepts_a_non_utf8_executable_path() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory
            .path()
            .join(OsString::from_vec(b"helper-\xff".to_vec()));
        std::fs::write(
            &executable,
            b"#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"environment\":null}'\n",
        )
        .unwrap();
        std::fs::set_permissions(
            &executable,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();

        let result = run_preparation_process(
            &crate::repocache::Ctx::new(),
            &[executable.into_os_string()],
            PreparationRequest {
                action: PREPARATION_ACTION_REUSE.to_string(),
                prepare: None,
                reuse: Some(ReuseWireParams {
                    params: sample_reuse_params("/missing"),
                }),
            },
        )
        .await
        .unwrap();
        assert!(result.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_wait_deadline_terminates_descendants_holding_helper_pipes() {
        let result = run_preparation_process(
            &crate::repocache::Ctx::new(),
            &[
                OsString::from("sh"),
                OsString::from("-c"),
                OsString::from("cat >/dev/null; sleep 30 & kill -TERM $$"),
            ],
            PreparationRequest {
                action: PREPARATION_ACTION_REUSE.to_string(),
                prepare: None,
                reuse: Some(ReuseWireParams {
                    params: sample_reuse_params("/missing"),
                }),
            },
        )
        .await
        .unwrap_err();
        assert!(result
            .to_string()
            .contains("preparation helper output did not close"));
    }
}
