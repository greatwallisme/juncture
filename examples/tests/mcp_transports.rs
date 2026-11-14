//! Integration tests for `examples/src/23_mcp_transports.rs`.
//!
//! The example is a binary, so every test drives the real executable rather than
//! calling library functions: that is the only way to cover what the example
//! exists to demonstrate, namely a spawned stdio server child and a live
//! Streamable HTTP endpoint answering with `text/event-stream`.
//!
//! Two modes are exercised:
//! - `--stdio-server`: script JSON-RPC requests on stdin, close stdin, and
//!   assert the exact response lines, order included.
//! - no arguments: run the full demo and assert both transports discovered and
//!   executed tools, in a canonical order that is stable across runs.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Bound for one child process. A demo that hangs would otherwise stall the
/// whole test binary instead of naming the failure.
const CHILD_DEADLINE: Duration = Duration::from_secs(60);

/// Path to the example binary this test suite drives.
const fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_23_mcp_transports")
}

/// Spawn the example, script `input` on stdin, and collect its output.
///
/// stdout and stderr are drained on their own threads. Waiting first and reading
/// afterwards would deadlock as soon as a child filled a pipe buffer, which a
/// chatty failure message is enough to do.
fn run(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(binary())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the example binary");

    let stdout = drain(child.stdout.take().expect("piped stdout"));
    let stderr = drain(child.stderr.take().expect("piped stderr"));

    // Closing stdin is what makes the scripted stdio sessions deterministic: the
    // child sees end-of-input, answers every request, and exits.
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin.write_all(input).expect("write scripted stdin");
    drop(stdin);

    let status = wait_with_deadline(&mut child);

    Output {
        status,
        stdout: stdout.join().expect("drained child stdout"),
        stderr: stderr.join().expect("drained child stderr"),
    }
}

/// Read one pipe to EOF on a background thread.
fn drain(mut pipe: impl std::io::Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut pipe, &mut bytes).expect("read child output");
        bytes
    })
}

/// Poll the child until it exits, killing it if it overruns the deadline.
///
/// Polling rather than blocking in `wait()` keeps a wedged child from hanging
/// the test run.
fn wait_with_deadline(child: &mut Child) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll the example binary") {
            return status;
        }
        assert!(
            start.elapsed() <= CHILD_DEADLINE,
            "example binary did not exit within {CHILD_DEADLINE:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Human-readable form of an exit status for assertion messages.
fn display_status(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "a signal".to_string(), |code| format!("code {code}"))
}

/// Run a scripted stdio-server session and return its stdout lines.
fn run_stdio_script(script: &str) -> Vec<String> {
    let output = run(&["--stdio-server"], script.as_bytes());
    assert!(
        output.status.success(),
        "stdio server exited with {}: {}",
        display_status(output.status),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    stdout.lines().map(str::to_string).collect()
}

/// Parse one response line into a JSON value.
fn parse_line(line: &str) -> serde_json::Value {
    serde_json::from_str(line).expect("response line must be JSON")
}

/// The `result` object of a successful response line.
fn result_of(line: &str) -> serde_json::Value {
    let value = parse_line(line);
    assert!(
        value.get("error").is_none(),
        "expected a result, got an error: {line}"
    );
    assert!(
        value.get("result").is_some(),
        "response has no result: {line}"
    );
    value["result"].clone()
}

/// The error code of a failing response line.
fn error_code_of(line: &str) -> i64 {
    let value = parse_line(line);
    assert!(
        value.get("error").is_some(),
        "response has no error object: {line}"
    );
    value["error"]["code"]
        .as_i64()
        .expect("error code must be an integer")
}

// ---------------------------------------------------------------------------
// stdio server mode
// ---------------------------------------------------------------------------

#[test]
fn stdio_discover_advertises_the_modern_protocol() {
    let lines = run_stdio_script(
        r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}
"#,
    );
    assert_eq!(lines.len(), 1, "one request must produce exactly one line");

    let result = result_of(&lines[0]);
    assert!(
        result["supportedVersions"]
            .as_array()
            .is_some_and(|versions| versions.iter().any(|version| version == "2026-07-28")),
        "the server must advertise the modern protocol: {result}"
    );
    assert_eq!(result["serverInfo"]["name"], "stdio-mock");
    assert_eq!(result["cacheScope"], "public");
    assert_eq!(result["ttlMs"], 60_000);
}

#[test]
fn stdio_tools_list_publishes_both_tools() {
    let lines = run_stdio_script(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
"#,
    );
    let result = result_of(&lines[0]);
    let names: Vec<&str> = result["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["echo", "add"],
        "the catalog must stay stable: it is the tool table the demo aggregates"
    );
}

#[test]
fn stdio_echo_and_add_return_result_frames() {
    let lines = run_stdio_script(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo","arguments":{"message":"hi"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add","arguments":{"a":2,"b":3}}}
"#,
    );
    assert_eq!(lines.len(), 2);

    let echo = result_of(&lines[0]);
    assert_eq!(echo["resultType"], "complete");
    assert_eq!(echo["result"]["isError"], false);
    assert_eq!(echo["result"]["content"][0]["text"], "stdio echoed: hi");

    let add = result_of(&lines[1]);
    assert_eq!(
        add["result"]["content"][0]["text"],
        "stdio computed: 2 + 3 = 5"
    );
}

