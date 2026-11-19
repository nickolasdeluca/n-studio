//! The agent's MCP server: JSON-RPC over HTTP on `127.0.0.1`, as the
//! Model Context Protocol's streamable HTTP transport has it, without the
//! streams: every request is answered with one JSON reply, which the
//! transport allows. Only a request with the running agent's token is
//! heard.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tiny_http::{Header, Method, Response, StatusCode};

use super::{tools, Agents};
use crate::error::{AppError, AppResult};

/// The versions of the protocol this server speaks, newest first
const VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The biggest request read: a tool's arguments are a query at most
const MAX_BODY: u64 = 4 * 1024 * 1024;

pub struct Server {
    port: u16,
    // Kept so the server lives as long as the app
    _http: Arc<tiny_http::Server>,
}

/// What a request needs of the app: whose token is good, and the tools
pub(super) trait Host: Send + Sync + 'static {
    fn admits(&self, token: &str) -> bool;
    fn call(&self, name: &str, arguments: &Value) -> Result<String, String>;
}

struct App(AppHandle);

impl Host for App {
    fn admits(&self, token: &str) -> bool {
        self.0.state::<Agents>().admits(token)
    }

    fn call(&self, name: &str, arguments: &Value) -> Result<String, String> {
        tools::call(&self.0, name, arguments)
    }
}

impl Server {
    pub fn start(app: AppHandle) -> AppResult<Self> {
        Self::listen(Arc::new(App(app)))
    }

