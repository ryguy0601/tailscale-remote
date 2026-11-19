use crate::input::InputCommand;
use crate::keymap;
use crate::protocol::{ClientMsg, ConnectedClient, HostEvent, ServerMsg};
use crate::qr;
use crate::system::{self, ShortcutItem};
use crate::tailscale::{is_tailscale_or_loopback, normalize_ip, TailscaleInfo};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, watch};

const QR_HTML_TEMPLATE: &str = include_str!("../static/qr.html");
const REMOTE_HTML: &str = include_str!("../static/remote.html");

const TOKEN_COOKIE_NAME: &str = "tsr_token";
const TOKEN_HEADER_NAME: &str = "x-tsr-token";
const CSRF_HEADER_NAME: &str = "x-tsr-csrf";
const MAX_DELTA_MOVE: i32 = 400;
const MAX_DELTA_SCROLL: i32 = 24;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_BODY_BYTES: usize = 64 * 1024;
const AUTH_WINDOW: Duration = Duration::from_secs(60);
const AUTH_MAX_FAILURES: usize = 20;
const AUTH_MAX_TRACKED_IPS: usize = 1024;

/// Active couch sleep timer scheduled by the user.
pub struct ActiveSleepTimer {
    pub ends_at_ms: u64,
    pub action: String,
    pub task: tokio::task::JoinHandle<()>,
}

/// Shared server state across HTTP routes and WebSocket handlers.
pub struct AppState {
    pub token: RwLock<String>,
    pub token_epoch: watch::Sender<u64>,
    pub auth_enabled: bool,
    pub allow_lan: bool,
    pub dry_run: bool,
    pub port: u16,
    pub use_magicdns: bool,
    pub tailscale: TailscaleInfo,
    pub input_tx: mpsc::Sender<InputCommand>,
    pub clients: RwLock<HashMap<u64, ConnectedClient>>,
    pub next_client_id: AtomicU64,
    pub host_tx: broadcast::Sender<HostEvent>,
    pub shortcuts: RwLock<Vec<ShortcutItem>>,
    pub sleep_timer: Mutex<Option<ActiveSleepTimer>>,
    pub auth_failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl AppState {
    pub fn new(
        token: String,
        auth_enabled: bool,
        allow_lan: bool,
        dry_run: bool,
        port: u16,
        use_magicdns: bool,
        tailscale: TailscaleInfo,
        input_tx: mpsc::Sender<InputCommand>,
    ) -> Arc<Self> {
        let (host_tx, _) = broadcast::channel(32);
        let (token_epoch, _) = watch::channel(0u64);
        Arc::new(Self {
            token: RwLock::new(token),
            token_epoch,
            auth_enabled,
            allow_lan,
            dry_run,
            port,
            use_magicdns,
            tailscale,
            input_tx,
            clients: RwLock::new(HashMap::new()),
            next_client_id: AtomicU64::new(1),
            host_tx,
            shortcuts: RwLock::new(system::load_shortcuts()),
            sleep_timer: Mutex::new(None),
            auth_failures: Mutex::new(HashMap::new()),
        })
    }

    pub fn current_token(&self) -> String {
        self.token.read().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn format_url_for_host(&self, host: &str) -> String {
        if self.auth_enabled {
            let tok = self.current_token();
            format!("http://{host}:{}/?t={tok}", self.port)
        } else {
            format!("http://{host}:{}/", self.port)
        }
    }

    pub fn pairing_url(&self) -> String {
        if self.use_magicdns {
            if let Some(ref dns) = self.tailscale.dns_name {
                return self.format_url_for_host(dns);
            }
        }
        self.format_url_for_host(&self.tailscale.ipv4)
    }

    pub fn magic_dns_url(&self) -> Option<String> {
        if self.use_magicdns {
            return None;
        }
        self.tailscale
            .dns_name
            .as_ref()
            .map(|dns| self.format_url_for_host(dns))
    }

    pub fn connected_list(&self) -> Vec<ConnectedClient> {
        let mut list: Vec<ConnectedClient> = self
            .clients
            .read()
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by_key(|c| c.id);
        list
    }

    pub fn snapshot(&self) -> HostEvent {
        let pairing_url = self.pairing_url();
        let qr_svg = qr::generate_svg(&pairing_url).unwrap_or_default();
        HostEvent::Snapshot {
            connected: self.connected_list(),
            pairing_url,
            magic_dns_url: self.magic_dns_url(),
            qr_svg,
            auth_enabled: self.auth_enabled,
            hostname: self.tailscale.hostname.clone(),
            tailscale_ip: self.tailscale.ipv4.clone(),
            os: std::env::consts::OS.to_string(),
        }
    }

    pub fn broadcast_snapshot(&self) {
        let _ = self.host_tx.send(self.snapshot());
    }
}

/// Builds the Axum router with loopback-only pairing endpoints and Tailscale-guarded remote endpoints.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(remote_page))
        .route("/qr", get(qr_page))
        .route("/ws/remote", get(ws_remote))
        .route("/ws/host", get(ws_host))
        .route("/api/status", get(api_status))
        .route("/api/token/rotate", post(api_rotate_token))
        .route("/api/shortcuts", get(api_get_shortcuts).post(api_add_shortcut))
        .route("/api/shortcuts/reset", post(api_reset_shortcuts))
        .route("/api/shortcuts/{id}", delete(api_delete_shortcut))
        .route("/api/launch", post(api_launch))
        .route("/api/audio", get(api_audio))
        .route("/api/audio/volume", post(api_audio_volume))
        .route("/api/audio/mute", post(api_audio_mute))
        .route("/api/audio/output", post(api_audio_output))
        .route("/api/now-playing", get(api_now_playing))
        .route("/api/screen", get(api_screen))
        .route(
            "/api/sleep-timer",
            get(api_get_sleep_timer)
                .post(api_set_sleep_timer)
                .delete(api_delete_sleep_timer),
        )
        .route("/api/power", post(api_power))
        .route("/api/peers", get(api_peers))
        .route("/api/windows", get(api_get_windows))
        .route("/api/windows/focus", post(api_focus_window))
        .route("/api/windows/close", post(api_close_window))
        .route("/api/workspace", post(api_switch_workspace))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        axum::http::header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' blob: data:; connect-src 'self' ws: wss:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    if !headers.contains_key(CACHE_CONTROL) {
        headers.insert(
            CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, must-revalidate"),
        );
    }
    resp
}

#[derive(Deserialize, Default)]
struct AuthQuery {
    #[serde(default)]
    t: Option<String>,
}