#[test]
fn stdio_notifications_are_never_answered() {
    // A notification carries no id and expects no reply. If the server answered
    // it, the assertion below would see two lines and every later response would
    // be attributed to the wrong call.
    let lines = run_stdio_script(
        r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{}}
{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}
"#,
    );
    assert_eq!(
        lines.len(),
        1,
        "a notification must not produce a response line: {lines:?}"
    );
    assert_eq!(parse_line(&lines[0])["id"], 7);
}

#[test]
fn stdio_blank_lines_are_ignored() {
    let lines = run_stdio_script(
        "\n   \n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\",\"params\":{}}\n\n",
    );
    assert_eq!(
        lines.len(),
        1,
        "blank lines must not be answered: {lines:?}"
    );
    assert_eq!(parse_line(&lines[0])["id"], 3);
}

#[test]
fn stdio_unknown_method_reports_method_not_found() {
    let lines = run_stdio_script(
        r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list","params":{}}
"#,
    );
    assert_eq!(error_code_of(&lines[0]), -32601);
}

#[test]
fn stdio_malformed_line_reports_a_parse_error_with_a_null_id() {
    let lines = run_stdio_script("this is not json\n");
    assert_eq!(
        error_code_of(&lines[0]),
        -32700,
        "a line that cannot be parsed must be reported, not silently dropped"
    );
    assert!(
        parse_line(&lines[0])["id"].is_null(),
        "an unparsable request has no id to echo"
    );
}

#[test]
fn stdio_missing_or_unknown_tool_arguments_are_rejected() {
    let lines = run_stdio_script(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"add","arguments":{"a":1}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope","arguments":{}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"arguments":{}}}
"#,
    );
    assert_eq!(lines.len(), 4);
    for line in &lines {
        assert_eq!(
            error_code_of(line),
            -32602,
            "invalid parameters must be rejected: {line}"
        );
    }
}

#[test]
fn stdio_integer_overflow_is_reported_not_wrapped() {
    // Wrapping would silently return a wrong sum to the model.
    let lines = run_stdio_script(&format!(
        "{}\n",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "add", "arguments": {"a": i64::MAX, "b": 1}}
        })
    ));
    assert_eq!(error_code_of(&lines[0]), -32602);
    assert!(
        lines[0].contains("integer overflow"),
        "the failure must name its cause: {}",
        lines[0]
    );
}

// ---------------------------------------------------------------------------
// Full demo: both transports
// ---------------------------------------------------------------------------

/// Run the demo with no arguments and return its stdout.
fn run_demo() -> String {
    let output = run(&[], b"");
    assert!(
        output.status.success(),
        "demo exited with {}: {}",
        display_status(output.status),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("demo stdout is UTF-8")
}

/// The canonical wire-order block of the demo output, in printed order.
fn canonical_order(stdout: &str) -> Vec<String> {
    let marker = "MCP tools in canonical wire order";
    let start = stdout
        .find(marker)
        .expect("demo output must contain the tool-order block");
    stdout[start..]
        .lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .map(|line| {
            line.trim().split_once(". ").map_or_else(
                || line.trim().to_string(),
                |(_, name)| name.trim().to_string(),
            )
        })
        .collect()
}

#[test]
fn demo_exercises_both_transports() {
    let stdout = run_demo();

    assert!(stdout.contains("[stdio] subprocess transport"), "{stdout}");
    assert!(
        stdout.contains("[sse] Streamable HTTP transport"),
        "{stdout}"
    );
    // Two servers, two tools each.
    assert_eq!(
        stdout.matches("tools    : 2").count(),
        2,
        "both transports must discover two tools:\n{stdout}"
    );
    // Each transport answered its own call.
    assert!(
        stdout.contains("stdio computed: 4 + 5 = 9"),
        "the stdio server must have executed the add call:\n{stdout}"
    );
    assert!(
        stdout.contains("sse echoed: hello over text/event-stream"),
        "the SSE server must have executed the echo call:\n{stdout}"
    );
    assert!(
        stdout.contains("Both transports exercised"),
        "the demo must reach its shutdown line:\n{stdout}"
    );
}

#[test]
fn demo_tool_order_is_canonical_and_disambiguated() {
    let names = canonical_order(&run_demo());

    assert_eq!(
        names,
        vec![
            "sse__add".to_string(),
            "sse__echo".to_string(),
            "stdio__add".to_string(),
            "stdio__echo".to_string(),
        ],
        "tools must be ordered by (server_alias, tool_name) and carry the alias prefix"
    );

    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "the printed order must be the canonical order, not discovery order"
    );

    // Both servers publish the same tool names; without the alias prefix the
    // aggregate toolset would contain duplicate wire names and one tool would
    // shadow the other.
    let unique: BTreeSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "wire names must be unique");
}

#[test]
fn demo_output_is_stable_across_runs() {
    // Discovery order and server-side JSON key order must not leak into output;
    // this section is the prompt-cache prefix segment S1, so any churn here is a
    // silent cache miss in production.
    let first = canonical_order(&run_demo());
    let second = canonical_order(&run_demo());
    assert_eq!(
        first, second,
        "two runs must produce the same canonical tool order"
    );
}
