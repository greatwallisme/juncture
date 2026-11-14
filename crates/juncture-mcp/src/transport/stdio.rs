//! Subprocess stdio transport: newline-delimited JSON-RPC over a child's pipes.
//!
//! Two details matter operationally (design `12-mcp-and-prefix-cache` §1.6):
//!
//! - The child's stderr is pumped into `tracing` on a dedicated task. An
//!   undrained pipe fills and blocks the child, which looks like a hang in the
//!   caller.
//! - The child is killed on drop, so a dropped client cannot leak an orphan
//!   server process.

use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::McpTransport;
use crate::protocol::{JsonRpcRequest, JsonRpcResponse, McpError};

/// How long one request may wait for its response.
///
/// A stdio server is a local subprocess, so a response this slow means the
/// server is wedged. The bound exists so a wedged server fails the call instead
/// of holding the transport lock forever.
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Spawn configuration for a stdio MCP server.
#[derive(Clone, Debug)]
pub struct StdioConfig {
    /// Executable to run.
    pub program: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
}

impl StdioConfig {
    /// Create a config for the given program with no arguments.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Add one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add one environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// Live subprocess plus its request plumbing.
struct Inner {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // `kill_on_drop` covers the graceful path; starting the kill here makes
        // the intent explicit and also covers a panic unwind, where `Child`'s
        // own drop is the last line of defense.
        let _ = self.child.start_kill();
    }
}

/// stdio transport to an MCP server child process.
pub struct StdioTransport {
    inner: Mutex<Inner>,
    /// Kept so the stderr pump task is aborted when the transport drops.
    stderr_task: Option<JoinHandle<()>>,
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport").finish_non_exhaustive()
    }
}

impl StdioTransport {
    /// Spawn the server and take ownership of its pipes.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Transport`] when the process cannot be started or its
    /// standard streams are unavailable.
    pub fn spawn(config: &StdioConfig) -> Result<Self, McpError> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Put the child in its own process group so a kill reaches everything it
        // spawned. Killing only the direct child leaves grandchildren holding the
        // pipes, which is the usual way a stdio MCP server leaks after shutdown
        // (design 12 §1.6).
        #[cfg(unix)]
        command.process_group(0);
        for (key, value) in &config.env {
            command.env(key, value);
        }

        let mut child = command.spawn().map_err(|error| {
            McpError::Transport(format!("failed to spawn {}: {error}", config.program))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("child stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("child stdout unavailable".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpError::Transport("child stderr unavailable".to_string()))?;

        // Drain stderr or the child blocks once the pipe buffer fills.
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "juncture_mcp::stdio", "{line}");
            }
        });

        Ok(Self {
            inner: Mutex::new(Inner {
                child,
                stdin,
                stdout: BufReader::new(stdout),
            }),
            stderr_task: Some(stderr_task),
        })
    }

    /// Abort the child process group.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Transport`] if the kill cannot be requested.
    pub async fn shutdown(&self) -> Result<(), McpError> {
        // Hold the lock only for the kill; the group signal needs nothing from
        // the guard.
        let pid = {
            let mut inner = self.inner.lock().await;
            let pid = inner.child.id();
            inner
                .child
                .start_kill()
                .map_err(|error| McpError::Transport(format!("kill failed: {error}")))?;
            pid
        };
        Self::kill_process_group(pid);
        Ok(())
    }

    /// Signal the child's process group so grandchildren die too.
    ///
    /// The child was spawned with `process_group(0)`, so its pid is also its
    /// process-group id. Best-effort: a group that is already gone makes the
    /// signal a no-op, and there is nothing an operator could do about it.
    #[cfg(unix)]
    fn kill_process_group(pid: Option<u32>) {
        let Some(pid) = pid else {
            return;
        };
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return;
        };
        // SAFETY: `killpg` takes a process-group id and a signal number; a
        // negative argument would address a different group, and the conversion
        // above rejects anything outside `pid_t`. Failure is ignored on purpose:
        // the group is usually already dead by the time this runs.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }

    /// No process groups on non-Unix targets.
    #[cfg(not(unix))]
    fn kill_process_group(_pid: Option<u32>) {}

    /// Process id of the child, if still running.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.inner
            .try_lock()
            .ok()
            .and_then(|inner| inner.child.id())
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        // stdio is strictly sequential: one outstanding request per server, so
        // hold the lock across write+read to keep responses correlated.
        let mut inner = self.inner.lock().await;

        let mut line = serde_json::to_string(&request)
            .map_err(|error| McpError::Transport(format!("encode request: {error}")))?;
        line.push('\n');

        inner
            .stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| McpError::Transport(format!("write failed: {error}")))?;
        inner
            .stdin
            .flush()
            .await
            .map_err(|error| McpError::Transport(format!("flush failed: {error}")))?;

        loop {
            let mut response_line = String::new();
            // A server that accepts a request and never answers would otherwise
            // hold this transport's mutex forever, stalling every later call
            // with no diagnostic. The timeout bounds that wait and names the
            // method, so a hung server is distinguishable from a slow one.
            let read =
                tokio::time::timeout(REQUEST_TIMEOUT, inner.stdout.read_line(&mut response_line))
                    .await
                    .map_err(|_elapsed| {
                        McpError::Transport(format!(
                            "server did not answer {} within {REQUEST_TIMEOUT:?}",
                            request.method
                        ))
                    })?
                    .map_err(|error| McpError::Transport(format!("read failed: {error}")))?;

            if read == 0 {
                return Err(McpError::Transport(
                    "server closed stdout before responding".to_string(),
                ));
            }

            let trimmed = response_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // A notification (no id) may be interleaved with the response.
            let parsed: serde_json::Value = serde_json::from_str(trimmed)
                .map_err(|error| McpError::Transport(format!("decode response: {error}")))?;
            if parsed.get("id").is_none() {
                tracing::debug!(target: "juncture_mcp::stdio", "dropping notification mid-request");
                continue;
            }

            let response: JsonRpcResponse = serde_json::from_value(parsed)
                .map_err(|error| McpError::Transport(format!("decode response: {error}")))?;

            // Correlate by id. Without this, a stale response left over from a
            // timed-out request would be returned as the answer to the next
            // one, silently misattributing results now that a read can time out.
            if response.id != request.id {
                tracing::warn!(
                    target: "juncture_mcp::stdio",
                    expected = %request.id,
                    got = %response.id,
                    "discarding response with a mismatched id"
                );
                continue;
            }

            drop(inner);
            return Ok(response);
        }
    }

    fn kind(&self) -> &'static str {
        "stdio"
    }
}