/// Constant-time byte slice equality to avoid timing side-channels on token checks.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in ab.iter().zip(bb.iter()) {
        diff |= std::hint::black_box(x) ^ std::hint::black_box(y);
    }
    std::hint::black_box(diff) == 0
}

fn extract_cookie_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let trimmed = part.trim();
        if let Some((k, v)) = trimmed.split_once('=') {
            if k.trim() == TOKEN_COOKIE_NAME {
                let val = v.trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn extract_header_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(TOKEN_HEADER_NAME)?.to_str().ok()?.trim();
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    normalize_ip(ip).is_loopback()
}

fn forbidden_response(msg: &'static str) -> Response {
    (StatusCode::FORBIDDEN, msg).into_response()
}

fn extract_host_only(raw_host: &str) -> Option<String> {
    let trimmed = raw_host.trim();
    if trimmed.is_empty()
        || trimmed
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '@' | '/' | '\\' | '?' | '#'))
    {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix('[') {
        let (addr, after) = rest.split_once(']')?;
        if addr.is_empty() {
            return None;
        }
        if !after.is_empty() {
            let port = after.strip_prefix(':')?;
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
        }
        return Some(addr.to_ascii_lowercase());
    }

    if let Some((prefix, suffix)) = trimmed.split_once(':') {
        if suffix.contains(':') {
            // Bare IPv6 literal without brackets or port
            if trimmed.parse::<IpAddr>().is_ok() {
                return Some(trimmed.to_ascii_lowercase());
            }
            return None;
        }
        if prefix.is_empty() || suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return Some(prefix.to_ascii_lowercase());
    }

    Some(trimmed.to_ascii_lowercase())
}

fn extract_origin_authority(origin: &str) -> Option<String> {
    let trimmed = origin.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        return None;
    }
    let (scheme, rest) = trimmed.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    if rest.is_empty()
        || rest
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '?' | '#' | '@' | '\\'))
    {
        return None;
    }
    Some(rest.to_ascii_lowercase())
}

fn is_private_or_link_local_ip(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        IpAddr::V6(v6) => {
            let seg0 = v6.segments()[0];
            v6.is_loopback() || (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80
        }
    }
}

fn check_loopback_host(headers: &HeaderMap) -> Result<(), Response> {
    let raw_host = headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| forbidden_response("Forbidden: missing Host header."))?;
    let host = extract_host_only(raw_host)
        .ok_or_else(|| forbidden_response("Forbidden: invalid Host header."))?;
    let allowed = host == "127.0.0.1"
        || host == "localhost"
        || host == "::1"
        || host
            .parse::<IpAddr>()
            .map(|ip| normalize_ip(ip).is_loopback())
            .unwrap_or(false);
    if allowed {
        Ok(())
    } else {
        Err(forbidden_response(
            "Forbidden: Host header must be loopback for local admin endpoints.",
        ))
    }
}

fn check_allowed_host(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let raw_host = headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| forbidden_response("Forbidden: missing Host header."))?;
    let host = extract_host_only(raw_host)
        .ok_or_else(|| forbidden_response("Forbidden: invalid Host header."))?;
    let host_no_dot = host.trim_end_matches('.');

    if host_no_dot == "127.0.0.1" || host_no_dot == "localhost" || host == "::1" {
        return Ok(());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_tailscale_or_loopback(ip) || (state.allow_lan && is_private_or_link_local_ip(ip)) {
            return Ok(());
        }
        return Err(forbidden_response("Forbidden: untrusted Host IP."));
    }
    let ts_host = state.tailscale.hostname.trim_end_matches('.');
    if !ts_host.is_empty() && host_no_dot.eq_ignore_ascii_case(ts_host) {
        return Ok(());
    }
    if let Some(ref dns) = state.tailscale.dns_name {
        let clean_dns = dns.trim_end_matches('.');
        if !clean_dns.is_empty() && host_no_dot.eq_ignore_ascii_case(clean_dns) {
            return Ok(());
        }
    }
    if state.allow_lan && host_no_dot.ends_with(".local") && host_no_dot.len() > 6 {
        return Ok(());
    }

    Err(forbidden_response("Forbidden: untrusted Host header."))
}

fn check_sec_fetch_site(headers: &HeaderMap) -> Result<(), Response> {
    if let Some(val) = headers.get("sec-fetch-site") {
        let s = val
            .to_str()
            .ok()
            .map(str::trim)
            .unwrap_or("");
        if !s.eq_ignore_ascii_case("same-origin") && !s.eq_ignore_ascii_case("none") {
            return Err(forbidden_response("Forbidden: cross-site request blocked."));
        }
    }
    Ok(())
}

fn check_same_origin(headers: &HeaderMap, require_origin: bool) -> Result<(), Response> {
    check_sec_fetch_site(headers)?;
    let Some(origin_hdr) = headers.get(ORIGIN) else {
        return if require_origin {
            Err(forbidden_response("Forbidden: Origin header is required."))
        } else {
            Ok(())
        };
    };
    let origin_str = origin_hdr
        .to_str()
        .ok()
        .ok_or_else(|| forbidden_response("Forbidden: invalid Origin header."))?;
    let origin_auth = extract_origin_authority(origin_str)
        .ok_or_else(|| forbidden_response("Forbidden: untrusted Origin header."))?;
    let host_auth = headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| forbidden_response("Forbidden: missing Host header."))?;

    if origin_auth == host_auth {
        Ok(())
    } else {
        Err(forbidden_response("Forbidden: Origin does not match Host."))
    }
}

fn check_auth_rate_limit(state: &AppState, ip: IpAddr) -> Result<(), Response> {
    if !state.auth_enabled {
        return Ok(());
    }
    let norm = normalize_ip(ip);
    let now = Instant::now();
    if let Ok(mut map) = state.auth_failures.lock() {
        if let Some(entries) = map.get_mut(&norm) {
            entries.retain(|t| now.saturating_duration_since(*t) < AUTH_WINDOW);
            if entries.len() >= AUTH_MAX_FAILURES {
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({
                        "ok": false,
                        "error": "Too many failed authentication attempts; try again later"
                    })),
                )
                    .into_response());
            }
            if entries.is_empty() {
                map.remove(&norm);
            }
        }
    }
    Ok(())
}

fn record_auth_failure(state: &AppState, ip: IpAddr) {
    if !state.auth_enabled {
        return;
    }
    let norm = normalize_ip(ip);
    let now = Instant::now();
    if let Ok(mut map) = state.auth_failures.lock() {
        if map.len() >= AUTH_MAX_TRACKED_IPS && !map.contains_key(&norm) {
            map.retain(|_, entries| {
                entries.retain(|t| now.saturating_duration_since(*t) < AUTH_WINDOW);
                !entries.is_empty()
            });
            if map.len() >= AUTH_MAX_TRACKED_IPS {
                if let Some(oldest_key) = map.keys().next().copied() {
                    map.remove(&oldest_key);
                }
            }
        }
        let entries = map.entry(norm).or_default();
        entries.retain(|t| now.saturating_duration_since(*t) < AUTH_WINDOW);
        entries.push(now);
    }
}

