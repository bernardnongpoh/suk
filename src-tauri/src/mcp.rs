//! A minimal MCP server (Streamable HTTP, JSON responses only) that gives Claude Code the tools in
//! `tools.rs`. It listens on localhost and requires a token generated at launch, so only the Claude
//! process this app starts can use it. The database stays in this process: a second process can't
//! open the embedded database while the app has it open.

use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2025-06-18";

/// Runs tool calls; implemented by the app and by tests.
pub trait Backend: Send + Sync + 'static {
    fn call(&self, name: &str, args: Value) -> Result<String, String>;
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub url: String,
    pub token: String,
}

impl Endpoint {
    /// The `--mcp-config` JSON for Claude Code.
    pub fn claude_config(&self, server_name: &str) -> String {
        json!({
            "mcpServers": {
                server_name: {
                    "type": "http",
                    "url": self.url,
                    "headers": { "Authorization": format!("Bearer {}", self.token) }
                }
            }
        })
        .to_string()
    }
}

struct Server<B> {
    backend: B,
    token: String,
}

/// Binds a free localhost port now and serves on the Tauri async runtime.
pub fn start(backend: impl Backend) -> std::io::Result<Endpoint> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let endpoint = Endpoint {
        url: format!("http://{}/mcp", listener.local_addr()?),
        token: uuid::Uuid::new_v4().simple().to_string(),
    };
    let app = router(backend, endpoint.token.clone());
    tauri::async_runtime::spawn(async move {
        let result = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => axum::serve(listener, app).await,
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            eprintln!("mcp server stopped: {e}");
        }
    });
    Ok(endpoint)
}

fn router(backend: impl Backend, token: String) -> Router {
    let server = Arc::new(Server { backend, token });
    Router::new()
        .route("/mcp", post(handle).get(no_stream).delete(no_stream))
        .with_state(server)
}

/// This server never pushes messages, so it offers no event stream.
async fn no_stream() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle<B: Backend>(
    State(server): State<Arc<Server<B>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == server.token);
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return Json(error(Value::Null, -32700, "parse error")).into_response();
    };
    // Notifications and responses carry no id and get no reply.
    let Some(id) = request.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let method = request["method"].as_str().unwrap_or_default().to_string();
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    let reply = match method.as_str() {
        "initialize" => result(
            id,
            json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "professor-os", "version": env!("CARGO_PKG_VERSION") }
            }),
        ),
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": crate::tools::definitions() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default().to_string();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            // Tools touch the database, which blocks.
            let server = server.clone();
            let outcome = tokio::task::spawn_blocking(move || server.backend.call(&name, args))
                .await
                .unwrap_or_else(|e| Err(format!("tool failed: {e}")));
            let (text, is_error) = match outcome {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            result(
                id,
                json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
            )
        }
        _ => error(id, -32601, &format!("method not found: {method}")),
    };
    Json(reply).into_response()
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    struct Echo;

    impl Backend for Echo {
        fn call(&self, name: &str, args: Value) -> Result<String, String> {
            match name {
                "fail" => Err("nope".into()),
                _ => Ok(format!("{name} {args}")),
            }
        }
    }

    async fn post(body: &str, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::post("/mcp").header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = router(Echo, "secret".into())
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn speaks_json_rpc_and_requires_the_token() {
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#;
        assert_eq!(post(init, None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(post(init, Some("wrong")).await.0, StatusCode::UNAUTHORIZED);

        let (status, body) = post(init, Some("secret")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["protocolVersion"], "2025-03-26");

        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(post(note, Some("secret")).await.0, StatusCode::ACCEPTED);

        let (_, list) = post(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, Some("secret")).await;
        let names: Vec<_> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].clone()).collect();
        assert!(names.contains(&json!("propose_schedule")));

        let call = r#"{"jsonrpc":"2.0","id":"a","method":"tools/call","params":{"name":"find","arguments":{"names":["x"]}}}"#;
        let (_, body) = post(call, Some("secret")).await;
        assert_eq!(body["id"], "a");
        assert_eq!(body["result"]["content"][0]["text"], r#"find {"names":["x"]}"#);
        assert_eq!(body["result"]["isError"], false);

        let fail = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fail"}}"#;
        assert_eq!(post(fail, Some("secret")).await.1["result"]["isError"], true);

        let unknown = r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#;
        assert_eq!(post(unknown, Some("secret")).await.1["error"]["code"], -32601);
    }
}