    pub(super) fn listen(host: Arc<dyn Host>) -> AppResult<Self> {
        let http = tiny_http::Server::http("127.0.0.1:0")
            .map_err(|e| AppError::Connection { message: format!("The agent's server could not start: {e}") })?;
        let port = http
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| AppError::Connection { message: "The agent's server has no port.".into() })?;
        let http = Arc::new(http);
        let listening = http.clone();
        std::thread::Builder::new()
            .name("agent-mcp".into())
            .spawn(move || {
                for request in listening.incoming_requests() {
                    let host = host.clone();
                    // A tool can wait on the user for minutes; the others
                    // are not held behind it
                    let _ = std::thread::Builder::new().name("agent-mcp-request".into()).spawn(move || {
                        serve(host.as_ref(), request);
                    });
                }
            })
            .map_err(|e| AppError::Connection { message: format!("The agent's server could not start: {e}") })?;
        Ok(Self { port, _http: http })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

fn status(code: u16) -> Response<std::io::Empty> {
    Response::empty(StatusCode(code))
}

fn serve(host: &dyn Host, mut request: tiny_http::Request) {
    let path = request.url().split('?').next().unwrap_or("");
    if path != "/mcp" {
        let _ = request.respond(status(404));
        return;
    }
    let token = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
        .map(str::to_owned);
    if !token.is_some_and(|t| host.admits(&t)) {
        let _ = request.respond(status(401));
        return;
    }
    match request.method() {
        Method::Post => {}
        // No stream of the server's own to listen to, and no session to end
        Method::Get => {
            let _ = request.respond(status(405).with_header(header("Allow", "POST")));
            return;
        }
        Method::Delete => {
            let _ = request.respond(status(200));
            return;
        }
        _ => {
            let _ = request.respond(status(405).with_header(header("Allow", "POST")));
            return;
        }
    }

    let mut body = String::new();
    if request.as_reader().take(MAX_BODY).read_to_string(&mut body).is_err() {
        let _ = request.respond(status(400));
        return;
    }
    match handle(&body, |name, arguments| host.call(name, arguments)) {
        Some(reply) => {
            let response =
                Response::from_string(reply.to_string()).with_header(header("Content-Type", "application/json"));
            let _ = request.respond(response);
        }
        // A notification, or a reply of the client's: nothing to say
        None => {
            let _ = request.respond(status(202));
        }
    }
}

/// The reply to one JSON-RPC message, or None for one that wants none
pub(super) fn handle(body: &str, call: impl FnOnce(&str, &Value) -> Result<String, String>) -> Option<Value> {
    let message: Value = match serde_json::from_str(body) {
        Ok(m) => m,
        Err(e) => return Some(error(Value::Null, -32700, &format!("Not JSON: {e}"))),
    };
    let id = message.get("id").cloned()?;
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // An answer to a request of ours; this server asks none
        return None;
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked.filter(|v| VERSIONS.contains(v)).unwrap_or(VERSIONS[0]);
            result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "n-studio", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "N Studio's databases: their metadata, and queries run where the user sees them.",
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools::definitions() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, -32602, "tools/call names no tool."));
            };
            if !tools::exists(name) {
                return Some(error(id, -32602, &format!("There is no tool {name}.")));
            }
            let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let outcome = call(name, &arguments);
            let (text, failed) = match outcome {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            result(id, json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
        }
        _ => error(id, -32601, &format!("No method {method}.")),
    })
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_tool(_: &str, _: &Value) -> Result<String, String> {
        panic!("no tool is called")
    }

    #[test]
    fn initialize_agrees_on_a_version_it_speaks() {
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            no_tool,
        )
        .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        let reply = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
            no_tool,
        )
        .unwrap();
        assert_eq!(reply["result"]["protocolVersion"], VERSIONS[0]);
    }

    #[test]
    fn a_notification_gets_no_reply() {
        assert!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, no_tool).is_none());
    }

    #[test]
    fn the_tools_are_listed_with_their_schemas() {
        let reply = handle(r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#, no_tool).unwrap();
        let tools = reply["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
        assert!(tools.iter().any(|t| t["name"] == "run_query"));
    }

    #[test]
    fn a_tool_call_carries_its_outcome() {
        let ok = handle(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_databases","arguments":{}}}"#,
            |name, _| Ok(format!("called {name}")),
        )
        .unwrap();
        assert_eq!(ok["result"]["content"][0]["text"], "called list_databases");
        assert_eq!(ok["result"]["isError"], false);
        let failed = handle(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"run_query","arguments":{}}}"#,
            |_, _| Err("no".into()),
        )
        .unwrap();
        assert_eq!(failed["result"]["isError"], true);
        let unknown =
            handle(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"drop_everything"}}"#, no_tool)
                .unwrap();
        assert_eq!(unknown["error"]["code"], -32602);
    }

    struct Fake;

    const TOKEN: &str = "good";

    impl Host for Fake {
        fn admits(&self, token: &str) -> bool {
            token == TOKEN
        }

        fn call(&self, name: &str, _: &Value) -> Result<String, String> {
            Ok(format!("[{{\"database\":\"fake\",\"called\":\"{name}\"}}]"))
        }
    }

    /// The status line and body of one request to the server
    fn post(port: u16, token: Option<&str>, method: &str, body: &str) -> (String, String) {
        use std::io::Write;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        write!(
            stream,
            "{method} /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        let (head, body) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
        (head.lines().next().unwrap_or("").to_string(), body.to_string())
    }

    #[test]
    fn only_the_agents_token_is_heard() {
        let server = Server::listen(Arc::new(Fake)).unwrap();
        let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        assert!(post(server.port(), None, "POST", ping).0.contains("401"));
        assert!(post(server.port(), Some("bad"), "POST", ping).0.contains("401"));
        let (status, body) = post(server.port(), Some(TOKEN), "POST", ping);
        assert!(status.contains("200"), "{status}");
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["result"], json!({}));
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(post(server.port(), Some(TOKEN), "POST", note).0.contains("202"));
        assert!(post(server.port(), Some(TOKEN), "GET", "").0.contains("405"));
    }

    /// Claude Code itself, signed in, finds the server's tools and calls
    /// one. It spends a little of the account it is signed in to, so it
    /// runs only when asked: `cargo test -p n-studio -- --ignored claude_code`
    #[test]
    #[ignore]
    fn claude_code_calls_a_tool() {
        let server = Server::listen(Arc::new(Fake)).unwrap();
        let config = json!({ "mcpServers": { "n-studio": {
            "type": "http",
            "url": format!("http://127.0.0.1:{}/mcp", server.port()),
            "headers": { "Authorization": format!("Bearer {TOKEN}") },
        }}});
        let output = std::process::Command::new("claude")
            .args([
                "-p",
                "Call the list_databases tool of the n-studio server once and reply with the value of `called` it returns, nothing else.",
                "--mcp-config",
                &config.to_string(),
                "--strict-mcp-config",
                "--allowedTools",
                "mcp__n-studio__list_databases",
            ])
            .output()
            .expect("claude is installed");
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("list_databases"), "{text}\n{}", String::from_utf8_lossy(&output.stderr));
    }

    /// Codex itself, signed in, reaches the server as the panel configures
    /// it (the overrides, the token in the environment) and calls a tool
    /// that is approved without asking. It spends a little of the account it
    /// is signed in to: `cargo test -p n-studio -- --ignored codex_calls`
    #[test]
    #[ignore]
    fn codex_calls_a_tool() {
        let server = Server::listen(Arc::new(Fake)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new("codex")
            .arg("exec")
            .args(["--skip-git-repo-check", "--ephemeral", "-C"])
            .arg(dir.path())
            .args(crate::agent::codex_args(&format!("http://127.0.0.1:{}/mcp", server.port())))
            .arg("Call the list_databases tool of the n-studio server once and reply with the value of `called` it returns, nothing else.")
            .env(crate::agent::TOKEN_VAR, TOKEN)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("codex is installed");
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("list_databases"), "{text}\n{}", String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    fn what_is_not_json_or_not_a_method_is_an_error() {
        assert_eq!(handle("{", no_tool).unwrap()["error"]["code"], -32700);
        assert_eq!(
            handle(r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#, no_tool).unwrap()["error"]["code"],
            -32601
        );
    }
}