fn clear_auth_failures(state: &AppState, ip: IpAddr) {
    let norm = normalize_ip(ip);
    if let Ok(mut map) = state.auth_failures.lock() {
        map.remove(&norm);
    }
}

fn check_tailnet_access(state: &AppState, ip: IpAddr) -> Result<(), Response> {
    if state.allow_lan || is_tailscale_or_loopback(ip) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Html(
                "<!doctype html><html><body style='font-family:sans-serif;background:#070b14;color:#f8fafc;display:grid;place-items:center;min-height:100vh;padding:24px;text-align:center'><div><h2>403 — Tailscale Access Only</h2><p style='color:#94a3b8;margin-top:8px'>Connect from a device on your Tailnet or launch with <code>--allow-lan</code>.</p></div></body></html>",
            ),
        )
            .into_response())
    }
}

/// Validates `X-TSR-Token` header, `tsr_token` cookie, or `?t=` query token.
/// Returns `Ok(Some(token))` if newly authenticated via query string (so caller sets `Set-Cookie`).
fn authorize_request(
    state: &AppState,
    query: &AuthQuery,
    headers: &HeaderMap,
) -> Result<Option<String>, ()> {
    if !state.auth_enabled {
        return Ok(None);
    }
    let expected = state.current_token();
    if let Some(ht) = extract_header_token(headers) {
        if ct_eq(&ht, &expected) {
            return Ok(None);
        }
    }
    if let Some(ct) = extract_cookie_token(headers) {
        if ct_eq(&ct, &expected) {
            return Ok(None);
        }
    }
    if let Some(ref qt) = query.t {
        if ct_eq(qt, &expected) {
            return Ok(Some(expected));
        }
    }
    Err(())
}

fn require_remote_auth(
    state: &AppState,
    ip: IpAddr,
    query: &AuthQuery,
    headers: &HeaderMap,
) -> Result<(), Response> {
    check_tailnet_access(state, ip)?;
    check_allowed_host(state, headers)?;
    check_same_origin(headers, false)?;
    check_auth_rate_limit(state, ip)?;
    if authorize_request(state, query, headers).is_err() {
        record_auth_failure(state, ip);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "ok": false, "error": "Invalid or missing pairing token" })),
        )
            .into_response());
    }
    clear_auth_failures(state, ip);
    Ok(())
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn qr_page(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if !is_loopback(addr.ip()) {
        return (
            StatusCode::FORBIDDEN,
            "Pairing QR screen is restricted to localhost.",
        )
            .into_response();
    }
    if let Err(resp) = check_loopback_host(&headers) {
        return resp;
    }
    if let Err(resp) = check_sec_fetch_site(&headers) {
        return resp;
    }

    let pairing_url = state.pairing_url();
    let qr_svg = qr::generate_svg(&pairing_url).unwrap_or_else(|_| "<p>QR Error</p>".to_string());
    let magic_html = match state.magic_dns_url() {
        Some(url) => format!("MagicDNS: <code>{}</code>", html_escape(&url)),
        None => String::new(),
    };
    let auth_badge = if state.auth_enabled {
        "Tailnet + Token"
    } else {
        "Tailnet Only"
    };

    let rendered = QR_HTML_TEMPLATE
        .replace("__QR_SVG__", &qr_svg)
        .replace("__PAIRING_URL__", &html_escape(&pairing_url))
        .replace("__MAGIC_DNS_HTML__", &magic_html)
        .replace("__HOSTNAME__", &html_escape(&state.tailscale.hostname))
        .replace("__TAILSCALE_IP__", &html_escape(&state.tailscale.ipv4))
        .replace("__OS_NAME__", &html_escape(std::env::consts::OS))
        .replace("__AUTH_BADGE__", auth_badge);

    Html(rendered).into_response()
}

async fn remote_page(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = check_tailnet_access(&state, addr.ip()) {
        return resp;
    }
    if let Err(resp) = check_allowed_host(&state, &headers) {
        return resp;
    }
    if let Err(resp) = check_auth_rate_limit(&state, addr.ip()) {
        return resp;
    }

    match authorize_request(&state, &query, &headers) {
        Ok(maybe_new_token) => {
            clear_auth_failures(&state, addr.ip());
            let mut resp = Html(REMOTE_HTML).into_response();
            if let Some(tok) = maybe_new_token {
                let cookie_val = format!("{TOKEN_COOKIE_NAME}={tok}; HttpOnly; SameSite=Lax; Path=/");
                if let Ok(hv) = cookie_val.parse() {
                    resp.headers_mut().insert(SET_COOKIE, hv);
                }
            }
            resp
        }
        Err(()) => {
            record_auth_failure(&state, addr.ip());
            (
                StatusCode::UNAUTHORIZED,
                Html(
                    "<!doctype html><html><head><meta name='viewport' content='width=device-width,initial-scale=1'><title>Pairing Required</title></head><body style='font-family:sans-serif;background:#070b14;color:#f8fafc;display:grid;place-items:center;min-height:100vh;padding:24px;text-align:center'><div style='max-width:380px;background:#0f172a;border:1px solid rgba(148,163,184,0.2);border-radius:20px;padding:24px'><h2 style='margin-bottom:10px'>Pairing Token Required</h2><p style='color:#94a3b8;line-height:1.5'>Your pairing link is missing or expired. Scan the QR code displayed on your media PC to connect.</p></div></body></html>",
                ),
            )
                .into_response()
        }
    }
}

async fn api_status(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = check_tailnet_access(&state, addr.ip()) {
        return resp;
    }
    if let Err(resp) = check_allowed_host(&state, &headers) {
        return resp;
    }
    if !is_loopback(addr.ip()) {
        if let Err(resp) = check_same_origin(&headers, false) {
            return resp;
        }
        if let Err(resp) = check_auth_rate_limit(&state, addr.ip()) {
            return resp;
        }
        if authorize_request(&state, &query, &headers).is_err() {
            record_auth_failure(&state, addr.ip());
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "ok": false, "error": "Invalid or missing pairing token" })),
            )
                .into_response();
        }
        clear_auth_failures(&state, addr.ip());
    }

    Json(json!({
        "hostname": state.tailscale.hostname,
        "tailscale_ip": state.tailscale.ipv4,
        "tailscale_ipv6": state.tailscale.ipv6,
        "dns_name": state.tailscale.dns_name,
        "os": std::env::consts::OS,
        "port": state.port,
        "auth_enabled": state.auth_enabled,
        "allow_lan": state.allow_lan,
        "dry_run": state.dry_run,
        "connected_clients": state.connected_list(),
    }))
    .into_response()
}

