//! Loopback-only Tilt web client and shared snapshot broker.

use crate::{MonitorError, Result, TiltResource};
use reqwest::blocking::{Client, Response};
use reqwest::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tracing::debug;

pub const DEFAULT_TILT_ENDPOINT: &str = "http://127.0.0.1:10350";
const VIEW_PATH: &str = "api/view";
const TRIGGER_PATH: &str = "api/trigger";
// Production Scio workspaces can return multi-megabyte snapshots because
// /api/view includes logList. Keep the request bounded, but leave enough time
// for an opt-in remote monitor to receive and parse that full response.
const TILT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct TiltSnapshot {
    pub tilt_start_time: String,
    pub resources: Vec<TiltResource>,
    pub response_bytes: usize,
    pub request_duration: Duration,
    pub parse_duration: Duration,
    pub view_request_count: u64,
    fetched_at: Instant,
}

impl TiltSnapshot {
    fn is_fresh(&self, max_age: Duration) -> bool {
        self.fetched_at.elapsed() <= max_age
    }

    pub fn ensure_same_tilt_session(&self, other: &Self) -> Result<()> {
        if self.tilt_start_time == other.tilt_start_time {
            Ok(())
        } else {
            Err(MonitorError::TiltRestarted)
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TiltBrokerMetrics {
    pub request_count: u64,
    pub response_bytes: u64,
    pub max_in_flight: u64,
}

#[derive(Debug)]
struct TiltClientInner {
    endpoint: Url,
    http: Client,
    web_cookie: Mutex<Option<String>>,
}

/// Client for Tilt's loopback web API.
#[derive(Debug, Clone)]
pub struct TiltClient {
    inner: Arc<TiltClientInner>,
}

impl TiltClient {
    pub fn new(endpoint: impl AsRef<str>) -> Result<Self> {
        let endpoint = Url::parse(endpoint.as_ref())
            .map_err(|error| MonitorError::InvalidTiltEndpoint(error.to_string()))?;
        if endpoint.scheme() != "http"
            || endpoint.host_str() != Some("127.0.0.1")
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(MonitorError::InvalidTiltEndpoint(
                "Tilt endpoint must use http://127.0.0.1:<port>".to_string(),
            ));
        }
        if endpoint.port().is_none() {
            return Err(MonitorError::InvalidTiltEndpoint(
                "Tilt endpoint must include an explicit port".to_string(),
            ));
        }

        let http = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(TILT_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| MonitorError::TiltHttp(error.to_string()))?;

        Ok(Self {
            inner: Arc::new(TiltClientInner { endpoint, http, web_cookie: Mutex::new(None) }),
        })
    }

    pub fn endpoint(&self) -> &Url {
        &self.inner.endpoint
    }

    pub fn is_running(&self) -> bool {
        self.get_snapshot().is_ok()
    }

    pub fn get_snapshot(&self) -> Result<TiltSnapshot> {
        let request_started = Instant::now();
        let (response, view_request_count) = self.send_view()?;
        self.parse_view_response(response, request_started, view_request_count)
    }

    pub fn get_resources(&self) -> Result<Vec<TiltResource>> {
        Ok(self.get_snapshot()?.resources)
    }

    pub fn get_monitored_resources(&self) -> Result<Vec<TiltResource>> {
        Ok(self.get_resources()?.into_iter().filter(TiltResource::should_monitor).collect())
    }

    pub fn trigger_resource(&self, resource_name: &str) -> Result<()> {
        let response = self.send_trigger_once(resource_name)?;
        if response.status() == StatusCode::FORBIDDEN {
            let body =
                response.text().map_err(|error| MonitorError::TiltHttp(error.to_string()))?;
            if !body.to_ascii_lowercase().contains("missing session token") {
                return Err(MonitorError::TiltTrigger(format!(
                    "Tilt /api/trigger returned 403: {body}"
                )));
            }
            self.bootstrap_web_cookie()?;
            return self
                .classify_trigger_response(resource_name, self.send_trigger_once(resource_name)?);
        }
        self.classify_trigger_response(resource_name, response)
    }

    fn send_trigger_once(&self, resource_name: &str) -> Result<Response> {
        let url = self.url(TRIGGER_PATH)?;
        let body = serde_json::json!({
            "manifest_names": [resource_name],
            "build_reason": 32,
        });
        let mut request = self.inner.http.post(url).json(&body);
        if let Some(cookie) = self.web_cookie()? {
            request = request.header(COOKIE, cookie);
        }
        request.send().map_err(|error| MonitorError::TiltHttp(error.to_string()))
    }

    fn classify_trigger_response(&self, resource_name: &str, response: Response) -> Result<()> {
        let status = response.status();
        let bytes = response.bytes().map_err(|error| MonitorError::TiltHttp(error.to_string()))?;

        match status {
            StatusCode::OK if bytes.is_empty() => Ok(()),
            StatusCode::NOT_FOUND => {
                Err(MonitorError::TiltTriggerUnknownResource(resource_name.to_string()))
            }
            _ => Err(MonitorError::TiltTrigger(format!(
                "unexpected response: status={status}, body_bytes={}",
                bytes.len()
            ))),
        }
    }

    fn send_view(&self) -> Result<(Response, u64)> {
        let response = self.send_view_once()?;
        if response.status() != StatusCode::FORBIDDEN {
            return Ok((response, 1));
        }

        let body = response.text().map_err(|error| MonitorError::TiltHttp(error.to_string()))?;
        if !body.to_ascii_lowercase().contains("missing session token") {
            return Err(MonitorError::TiltHttp(format!("Tilt /api/view returned 403: {body}")));
        }

        self.bootstrap_web_cookie()?;
        Ok((self.send_view_once()?, 2))
    }

    fn send_view_once(&self) -> Result<Response> {
        let url = self.url(VIEW_PATH)?;
        let mut request = self.inner.http.get(url);
        if let Some(cookie) = self.web_cookie()? {
            request = request.header(COOKIE, cookie);
        }
        request.send().map_err(|error| {
            if error.is_connect() {
                MonitorError::TiltNotRunning
            } else {
                MonitorError::TiltHttp(error.to_string())
            }
        })
    }

    fn bootstrap_web_cookie(&self) -> Result<()> {
        let response = self
            .inner
            .http
            .get(self.inner.endpoint.clone())
            .send()
            .map_err(|error| MonitorError::TiltHttp(error.to_string()))?;
        if !response.status().is_success() {
            return Err(MonitorError::TiltHttp(format!(
                "Tilt web session bootstrap returned {}",
                response.status()
            )));
        }
        let cookie = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next().map(str::trim))
            .find(|value| value.starts_with("Tilt-Token="))
            .map(str::to_string)
            .ok_or(MonitorError::TiltWebCookieMissing)?;
        *self
            .inner
            .web_cookie
            .lock()
            .map_err(|_| MonitorError::TiltHttp("Tilt cookie lock poisoned".to_string()))? =
            Some(cookie);
        Ok(())
    }

    fn parse_view_response(
        &self,
        response: Response,
        request_started: Instant,
        view_request_count: u64,
    ) -> Result<TiltSnapshot> {
        let status = response.status();
        if status != StatusCode::OK {
            return Err(MonitorError::TiltHttp(format!("Tilt /api/view returned {status}")));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err(MonitorError::TiltViewHtmlResponse(content_type.to_string()));
        }

        let body = response.bytes().map_err(|error| MonitorError::TiltHttp(error.to_string()))?;
        let request_duration = request_started.elapsed();
        let response_bytes = body.len();
        let parse_started = Instant::now();
        let view: TiltView = serde_json::from_slice(&body)
            .map_err(|error| MonitorError::TiltViewSchema(error.to_string()))?;
        if view.tilt_start_time.trim().is_empty()
            || chrono::DateTime::parse_from_rfc3339(&view.tilt_start_time).is_err()
        {
            return Err(MonitorError::TiltViewSchema(
                "tiltStartTime must be a nonempty RFC3339 timestamp".to_string(),
            ));
        }

        let resources = view
            .ui_resources
            .iter()
            .map(|resource| {
                TiltResource::from_json(resource).ok_or_else(|| {
                    MonitorError::TiltViewSchema(
                        "uiResources contains an invalid resource entry".to_string(),
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(|resource| resource.name != "(Tiltfile)")
            .collect::<Vec<_>>();
        let parse_duration = parse_started.elapsed();
        debug!(
            resource_count = resources.len(),
            response_bytes,
            ?request_duration,
            ?parse_duration,
            "Fetched Tilt snapshot"
        );

        Ok(TiltSnapshot {
            tilt_start_time: view.tilt_start_time,
            resources,
            response_bytes,
            request_duration,
            parse_duration,
            view_request_count,
            fetched_at: Instant::now(),
        })
    }

    fn url(&self, path: &str) -> Result<Url> {
        self.inner
            .endpoint
            .join(path)
            .map_err(|error| MonitorError::InvalidTiltEndpoint(error.to_string()))
    }

    fn web_cookie(&self) -> Result<Option<String>> {
        self.inner
            .web_cookie
            .lock()
            .map(|cookie| cookie.clone())
            .map_err(|_| MonitorError::TiltHttp("Tilt cookie lock poisoned".to_string()))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TiltView {
    tilt_start_time: String,
    ui_resources: Vec<serde_json::Value>,
}

#[derive(Debug, Default)]
struct BrokerState {
    latest: Option<Arc<TiltSnapshot>>,
    metrics: TiltBrokerMetrics,
    in_flight: bool,
    last_failure: Option<String>,
}

#[derive(Debug, Default)]
struct BrokerShared {
    state: Mutex<BrokerState>,
    completed: Condvar,
}

/// Coalesces Tilt reads so monitor/stats/telemetry can share one immutable response.
#[derive(Debug, Clone)]
pub struct TiltSnapshotBroker {
    client: TiltClient,
    max_age: Duration,
    shared: Arc<BrokerShared>,
}

impl TiltSnapshotBroker {
    pub fn new(endpoint: impl AsRef<str>, max_age: Duration) -> Result<Self> {
        Ok(Self {
            client: TiltClient::new(endpoint)?,
            max_age,
            shared: Arc::new(BrokerShared::default()),
        })
    }

    pub fn client(&self) -> &TiltClient {
        &self.client
    }

    pub fn snapshot(&self) -> Result<Arc<TiltSnapshot>> {
        self.read(false)
    }

    pub fn refresh(&self) -> Result<Arc<TiltSnapshot>> {
        self.read(true)
    }

    pub fn resources(&self) -> Result<Vec<TiltResource>> {
        Ok(self.snapshot()?.resources.clone())
    }

    pub fn monitored_resources(&self) -> Result<Vec<TiltResource>> {
        Ok(self.resources()?.into_iter().filter(TiltResource::should_monitor).collect())
    }

    pub fn metrics(&self) -> Result<TiltBrokerMetrics> {
        self.shared
            .state
            .lock()
            .map(|state| state.metrics)
            .map_err(|_| MonitorError::TiltHttp("Tilt snapshot lock poisoned".to_string()))
    }

    fn read(&self, force: bool) -> Result<Arc<TiltSnapshot>> {
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| MonitorError::TiltHttp("Tilt snapshot lock poisoned".to_string()))?;

        if !force {
            if let Some(snapshot) = &state.latest {
                if snapshot.is_fresh(self.max_age) {
                    return Ok(Arc::clone(snapshot));
                }
            }
        }

        if state.in_flight {
            while state.in_flight {
                state = self.shared.completed.wait(state).map_err(|_| {
                    MonitorError::TiltHttp("Tilt snapshot lock poisoned".to_string())
                })?;
            }
            if let Some(failure) = &state.last_failure {
                return Err(MonitorError::TiltHttp(failure.clone()));
            }
            return state.latest.as_ref().map(Arc::clone).ok_or_else(|| {
                MonitorError::TiltHttp("coalesced Tilt snapshot request failed".to_string())
            });
        }

        state.in_flight = true;
        state.metrics.max_in_flight = state.metrics.max_in_flight.max(1);
        drop(state);

        let result = self.client.get_snapshot().map(Arc::new);
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| MonitorError::TiltHttp("Tilt snapshot lock poisoned".to_string()))?;
        state.in_flight = false;
        match &result {
            Ok(snapshot) => {
                state.metrics.request_count += snapshot.view_request_count;
                state.metrics.response_bytes += snapshot.response_bytes as u64;
                state.latest = Some(Arc::clone(snapshot));
                state.last_failure = None;
            }
            Err(error) => state.last_failure = Some(error.to_string()),
        }
        self.shared.completed.notify_all();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const VIEW: &str = r#"{
        "tiltStartTime": "2026-08-17T10:00:00Z",
        "uiResources": [{
            "metadata": {"name": "test-service", "labels": ["service"]},
            "status": {
                "runtimeStatus": "ok",
                "updateStatus": "ok",
                "conditions": [{"type": "Ready", "status": "True"}],
                "buildHistory": [{
                    "spanID": "build:3",
                    "startTime": "2026-08-17T10:00:01Z",
                    "finishTime": "2026-08-17T10:00:02Z",
                    "error": ""
                }]
            }
        }],
        "logList": {"spans": {"large": "ignored"}}
    }"#;

    fn serve(responses: Vec<String>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_thread = Arc::clone(&requests);
        std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer).unwrap();
                requests_for_thread.fetch_add(1, Ordering::SeqCst);
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}/"), requests)
    }

    fn http(status: &str, content_type: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn accepts_strict_view_shape_and_filters_tiltfile() {
        let (endpoint, _) = serve(vec![http("200 OK", "application/json", VIEW)]);
        let snapshot = TiltClient::new(endpoint).unwrap().get_snapshot().unwrap();
        assert_eq!(snapshot.tilt_start_time, "2026-08-17T10:00:00Z");
        assert_eq!(snapshot.resources.len(), 1);
        assert_eq!(snapshot.resources[0].name, "test-service");
        assert_eq!(snapshot.resources[0].build_history[0].span_id, "build:3");
    }

    #[test]
    fn request_duration_includes_response_body_transfer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_http_request(&mut stream);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                VIEW.len()
            );
            stream.write_all(headers.as_bytes()).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            stream.write_all(VIEW.as_bytes()).unwrap();
        });

        let snapshot =
            TiltClient::new(format!("http://{address}/")).unwrap().get_snapshot().unwrap();
        assert!(snapshot.request_duration >= Duration::from_millis(40));
    }

    #[test]
    fn rejects_html_200_and_old_cli_items_shape() {
        let (html_endpoint, _) = serve(vec![http("200 OK", "text/html", "<html></html>")]);
        assert!(matches!(
            TiltClient::new(html_endpoint).unwrap().get_snapshot(),
            Err(MonitorError::TiltViewHtmlResponse(_))
        ));

        let old = r#"{"items": [], "tiltStartTime": "2026-08-17T10:00:00Z"}"#;
        let (old_endpoint, _) = serve(vec![http("200 OK", "application/json", old)]);
        assert!(matches!(
            TiltClient::new(old_endpoint).unwrap().get_snapshot(),
            Err(MonitorError::TiltViewSchema(_))
        ));
    }

    #[test]
    fn bootstraps_loopback_web_cookie_only_when_required() {
        let forbidden = http("403 Forbidden", "text/plain", "missing session token");
        let root = "HTTP/1.1 200 OK\r\nSet-Cookie: Tilt-Token=abc123; SameSite=Strict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string();
        let (endpoint, requests) =
            serve(vec![forbidden, root, http("200 OK", "application/json", VIEW)]);
        let snapshot = TiltClient::new(endpoint).unwrap().get_snapshot().unwrap();
        assert_eq!(snapshot.resources.len(), 1);
        assert_eq!(snapshot.view_request_count, 2);
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn broker_reuses_one_snapshot_within_cadence() {
        let (endpoint, requests) = serve(vec![http("200 OK", "application/json", VIEW)]);
        let broker = TiltSnapshotBroker::new(endpoint, Duration::from_secs(2)).unwrap();
        let first = broker.snapshot().unwrap();
        let second = broker.snapshot().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(broker.metrics().unwrap().request_count, 1);
        assert_eq!(broker.metrics().unwrap().max_in_flight, 1);
    }

    #[test]
    fn concurrent_consumers_coalesce_onto_one_request() {
        let (endpoint, requests) = serve(vec![http("200 OK", "application/json", VIEW)]);
        let broker = TiltSnapshotBroker::new(endpoint, Duration::from_secs(2)).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let broker = broker.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    broker.snapshot().unwrap().tilt_start_time.clone()
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            assert_eq!(handle.join().unwrap(), "2026-08-17T10:00:00Z");
        }
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(broker.metrics().unwrap().request_count, 1);
    }

    #[test]
    fn detects_tilt_restart_between_snapshots() {
        let restarted = VIEW.replace("2026-08-17T10:00:00Z", "2026-08-17T10:01:00Z");
        let (endpoint, _) = serve(vec![
            http("200 OK", "application/json", VIEW),
            http("200 OK", "application/json", &restarted),
        ]);
        let broker = TiltSnapshotBroker::new(endpoint, Duration::from_secs(2)).unwrap();
        let first = broker.snapshot().unwrap();
        let second = broker.refresh().unwrap();
        assert!(matches!(
            first.ensure_same_tilt_session(&second),
            Err(MonitorError::TiltRestarted)
        ));
    }

    #[test]
    fn posts_exact_tilt_trigger_contract() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let captured_for_thread = Arc::clone(&captured);
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            *captured_for_thread.lock().unwrap() = read_http_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        });

        TiltClient::new(format!("http://{address}/"))
            .unwrap()
            .trigger_resource("qe-bazel-build")
            .unwrap();
        let request = captured.lock().unwrap().clone();
        assert!(request.starts_with("POST /api/trigger HTTP/1.1\r\n"));
        assert!(request.to_ascii_lowercase().contains("content-type: application/json"));
        assert!(request.contains(r#"{"build_reason":32,"manifest_names":["qe-bazel-build"]}"#));
    }

    #[test]
    fn classifies_unknown_trigger_resource() {
        let (endpoint, _) = serve(vec![http("404 Not Found", "text/plain", "unknown")]);
        assert!(matches!(
            TiltClient::new(endpoint)
                .unwrap()
                .trigger_resource("missing-resource"),
            Err(MonitorError::TiltTriggerUnknownResource(resource))
                if resource == "missing-resource"
        ));
    }

    #[test]
    fn trigger_bootstraps_loopback_web_cookie_when_required() {
        let forbidden = http("403 Forbidden", "text/plain", "missing session token");
        let root = "HTTP/1.1 200 OK\r\nSet-Cookie: Tilt-Token=abc123; SameSite=Strict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string();
        let success =
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string();
        let (endpoint, requests) = serve(vec![forbidden, root, success]);
        TiltClient::new(endpoint).unwrap().trigger_resource("qe-bazel-build").unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn validates_loopback_endpoint() {
        for endpoint in
            ["http://localhost:10350", "http://0.0.0.0:10350", "https://127.0.0.1:10350"]
        {
            assert!(matches!(TiltClient::new(endpoint), Err(MonitorError::InvalidTiltEndpoint(_))));
        }
    }
}