/// Convenience: share a transport across tool instances.
pub type SharedTransport = Arc<dyn McpTransport>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let config = StdioConfig::new("uvx")
            .arg("mcp-server-fs")
            .env("KEY", "value");
        assert_eq!(config.program, "uvx");
        assert_eq!(config.args, vec!["mcp-server-fs"]);
        assert_eq!(config.env, vec![("KEY".to_string(), "value".to_string())]);
    }

    #[test]
    fn test_spawn_nonexistent_program_errors() {
        let config = StdioConfig::new("/definitely/not/a/real/binary");
        let error = StdioTransport::spawn(&config).expect_err("spawn must fail");
        assert!(matches!(error, McpError::Transport(_)));
    }

    #[tokio::test]
    async fn test_round_trip_with_echo_server() {
        // A minimal server: echo a fixed result for any request id. Uses the
        // system shell so the test does not need a real MCP server.
        let config = StdioConfig::new("sh").arg("-c").arg(
            r#"while IFS= read -r line; do id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p'); printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete","result":{"ok":true}}}\n' "$id"; done"#,
        );
        let transport = StdioTransport::spawn(&config).expect("spawn echo server");

        let request = JsonRpcRequest::with_meta(serde_json::json!(1), "server/discover", None);
        let response = transport.request(request).await.expect("response");
        let result = response.result.expect("result present");
        assert_eq!(result["resultType"], "complete");
    }

    #[tokio::test]
    async fn test_response_with_foreign_id_is_not_returned() {
        // A server that answers with a different id must not have its answer
        // attributed to this request: after a timeout, the late reply to the
        // previous request would otherwise be returned as the next answer.
        let config = StdioConfig::new("sh").arg("-c").arg(
            r#"while IFS= read -r line; do printf '{"jsonrpc":"2.0","id":999,"result":{"resultType":"complete","result":{"stale":true}}}\n'; printf '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","result":{"fresh":true}}}\n'; done"#,
        );
        let transport = StdioTransport::spawn(&config).expect("spawn");

        let request = JsonRpcRequest::with_meta(serde_json::json!(1), "tools/list", None);
        let response = transport.request(request).await.expect("response");
        assert_eq!(response.id, serde_json::json!(1));
        assert_eq!(response.result.expect("result")["result"]["fresh"], true);
    }

    #[tokio::test]
    async fn test_stderr_does_not_deadlock() {
        // Emit far more stderr than a pipe buffer holds before answering. If
        // stderr were not drained, the write would block forever.
        let config = StdioConfig::new("sh").arg("-c").arg(
            r#"for i in $(seq 1 5000); do printf 'noise line %s\n' "$i" >&2; done
               while IFS= read -r line; do id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p'); printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete"}}\n' "$id"; done"#,
        );
        let transport = StdioTransport::spawn(&config).expect("spawn");

        let request = JsonRpcRequest::with_meta(serde_json::json!(1), "tools/list", None);
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            transport.request(request),
        )
        .await
        .expect("stderr pump must prevent deadlock")
        .expect("response");
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_early_exit_reports_transport_error() {
        let config = StdioConfig::new("sh").arg("-c").arg("exit 0");
        let transport = StdioTransport::spawn(&config).expect("spawn");
        let request = JsonRpcRequest::with_meta(serde_json::json!(1), "tools/list", None);
        let result = transport.request(request).await;
        assert!(matches!(result, Err(McpError::Transport(_))));
    }

    #[tokio::test]
    async fn test_drop_kills_child() {
        let config = StdioConfig::new("sh").arg("-c").arg("sleep 300");
        let transport = StdioTransport::spawn(&config).expect("spawn");
        let pid = transport.pid().expect("pid available");
        drop(transport);

        // Give the kernel a moment to reap the killed process.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // A live child still owns /proc/<pid>; a killed one does not.
        let alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
        assert!(!alive, "dropping the transport must not leak the child");
    }
}