async fn api_rotate_token(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if !is_loopback(addr.ip()) {
        return (
            StatusCode::FORBIDDEN,
            "Token rotation is restricted to localhost.",
        )
            .into_response();
    }
    if let Err(resp) = check_loopback_host(&headers) {
        return resp;
    }
    if let Err(resp) = check_same_origin(&headers, false) {
        return resp;
    }
    let csrf_ok = headers
        .get(CSRF_HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim() == "1")
        .unwrap_or(false);
    if !csrf_ok {
        return forbidden_response("Missing required CSRF header.");
    }

    if state.auth_enabled {
        let new_tok = qr::generate_token(12);
        if let Ok(mut guard) = state.token.write() {
            *guard = new_tok;
        }
        state.token_epoch.send_modify(|e| *e = e.wrapping_add(1));
    }
    let pairing_url = state.pairing_url();
    state.broadcast_snapshot();

    Json(json!({
        "ok": true,
        "pairing_url": pairing_url,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct AddShortcutPayload {
    name: String,
    target: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    color: Option<String>,
}

async fn api_get_shortcuts(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let list = state
        .shortcuts
        .read()
        .map(|g| g.clone())
        .unwrap_or_default();
    Json(list).into_response()
}

async fn api_add_shortcut(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AddShortcutPayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let kind = payload.kind.unwrap_or_else(|| "url".to_string());
    let updated = {
        let Ok(mut guard) = state.shortcuts.write() else {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Shortcuts lock poisoned").into_response();
        };
        if let Err(err) = system::add_shortcut_to_list(
            &mut guard,
            &payload.name,
            &kind,
            &payload.target,
            payload.icon.as_deref(),
            payload.color.as_deref(),
        ) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": err })),
            )
                .into_response();
        }
        guard.clone()
    };
    let _ = tokio::task::spawn_blocking({
        let snapshot = updated.clone();
        move || system::save_shortcuts(&snapshot)
    })
    .await;
    Json(updated).into_response()
}

async fn api_delete_shortcut(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let updated = {
        let Ok(mut guard) = state.shortcuts.write() else {
            return (StatusCode::INTERNAL_SERVER_ERROR, "Shortcuts lock poisoned").into_response();
        };
        guard.retain(|item| item.id != id);
        guard.clone()
    };
    let _ = tokio::task::spawn_blocking({
        let snapshot = updated.clone();
        move || system::save_shortcuts(&snapshot)
    })
    .await;
    Json(updated).into_response()
}

async fn api_reset_shortcuts(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let defaults = system::default_shortcuts();
    if let Ok(mut guard) = state.shortcuts.write() {
        *guard = defaults.clone();
    }
    let _ = tokio::task::spawn_blocking({
        let snapshot = defaults.clone();
        move || system::save_shortcuts(&snapshot)
    })
    .await;
    Json(defaults).into_response()
}

#[derive(Deserialize)]
struct LaunchPayload {
    kind: String,
    target: String,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    app_mode: Option<bool>,
}

async fn api_launch(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LaunchPayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }

    let dry_run = state.dry_run;
    let app_mode = payload.app_mode.unwrap_or(false);
    let kind = payload.kind.trim().to_ascii_lowercase();
    let target = payload.target;
    let engine = payload.engine.unwrap_or_else(|| "youtube".to_string());

    let res = tokio::task::spawn_blocking(move || match kind.as_str() {
        "url" => system::launch_url(&target, app_mode, dry_run),
        "search" => {
            let search_url = system::build_search_url(&engine, &target)?;
            system::launch_url(&search_url, app_mode, dry_run)
        }
        "preset" => system::launch_preset(&target, dry_run),
        other => Err(format!("Unsupported launch kind '{other}'")),
    })
    .await
    .unwrap_or_else(|e| Err(format!("Launch task failed: {e}")));

    match res {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
    }
}

async fn api_audio(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let audio = tokio::task::spawn_blocking(system::get_audio_state)
        .await
        .unwrap_or(system::AudioState {
            volume: None,
            muted: false,
            outputs: Vec::new(),
        });
    Json(audio).into_response()
}

#[derive(Deserialize)]
struct VolumePayload {
    volume: u8,
}

async fn api_audio_volume(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<VolumePayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let dry_run = state.dry_run;
    let vol = payload.volume;
    match tokio::task::spawn_blocking(move || system::set_volume(vol, dry_run)).await {
        Ok(Ok(applied)) => Json(json!({ "ok": true, "volume": applied })).into_response(),
        Ok(Err(err)) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct MutePayload {
    #[serde(default)]
    muted: Option<bool>,
}

async fn api_audio_mute(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<MutePayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let dry_run = state.dry_run;
    let muted = payload.muted;
    match tokio::task::spawn_blocking(move || system::set_mute(muted, dry_run)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(err)) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct OutputPayload {
    id: String,
}

async fn api_audio_output(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<OutputPayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let dry_run = state.dry_run;
    let id = payload.id;
    match tokio::task::spawn_blocking(move || system::set_audio_output(&id, dry_run)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn api_now_playing(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let np = tokio::task::spawn_blocking(system::get_now_playing)
        .await
        .unwrap_or(system::NowPlaying {
            status: "None".to_string(),
            title: None,
            artist: None,
            player: None,
        });
    Json(np).into_response()
}

#[derive(Deserialize, Default)]
struct ScreenQuery {
    #[serde(default)]
    t: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

async fn api_screen(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(sq): Query<ScreenQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let auth = AuthQuery { t: sq.t };
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &auth, &headers) {
        return resp;
    }
    let cursor_zoom = sq.mode.as_deref() == Some("cursor");
    match tokio::task::spawn_blocking(move || system::capture_screen_jpeg(cursor_zoom)).await {
        Ok(Ok(bytes)) => (
            StatusCode::OK,
            [
                (CONTENT_TYPE, "image/jpeg"),
                (CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(err)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        )
            .into_response(),
    }
}

fn sleep_timer_json(state: &AppState) -> serde_json::Value {
    let now_ms = now_epoch_ms();
    if let Ok(guard) = state.sleep_timer.lock() {
        if let Some(ref active) = *guard {
            let rem_ms = active.ends_at_ms.saturating_sub(now_ms);
            return json!({
                "active": true,
                "remaining_secs": (rem_ms + 999) / 1000,
                "ends_at_ms": active.ends_at_ms,
                "action": active.action,
            });
        }
    }
    json!({
        "active": false,
        "remaining_secs": 0,
        "ends_at_ms": null,
        "action": null,
    })
}

async fn api_get_sleep_timer(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    Json(sleep_timer_json(&state)).into_response()
}

#[derive(Deserialize)]
struct SleepTimerPayload {
    minutes: u64,
    #[serde(default)]
    action: Option<String>,
}

async fn api_set_sleep_timer(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SleepTimerPayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    if payload.minutes == 0 || payload.minutes > 720 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "Sleep timer minutes must be between 1 and 720" })),
        )
            .into_response();
    }
    let action = payload
        .action
        .unwrap_or_else(|| "sleep".to_string())
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        action.as_str(),
        "sleep" | "display_off" | "pause_only" | "shutdown"
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": format!("Invalid sleep timer action '{action}'") })),
        )
            .into_response();
    }

    let ends_at_ms = now_epoch_ms().saturating_add(payload.minutes.saturating_mul(60_000));
    let state_clone = state.clone();
    let action_clone = action.clone();
    let delay = Duration::from_secs(payload.minutes.saturating_mul(60));

    let handle = tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        // Pause active media playback first (KEY_PLAYPAUSE = 164)
        let _ = state_clone
            .input_tx
            .send(InputCommand::Key {
                scancode: 164,
                down: None,
                mods: Vec::new(),
            })
            .await;
        tokio::time::sleep(Duration::from_secs(1)).await;

        if action_clone != "pause_only" {
            let dry_run = state_clone.dry_run;
            let act = action_clone.clone();
            let _ = tokio::task::spawn_blocking(move || system::execute_power_action(&act, dry_run))
                .await;
        }

        if let Ok(mut guard) = state_clone.sleep_timer.lock() {
            if guard
                .as_ref()
                .map(|t| t.ends_at_ms == ends_at_ms)
                .unwrap_or(false)
            {
                *guard = None;
            }
        }
    });

    if let Ok(mut guard) = state.sleep_timer.lock() {
        if let Some(prev) = guard.take() {
            prev.task.abort();
        }
        *guard = Some(ActiveSleepTimer {
            ends_at_ms,
            action,
            task: handle,
        });
    }

    Json(sleep_timer_json(&state)).into_response()
}

