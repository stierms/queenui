use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderName, HeaderValue, StatusCode},
    response::Response,
    routing::any,
    Router,
};
use futures_util::stream;
use reqwest::Url;
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{task::JoinHandle, time::sleep};
use tokio_util::sync::CancellationToken;

pub(crate) fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("queenui-{label}-{}", uuid::Uuid::new_v4()))
}

pub(crate) fn app_config(engine_path: &str, enabled: bool) -> crate::models::AppConfig {
    crate::models::AppConfig {
        engines: vec![crate::models::EngineProfile {
            id: "engine".into(),
            name: "Fake UCI".into(),
            path: engine_path.into(),
            author: None,
            option_count: 0,
            last_probed_at_ms: None,
            probe_ok: None,
            options: Vec::new(),
            opening_book: None,
        }],
        accounts: vec![crate::models::AccountProfile {
            id: "bot".into(),
            username: "Bot".into(),
            engine_id: "engine".into(),
            rating: Some(2000),
            enabled,
        }],
        ..crate::models::AppConfig::default()
    }
}

#[derive(Default)]
pub(crate) struct MemorySecretStore {
    values: Mutex<HashMap<String, String>>,
    fail_delete: Mutex<Option<String>>,
}

impl MemorySecretStore {
    pub(crate) fn with(account_id: &str, token: &str) -> Self {
        Self {
            values: Mutex::new(HashMap::from([(account_id.to_string(), token.to_string())])),
            fail_delete: Mutex::new(None),
        }
    }

    pub(crate) fn contains(&self, account_id: &str) -> bool {
        self.values
            .lock()
            .expect("memory secrets")
            .contains_key(account_id)
    }

    pub(crate) fn fail_deletes_with(&self, detail: &str) {
        *self.fail_delete.lock().expect("memory secret failure") = Some(detail.to_string());
    }
}

impl crate::storage::SecretStore for MemorySecretStore {
    fn store(&self, account_id: &str, token: &str) -> Result<(), String> {
        self.values
            .lock()
            .expect("memory secrets")
            .insert(account_id.to_string(), token.to_string());
        Ok(())
    }

    fn get(&self, account_id: &str) -> Result<String, String> {
        self.values
            .lock()
            .expect("memory secrets")
            .get(account_id)
            .cloned()
            .ok_or_else(|| "Missing scripted secret".to_string())
    }

    fn delete(&self, account_id: &str) -> Result<(), String> {
        if let Some(detail) = self
            .fail_delete
            .lock()
            .expect("memory secret failure")
            .clone()
        {
            return Err(detail);
        }
        self.values
            .lock()
            .expect("memory secrets")
            .remove(account_id);
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) enum ScriptReply {
    Json(StatusCode, String),
    JsonWithHeaders(StatusCode, String, Vec<(&'static str, &'static str)>),
    /// Sends successful headers, then fails while the body is being decoded.
    /// This is the exact ambiguous-write shape for a committed POST whose
    /// response is lost.
    BodyError(StatusCode),
    /// Returns one NDJSON line and keeps the body open until cancellation.
    NdjsonHold(String, CancellationToken),
    Delay(Duration, Box<ScriptReply>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecordedRequest {
    pub method: String,
    pub path: String,
}

type RouteKey = (String, String);

#[derive(Default)]
struct ScriptState {
    replies: Mutex<HashMap<RouteKey, VecDeque<ScriptReply>>>,
    requests: Mutex<Vec<RecordedRequest>>,
}

pub(crate) struct ScriptedHttp {
    state: Arc<ScriptState>,
    base: Url,
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

impl ScriptedHttp {
    pub(crate) async fn start() -> Self {
        let state = Arc::new(ScriptState::default());
        let app = Router::new()
            .fallback(any(scripted_response))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback scripted transport");
        let address = listener.local_addr().expect("scripted transport address");
        let cancellation = CancellationToken::new();
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .expect("scripted transport server");
        });
        Self {
            state,
            base: Url::parse(&format!("http://{address}/api")).expect("loopback API URL"),
            cancellation,
            task,
        }
    }

    pub(crate) fn base(&self) -> Url {
        self.base.clone()
    }

    pub(crate) fn push(&self, method: &str, path: &str, reply: ScriptReply) {
        self.state
            .replies
            .lock()
            .expect("script replies")
            .entry((method.to_string(), path.to_string()))
            .or_default()
            .push_back(reply);
    }

    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.state.requests.lock().expect("script requests").clone()
    }

    pub(crate) fn count(&self, method: &str, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|request| request.method == method && request.path == path)
            .count()
    }

    pub(crate) async fn wait_for_count(&self, method: &str, path: &str, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.count(method, path) < count {
                tokio::task::yield_now().await;
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("scripted request did not arrive");
    }
}

impl Drop for ScriptedHttp {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.task.abort();
    }
}

async fn scripted_response(
    State(state): State<Arc<ScriptState>>,
    request: Request,
) -> Response<Body> {
    let key = (
        request.method().as_str().to_string(),
        request.uri().path().to_string(),
    );
    state
        .requests
        .lock()
        .expect("script requests")
        .push(RecordedRequest {
            method: key.0.clone(),
            path: key.1.clone(),
        });
    let reply = state
        .replies
        .lock()
        .expect("script replies")
        .get_mut(&key)
        .and_then(VecDeque::pop_front);
    response_for(reply.unwrap_or_else(|| {
        ScriptReply::Json(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"unexpected\":\"{} {}\"}}", key.0, key.1),
        )
    }))
    .await
}

async fn response_for(reply: ScriptReply) -> Response<Body> {
    match reply {
        ScriptReply::Json(status, body) => Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("scripted JSON response"),
        ScriptReply::JsonWithHeaders(status, body, headers) => {
            let mut response = Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("scripted JSON response");
            for (name, value) in headers {
                response.headers_mut().insert(
                    HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                );
            }
            response
        }
        ScriptReply::BodyError(status) => {
            let body = Body::from_stream(stream::once(async {
                Err::<axum::body::Bytes, _>(std::io::Error::other("scripted response loss"))
            }));
            Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(body)
                .expect("scripted broken response")
        }
        ScriptReply::NdjsonHold(line, cancellation) => {
            let body = Body::from_stream(stream::unfold(
                (Some(line.into_bytes()), cancellation),
                |(line, cancellation)| async move {
                    match line {
                        Some(mut line) => {
                            line.push(b'\n');
                            Some((
                                Ok::<_, Infallible>(axum::body::Bytes::from(line)),
                                (None, cancellation),
                            ))
                        }
                        None => {
                            cancellation.cancelled().await;
                            None
                        }
                    }
                },
            ));
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/x-ndjson")
                .body(body)
                .expect("scripted NDJSON response")
        }
        ScriptReply::Delay(delay, reply) => {
            sleep(delay).await;
            Box::pin(response_for(*reply)).await
        }
    }
}
