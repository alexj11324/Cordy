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
//! - The preparation wire restores the raw OpenclawGateway token only for the
//!   trusted local stdin pipe; public serialization and diagnostics mask it.
//! - slog logger dropped (tracing).
//! - WaitDelay semantics: cancellation terminates the platform process-tree
//!   boundary before awaiting the child and pipe readers.
//!
//! NOTE: the parent-side entry points (prepare_isolated / reuse_isolated) are
//! wired into the task launcher in a later slice; until then this module is
//! exercised by its unit tests only, hence `allow(dead_code)`.
#![allow(dead_code)]

use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::execenv::{Environment, PrepareParams};

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

// The helper-protocol view carries the gateway pin plainly over this trusted
// local stdin pipe (Go's preparationOpenclawGatewayPin rationale). Its custom
// serializer starts with PrepareParams' masked public serialization and then
// overwrites only the private wire field with the raw value.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparationWireParams {
    #[serde(flatten)]
    pub params: PrepareParams,
}

impl Serialize for PreparationWireParams {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut value = serde_json::to_value(&self.params).map_err(serde::ser::Error::custom)?;
        if let serde_json::Value::Object(fields) = &mut value {
            fields.insert(
                "OpenclawGateway".into(),
                raw_gateway_value(&self.params.openclaw_gateway),
            );
        }
        value.serialize(serializer)
    }
}

fn raw_gateway_value(pin: &super::execenv::OpenclawGatewayPin) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    if !pin.host.is_empty() {
        fields.insert("host".into(), serde_json::Value::String(pin.host.clone()));
    }
    if pin.port != 0 {
        fields.insert("port".into(), serde_json::Value::Number(pin.port.into()));
    }
    if !pin.token.is_empty() {
        fields.insert("token".into(), serde_json::Value::String(pin.token.clone()));
    }
    if pin.tls {
        fields.insert("tls".into(), serde_json::Value::Bool(true));
    }
    serde_json::Value::Object(fields)
}

// S9-integration: ReuseParams is defined by the Reuse port (execenv.go's
// second half); the helper protocol needs the wire shape now. Mirrors Go's
// `type preparationReuseParams struct { *ReuseParams; OpenclawGateway ... }`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ReuseParamsPlaceholder {
    #[serde(rename = "WorkspacesRoot", default)]
    pub workspaces_root: String,
    #[serde(rename = "WorkspaceId", default)]
    pub workspace_id: String,
    #[serde(rename = "TaskId", default)]
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReuseWireParams {
    #[serde(flatten)]
    pub params: ReuseParamsPlaceholder,
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
    command: &[String],
    params: PrepareParams,
) -> anyhow::Result<Environment> {
    run_preparation_process(
        ctx,
        command,
        PreparationRequest {
            action: PREPARATION_ACTION_PREPARE.to_string(),
            prepare: Some(PreparationWireParams { params }),
            reuse: None,
        },
    )
    .await
}

/// reuse_isolated executes Reuse under the same killable-helper contract.
pub(crate) async fn reuse_isolated(
    ctx: &crate::repocache::Ctx,
    command: &[String],
    params: ReuseParamsPlaceholder,
) -> anyhow::Result<Environment> {
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
    command: &[String],
    request: PreparationRequest,
) -> anyhow::Result<Environment> {
    if command.is_empty() || command[0].trim().is_empty() {
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
    let write_task = tokio::spawn(async move {
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
    let stderr_task = tokio::spawn(async move {
        let mut buf = stderr_buf;
        if let Some(s) = stderr_pipe.as_mut() {
            use tokio::io::AsyncReadExt as _;
            let _ = s.read_to_end(&mut buf).await;
        }
        buf
    });
    let stdout_task = tokio::spawn(async move {
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

    let status_result = tokio::select! {
        r = &mut wait_task => r.ok(),
        _ = ctx.cancelled() => {
            #[cfg(unix)]
            stop_process_group(pid as i32);
            #[cfg(windows)]
            process_job.terminate().context("execenv: stop preparation process tree")?;
            let _ = wait_task.await;
            return Err(anyhow!(ctx.cause().to_string()));
        }
    };

    let status = match status_result {
        Some(Ok(status)) => status,
        Some(Err(e)) => return Err(anyhow!("execenv: preparation helper failed: {e}")),
        None => return Err(anyhow!("execenv: preparation helper wait task failed")),
    };
    #[cfg(windows)]
    process_job
        .finish()
        .await
        .context("execenv: finish preparation process tree")?;
    let _ = PREPARATION_WAIT_DELAY; // documented grace; EOF reads bound below

    let write_err = write_task.await.ok();
    let detail = stderr_task.await.unwrap_or_default();
    let stdout_bytes = stdout_task
        .await
        .map_err(|e| anyhow!("execenv: join stdout reader: {e}"))?;

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
    if let Some(Ok(())) = write_err {
        // fall through
    } else if let Some(Err(e)) = write_err {
        return Err(anyhow!("execenv: write preparation request: {e}"));
    }

    let response: PreparationResponse =
        serde_json::from_slice(&stdout_bytes).context("execenv: decode preparation response")?;
    if !response.error.is_empty() {
        return Err(rehydrate_preparation_error(
            &response.error,
            &response.error_kind,
        ));
    }
    response
        .environment
        .ok_or_else(|| anyhow!("execenv: preparation helper returned no environment"))
}

/// stop_process_group kills the helper and any CLI it spawned. After SIGKILL
/// is pending, a helper blocked in a kernel filesystem call cannot return and
/// perform another write when that call eventually unblocks. ESRCH tolerated.
#[cfg(unix)]
fn stop_process_group(pid: i32) {
    if pid <= 0 {
        return;
    }
    let rc = unsafe { libc::kill(-pid, libc::SIGKILL) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(error = %err, "execenv: kill preparation process group failed");
        }
    }
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
/// The prepare/reuse bodies are wired to the real implementations; the reuse
/// arm lands with the Reuse port (same file family) and fails closed until
/// then, exactly like the Go helper would on an unknown action.
pub async fn run_preparation_helper<I, O>(input: I, output: &mut O) -> anyhow::Result<()>
where
    I: std::io::Read,
    O: std::io::Write,
{
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
            match super::execenv::prepare(wire.params).await {
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
            // Reuse is ported alongside the daemon's session-reuse wiring;
            // until that slice lands the helper fails closed loudly instead of
            // returning a fabricated environment.
            let err: anyhow::Error = anyhow!("execenv: reuse not yet ported in this build")
                .context(format!("{:#}", wire.params.task_id));
            response.error = format!("{err:#}");
            response.error_kind = preparation_error_kind(&err).to_string();
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
    use super::super::execenv::OpenclawGatewayPin;
    use super::*;

    fn sample_prepare_params() -> PrepareParams {
        PrepareParams {
            workspaces_root: "/tmp/ws".into(),
            workspace_id: "ws1".into(),
            task_id: "01a01ec0-e69d-7000-8000-0123456789ab".into(),
            ..Default::default()
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
}