async fn api_delete_sleep_timer(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    if let Ok(mut guard) = state.sleep_timer.lock() {
        if let Some(prev) = guard.take() {
            prev.task.abort();
        }
    }
    Json(sleep_timer_json(&state)).into_response()
}

#[derive(Deserialize)]
struct PowerPayload {
    action: String,
}

async fn api_power(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PowerPayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let dry_run = state.dry_run;
    let action = payload.action;
    match tokio::task::spawn_blocking(move || system::execute_power_action(&action, dry_run)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn api_peers(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let port = state.port;
    let self_url = format!("http://{}:{}/", state.tailscale.ipv4, state.port);
    let peers = tokio::task::spawn_blocking(move || system::probe_tailnet_peers(port, &self_url))
        .await
        .unwrap_or_default();
    Json(peers).into_response()
}

async fn api_get_windows(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let windows = tokio::task::spawn_blocking(system::list_open_windows)
        .await
        .unwrap_or_default();
    Json(windows).into_response()
}

#[derive(Deserialize)]
struct WindowFocusPayload {
    id: String,
}

async fn api_focus_window(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<WindowFocusPayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let dry_run = state.dry_run;
    let id = payload.id;
    match tokio::task::spawn_blocking(move || system::focus_window(&id, dry_run)).await {
        Ok(Ok(label)) => Json(json!({ "ok": true, "focused": label })).into_response(),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
struct WindowClosePayload {
    #[serde(default)]
    id: Option<String>,
}

async fn api_close_window(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    payload: Option<Json<WindowClosePayload>>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }
    let dry_run = state.dry_run;
    let target_id = payload
        .and_then(|Json(p)| p.id)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let res = tokio::task::spawn_blocking(move || match target_id {
        Some(id) => system::close_window(&id, dry_run),
        None => system::close_window_under_cursor(dry_run),
    })
    .await;

    match res {
        Ok(Ok(label)) => Json(json!({ "ok": true, "closed": label })).into_response(),
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct WorkspacePayload {
    dir: String,
}

async fn api_switch_workspace(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<WorkspacePayload>,
) -> Response {
    if let Err(resp) = require_remote_auth(&state, addr.ip(), &query, &headers) {
        return resp;
    }

    let dry_run = state.dry_run;
    let dir = payload.dir.trim().to_ascii_lowercase();
    let dir_clone = dir.clone();

    let res = tokio::task::spawn_blocking(move || system::switch_workspace(&dir_clone, dry_run))
        .await;

    match res {
        Ok(Ok(true)) => Json(json!({ "ok": true, "dir": dir })).into_response(),
        Ok(Ok(false)) => {
            let arrow = if dir == "next" { 106 } else { 105 }; // KEY_RIGHT : KEY_LEFT
            #[cfg(windows)]
            let mods = vec![125, 29]; // Win + Ctrl
            #[cfg(target_os = "macos")]
            let mods = vec![29]; // Ctrl
            #[cfg(not(any(windows, target_os = "macos")))]
            let mods = vec![29, 56]; // Ctrl + Alt

            let _ = state
                .input_tx
                .send(InputCommand::Key {
                    scancode: arrow,
                    down: None,
                    mods,
                })
                .await;
            Json(json!({ "ok": true, "dir": dir })).into_response()
        }
        Ok(Err(err)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": err })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        )
            .into_response(),
    }
}

async fn ws_host(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    if !is_loopback(addr.ip()) {
        return (
            StatusCode::FORBIDDEN,
            "Host WebSocket is restricted to localhost.",
        )
            .into_response();
    }
    if let Err(resp) = check_loopback_host(&headers) {
        return resp;
    }
    if let Err(resp) = check_same_origin(&headers, true) {
        return resp;
    }
    ws.max_message_size(MAX_BODY_BYTES)
        .max_frame_size(MAX_BODY_BYTES)
        .on_upgrade(move |socket| handle_host_socket(socket, state))
}

async fn handle_host_socket(mut socket: WebSocket, state: Arc<AppState>) {
    if let Ok(json_str) = serde_json::to_string(&state.snapshot()) {
        if socket.send(Message::Text(json_str.into())).await.is_err() {
            return;
        }
    }

    let mut rx = state.host_tx.subscribe();
    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(snapshot) => {
                        if let Ok(json_str) = serde_json::to_string(&snapshot) {
                            if socket.send(Message::Text(json_str.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Ok(json_str) = serde_json::to_string(&state.snapshot()) {
                            if socket.send(Message::Text(json_str.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

async fn ws_remote(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<AuthQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    if let Err(resp) = check_tailnet_access(&state, addr.ip()) {
        return resp;
    }
    if let Err(resp) = check_allowed_host(&state, &headers) {
        return resp;
    }
    if let Err(resp) = check_same_origin(&headers, false) {
        return resp;
    }
    if let Err(resp) = check_auth_rate_limit(&state, addr.ip()) {
        return resp;
    }
    if authorize_request(&state, &query, &headers).is_err() {
        record_auth_failure(&state, addr.ip());
        return (StatusCode::UNAUTHORIZED, "Invalid or missing pairing token.").into_response();
    }
    clear_auth_failures(&state, addr.ip());
    ws.max_message_size(MAX_BODY_BYTES)
        .max_frame_size(MAX_BODY_BYTES)
        .on_upgrade(move |socket| handle_remote_socket(socket, addr, state))
}

async fn handle_remote_socket(mut socket: WebSocket, addr: SocketAddr, state: Arc<AppState>) {
    let client_id = state.next_client_id.fetch_add(1, Ordering::Relaxed);
    let ip_str = normalize_ip(addr.ip()).to_string();
    let label = state.tailscale.lookup_peer_label(addr.ip());
    let mut epoch_rx = state.token_epoch.subscribe();

    {
        if let Ok(mut guard) = state.clients.write() {
            guard.insert(
                client_id,
                ConnectedClient {
                    id: client_id,
                    ip: ip_str.clone(),
                    label: label.clone(),
                },
            );
        }
    }
    eprintln!("[+] Phone connected: {label} ({ip_str})");
    state.broadcast_snapshot();

    let hello = ServerMsg::Hello {
        hostname: state.tailscale.hostname.clone(),
        os: std::env::consts::OS.to_string(),
        tailscale_ip: state.tailscale.ipv4.clone(),
        dry_run: state.dry_run,
    };
    if let Ok(hello_json) = serde_json::to_string(&hello) {
        if socket.send(Message::Text(hello_json.into())).await.is_err() {
            cleanup_remote_client(client_id, &label, &state).await;
            return;
        }
    }

    loop {
        tokio::select! {
            frame = socket.recv() => {
                match frame {
                    Some(Ok(Message::Text(txt))) => {
                        let Ok(msg) = serde_json::from_str::<ClientMsg>(txt.as_str()) else {
                            let err_json = serde_json::to_string(&ServerMsg::Error {
                                message: "malformed command payload".to_string(),
                            })
                            .unwrap_or_default();
                            let _ = socket.send(Message::Text(err_json.into())).await;
                            continue;
                        };

                        if let Some(reply) = dispatch_client_msg(&state.input_tx, msg).await {
                            if let Ok(reply_json) = serde_json::to_string(&reply) {
                                if socket.send(Message::Text(reply_json.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
            res = epoch_rx.changed(), if state.auth_enabled => {
                if res.is_ok() {
                    let err_json = serde_json::to_string(&ServerMsg::Error {
                        message: "pairing token rotated; please re-scan QR code".to_string(),
                    })
                    .unwrap_or_default();
                    let _ = socket.send(Message::Text(err_json.into())).await;
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }

    cleanup_remote_client(client_id, &label, &state).await;
}

async fn cleanup_remote_client(client_id: u64, label: &str, state: &AppState) {
    if let Ok(mut guard) = state.clients.write() {
        guard.remove(&client_id);
    }
    let _ = state.input_tx.send(InputCommand::ReleaseAll).await;
    eprintln!("[-] Phone disconnected: {label}");
    state.broadcast_snapshot();
}

async fn dispatch_client_msg(
    input_tx: &mpsc::Sender<InputCommand>,
    msg: ClientMsg,
) -> Option<ServerMsg> {
    match msg {
        ClientMsg::Move { dx, dy } => {
            let cx = dx.clamp(-MAX_DELTA_MOVE, MAX_DELTA_MOVE);
            let cy = dy.clamp(-MAX_DELTA_MOVE, MAX_DELTA_MOVE);
            if cx != 0 || cy != 0 {
                let _ = input_tx.try_send(InputCommand::MouseMove(cx, cy));
            }
            None
        }
        ClientMsg::Click { btn, down } => {
            let norm_btn = btn.trim().to_ascii_lowercase();
            if !matches!(
                norm_btn.as_str(),
                "left" | "right" | "middle" | "back" | "forward"
            ) {
                return Some(ServerMsg::Error {
                    message: format!("unknown mouse button: {btn}"),
                });
            }
            let _ = input_tx
                .send(InputCommand::MouseClick {
                    btn: norm_btn,
                    down,
                })
                .await;
            None
        }
        ClientMsg::Scroll { dx, dy } => {
            let cx = dx.clamp(-MAX_DELTA_SCROLL, MAX_DELTA_SCROLL);
            let cy = dy.clamp(-MAX_DELTA_SCROLL, MAX_DELTA_SCROLL);
            if cx != 0 || cy != 0 {
                let _ = input_tx.try_send(InputCommand::MouseScroll(cx, cy));
            }
            None
        }
        ClientMsg::Key { code, down, mods } => {
            let Some(scancode) = keymap::web_key_to_evdev(&code) else {
                return Some(ServerMsg::Error {
                    message: format!("unknown key code: {code}"),
                });
            };
            let mut mod_codes: Vec<u16> = Vec::with_capacity(4);
            for m in mods.unwrap_or_default() {
                if let Some(sc) = keymap::web_key_to_evdev(&m) {
                    if matches!(sc, 29 | 97 | 42 | 54 | 56 | 100 | 125 | 126)
                        && !mod_codes.contains(&sc)
                    {
                        mod_codes.push(sc);
                        if mod_codes.len() >= 4 {
                            break;
                        }
                    }
                }
            }
            let _ = input_tx
                .send(InputCommand::Key {
                    scancode,
                    down,
                    mods: mod_codes,
                })
                .await;
            None
        }
        ClientMsg::Text { mut text, enter } => {
            if text.len() > MAX_TEXT_BYTES {
                let mut end = MAX_TEXT_BYTES;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            let _ = input_tx
                .send(InputCommand::TypeText {
                    text,
                    enter: enter.unwrap_or(false),
                })
                .await;
            None
        }
        ClientMsg::Clipboard { mut text, paste } => {
            if text.len() > MAX_TEXT_BYTES * 4 {
                let mut end = MAX_TEXT_BYTES * 4;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            let _ = input_tx
                .send(InputCommand::Clipboard {
                    text,
                    paste: paste.unwrap_or(true),
                })
                .await;
            None
        }
        ClientMsg::Media { action } => {
            let Some(scancode) = keymap::media_action_to_evdev(&action) else {
                return Some(ServerMsg::Error {
                    message: format!("unknown media action: {action}"),
                });
            };
            let _ = input_tx
                .send(InputCommand::Key {
                    scancode,
                    down: None,
                    mods: Vec::new(),
                })
                .await;
            None
        }
        ClientMsg::Ping { ts } => Some(ServerMsg::Pong { ts }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::spawn_input_worker;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    fn test_state(token: &str) -> Arc<AppState> {
        let input_tx = spawn_input_worker(true).expect("dry-run input worker");
        let ts = TailscaleInfo {
            ipv4: "100.64.0.10".to_string(),
            ipv6: None,
            hostname: "media-pc".to_string(),
            dns_name: Some("media-pc.tailnet.ts.net".to_string()),
            peers: HashMap::new(),
        };
        AppState::new(token.to_string(), true, false, true, 9880, false, ts, input_tx)
    }

    async fn send_http(addr: SocketAddr, req: &str) -> (u16, String, String) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let full = String::from_utf8_lossy(&buf).into_owned();
        let (head, body) = full.split_once("\r\n\r\n").unwrap_or((&full, ""));
        let status_line = head.lines().next().unwrap_or("");
        let code = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(0);
        (code, head.to_string(), body.to_string())
    }

    #[tokio::test]
    async fn test_http_endpoints_and_token_rotation() {
        let state = test_state("SecretTok123");
        let app = build_router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });

        // 1. /qr returns 200 with SVG on loopback
        let (code, _, body) = send_http(
            addr,
            &format!("GET /qr HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("<svg"));
        assert!(body.contains("SecretTok123"));

        // 2. / without token returns 401
        let (code, _, _) = send_http(
            addr,
            &format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert_eq!(code, 401);

        // 3. / with valid token returns 200 + Set-Cookie
        let (code, head, body) = send_http(
            addr,
            &format!("GET /?t=SecretTok123 HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert_eq!(code, 200);
        assert!(head.to_ascii_lowercase().contains("set-cookie: tsr_token=secrettok123")
            || head.contains("Set-Cookie: tsr_token=SecretTok123"));
        assert!(body.contains("Tailscale Remote"));

        // 4. / with cookie works without query token
        let (code, _, _) = send_http(
            addr,
            &format!(
                "GET / HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=SecretTok123\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 200);

        // 5. /api/status returns JSON without exposing token
        let (code, _, body) = send_http(
            addr,
            &format!("GET /api/status HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("\"hostname\":\"media-pc\""));
        assert!(!body.contains("SecretTok123"));

        // 6. POST /api/token/rotate rotates token (with X-TSR-CSRF header) and invalidates old token
        let (code, _, body) = send_http(
            addr,
            &format!(
                "POST /api/token/rotate HTTP/1.1\r\nHost: {addr}\r\nX-TSR-CSRF: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("\"ok\":true"));
        assert_ne!(state.current_token(), "SecretTok123");

        let (code, _, _) = send_http(
            addr,
            &format!("GET /?t=SecretTok123 HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert_eq!(code, 401);

        server_task.abort();
    }

    #[tokio::test]
    async fn test_feature_apis_auth_launch_timer_and_power() {
        let state = test_state("FeatTok99");
        let app = build_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });

        // Unauthenticated requests to new APIs must fail with 401
        for path in [
            "/api/shortcuts",
            "/api/audio",
            "/api/now-playing",
            "/api/sleep-timer",
            "/api/peers",
        ] {
            let (code, _, _) = send_http(
                addr,
                &format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
            )
            .await;
            assert_eq!(code, 401, "expected 401 for unauthenticated GET {path}");
        }

        // Authenticated GET /api/shortcuts returns YouTube & Browser defaults
        let (code, _, body) = send_http(
            addr,
            &format!(
                "GET /api/shortcuts HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("YouTube"));
        assert!(body.contains("https://www.youtube.com"));

        // Authenticated POST /api/launch succeeds on search & rejects file://
        let launch_ok = r#"{"kind":"search","engine":"youtube","target":"rust programming"}"#;
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/launch HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{launch_ok}",
                launch_ok.len()
            ),
        )
        .await;
        assert_eq!(code, 200);

        let launch_bad = r#"{"kind":"url","target":"file:///etc/passwd"}"#;
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/launch HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{launch_bad}",
                launch_bad.len()
            ),
        )
        .await;
        assert_eq!(code, 400);

        // Sleep timer create -> inspect -> cancel lifecycle
        let timer_body = r#"{"minutes":30,"action":"sleep"}"#;
        let (code, _, body) = send_http(
            addr,
            &format!(
                "POST /api/sleep-timer HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{timer_body}",
                timer_body.len()
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("\"active\":true"));

        let (code, _, body) = send_http(
            addr,
            &format!(
                "DELETE /api/sleep-timer HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("\"active\":false"));

        // Power action allowlist under dry-run
        let pwr_ok = r#"{"action":"display_off"}"#;
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/power HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{pwr_ok}",
                pwr_ok.len()
            ),
        )
        .await;
        assert_eq!(code, 200);

        let pwr_bad = r#"{"action":"wipe_disk"}"#;
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/power HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{pwr_bad}",
                pwr_bad.len()
            ),
        )
        .await;
        assert_eq!(code, 400);

        // Window endpoints require auth & handle unknown IDs cleanly
        let (code, _, _) = send_http(
            addr,
            &format!("GET /api/windows HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert_eq!(code, 401);

        let (code, _, body) = send_http(
            addr,
            &format!(
                "GET /api/windows HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.trim_start().starts_with('['));

        let win_bad = r#"{"id":"0x000000"}"#;
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/windows/focus HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{win_bad}",
                win_bad.len()
            ),
        )
        .await;
        assert_eq!(code, 400);

        // Workspace switch requires auth and validates direction
        let ws_ok = r#"{"dir":"next"}"#;
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/workspace HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ws_ok}",
                ws_ok.len()
            ),
        )
        .await;
        assert_eq!(code, 401);

        let (code, _, body) = send_http(
            addr,
            &format!(
                "POST /api/workspace HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ws_ok}",
                ws_ok.len()
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert!(body.contains("\"ok\":true"));

        let ws_bad = r#"{"dir":"sideways"}"#;
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/workspace HTTP/1.1\r\nHost: {addr}\r\nCookie: tsr_token=FeatTok99\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ws_bad}",
                ws_bad.len()
            ),
        )
        .await;
        assert_eq!(code, 400);

        server_task.abort();
    }

    #[tokio::test]
    async fn test_non_loopback_forbidden_on_qr_and_rotate() {
        let state = test_state("Tok123456789");
        let non_loopback: SocketAddr = "100.80.10.5:51234".parse().unwrap();

        let resp_qr = qr_page(
            ConnectInfo(non_loopback),
            HeaderMap::new(),
            State(state.clone()),
        )
        .await;
        assert_eq!(resp_qr.status(), StatusCode::FORBIDDEN);

        let resp_rot = api_rotate_token(
            ConnectInfo(non_loopback),
            HeaderMap::new(),
            State(state),
        )
        .await;
        assert_eq!(resp_rot.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_security_cswsh_dns_rebinding_csrf_and_headers() {
        let state = test_state("SecAuditTok1");
        let app = build_router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });

        // 1. /ws/host rejected (403) when Origin is missing or cross-origin
        let (code, _, _) = send_http(
            addr,
            &format!(
                "GET /ws/host HTTP/1.1\r\nHost: {addr}\r\nConnection: close, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 403);

        let (code, _, _) = send_http(
            addr,
            &format!(
                "GET /ws/host HTTP/1.1\r\nHost: {addr}\r\nOrigin: https://evil.com\r\nConnection: close, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 403);

        // 2. DNS rebinding rejected (403) on /qr and / when Host is evil.com
        let (code, _, _) = send_http(
            addr,
            "GET /qr HTTP/1.1\r\nHost: evil.com:9880\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert_eq!(code, 403);

        let (code, _, _) = send_http(
            addr,
            "GET /?t=SecAuditTok1 HTTP/1.1\r\nHost: evil.com:9880\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert_eq!(code, 403);

        // 3. CSRF rejected (403) on POST /api/token/rotate when missing X-TSR-CSRF or cross-origin
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/token/rotate HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 403);

        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/token/rotate HTTP/1.1\r\nHost: {addr}\r\nOrigin: https://evil.com\r\nX-TSR-CSRF: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 403);

        // 4. POST /api/token/rotate advances token_epoch so active /ws/remote sessions are revoked
        let epoch_rx = state.token_epoch.subscribe();
        let before_epoch = *epoch_rx.borrow();
        let (code, _, _) = send_http(
            addr,
            &format!(
                "POST /api/token/rotate HTTP/1.1\r\nHost: {addr}\r\nOrigin: http://{addr}\r\nX-TSR-CSRF: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(*epoch_rx.borrow(), before_epoch + 1);
        let current_tok = state.current_token();

        // 5. Security headers present on /qr and /
        for path in ["/qr".to_string(), format!("/?t={current_tok}")] {
            let (code, head, _) = send_http(
                addr,
                &format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
            )
            .await;
            assert_eq!(code, 200);
            let hl = head.to_ascii_lowercase();
            assert!(hl.contains("x-frame-options: deny"), "missing X-Frame-Options on {path}");
            assert!(hl.contains("x-content-type-options: nosniff"), "missing nosniff on {path}");
            assert!(hl.contains("referrer-policy: no-referrer"), "missing Referrer-Policy on {path}");
            assert!(hl.contains("cache-control: no-store"), "missing Cache-Control on {path}");
            assert!(hl.contains("content-security-policy:"), "missing CSP on {path}");
        }

        // 6. X-TSR-Token header authenticates /api/shortcuts without cookie or ?t=
        let (code, _, _) = send_http(
            addr,
            &format!(
                "GET /api/shortcuts HTTP/1.1\r\nHost: {addr}\r\nX-TSR-Token: {current_tok}\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(code, 200);

        server_task.abort();
    }

    #[tokio::test]
    async fn test_api_status_non_loopback_auth_and_rate_limit() {
        let state = test_state("RateTok987");
        let tailnet_ip: SocketAddr = "100.80.10.5:51234".parse().unwrap();

        let mut host_headers = HeaderMap::new();
        host_headers.insert(HOST, HeaderValue::from_static("100.64.0.10:9880"));

        // Unauthenticated non-loopback /api/status returns 401
        let resp = api_status(
            ConnectInfo(tailnet_ip),
            Query(AuthQuery::default()),
            host_headers.clone(),
            State(state.clone()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Authenticated non-loopback /api/status returns 200 (and clears failure count)
        let resp_ok = api_status(
            ConnectInfo(tailnet_ip),
            Query(AuthQuery {
                t: Some("RateTok987".to_string()),
            }),
            host_headers.clone(),
            State(state.clone()),
        )
        .await;
        assert_eq!(resp_ok.status(), StatusCode::OK);

        // Exceeding 20 failed auth attempts from an IP triggers 429 Too Many Requests
        let brute_ip: IpAddr = "100.80.10.99".parse().unwrap();
        for i in 0..AUTH_MAX_FAILURES {
            let err = require_remote_auth(
                &state,
                brute_ip,
                &AuthQuery {
                    t: Some(format!("wrong-{i}")),
                },
                &host_headers,
            )
            .unwrap_err();
            assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        }
        let limited = require_remote_auth(
            &state,
            brute_ip,
            &AuthQuery {
                t: Some("RateTok987".to_string()),
            },
            &host_headers,
        )
        .unwrap_err();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_dispatch_client_msg_validation() {
        let input_tx = spawn_input_worker(true).expect("dry-run input worker");

        // Invalid mouse button is rejected
        let bad_click = dispatch_client_msg(
            &input_tx,
            ClientMsg::Click {
                btn: "arbitrary_btn".to_string(),
                down: None,
            },
        )
        .await;
        assert!(matches!(bad_click, Some(ServerMsg::Error { .. })));

        // Valid mouse button succeeds
        let ok_click = dispatch_client_msg(
            &input_tx,
            ClientMsg::Click {
                btn: "Left".to_string(),
                down: None,
            },
        )
        .await;
        assert!(ok_click.is_none());

        // Non-modifier keys in `mods` are stripped and duplicates/count > 4 are capped
        let res = dispatch_client_msg(
            &input_tx,
            ClientMsg::Key {
                code: "KeyC".to_string(),
                down: None,
                mods: Some(vec![
                    "KeyA".to_string(),
                    "ctrl".to_string(),
                    "ctrl".to_string(),
                    "alt".to_string(),
                    "shift".to_string(),
                    "super".to_string(),
                    "AltRight".to_string(),
                ]),
            },
        )
        .await;
        assert!(res.is_none());
    }
}
