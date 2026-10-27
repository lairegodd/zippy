use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows::Win32::Foundation::HWND;
use windows::Win32::Security::Cryptography::{BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

use crate::hub::{self, Control, Phase};
use crate::settings::Settings;
use crate::stats::Stat;

const MAX_CONNECTIONS: usize = 16;
const MAX_HEAD: usize = 8 * 1024;
const MAX_BODY: usize = 2 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const LOG_LINES_SENT: usize = 150;
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self'; \
                   connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

const INDEX: &str = include_str!("../web/index.html");
const STYLE: &str = include_str!("../web/app.css");
const SCRIPT: &str = include_str!("../web/app.js");
const FONT: &[u8] = include_bytes!("../web/nunito.woff2");

struct Server {
    port: u16,
    token: String,
    settings_path: PathBuf,
    /// Serializes read-modify-write of settings.ini between concurrent requests.
    settings_lock: Mutex<()>,
    active: AtomicUsize,
}

/// Holds one of the limited connection slots until the request is answered.
struct Slot(Arc<Server>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self { status, content_type, body: body.into() }
    }

    fn json(status: u16, value: &Value) -> Self {
        Self::new(status, "application/json; charset=utf-8", value.to_string())
    }

    fn error(status: u16, message: &str) -> Self {
        Self::json(status, &json!({ "error": message }))
    }

    fn write(&self, stream: &mut TcpStream) -> io::Result<()> {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            413 => "Content Too Large",
            415 => "Unsupported Media Type",
            421 => "Misdirected Request",
            431 => "Request Header Fields Too Large",
            _ => "Error",
        };
        let mut head = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\
             Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
             X-Frame-Options: DENY\r\nCross-Origin-Opener-Policy: same-origin\r\n\
             Cross-Origin-Resource-Policy: same-origin\r\nContent-Security-Policy: {CSP}\r\n",
            self.status,
            self.content_type,
            self.body.len()
        );
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    unsafe { BCryptGenRandom(BCRYPT_ALG_HANDLE::default(), &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(io::Error::other)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Compares secrets without stopping at the first difference.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Binds to 127.0.0.1 on a free port and returns the link that carries the key.
/// The key sits in the fragment, which browsers never send to any server or in a Referer.
pub fn start(settings_path: PathBuf) -> io::Result<String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let token = random_token()?;
    let url = format!("http://127.0.0.1:{port}/#key={token}");
    let server = Arc::new(Server {
        port,
        token,
        settings_path,
        settings_lock: Mutex::new(()),
        active: AtomicUsize::new(0),
    });
    thread::Builder::new().name("dashboard".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            if server.active.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                server.active.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            let slot = Slot(server.clone());
            let _ = thread::Builder::new().name("dashboard-request".into()).spawn(move || slot.0.serve(stream));
        }
    })?;
    Ok(url)
}

pub fn open_in_browser(url: &str) {
    unsafe {
        ShellExecuteW(HWND::default(), w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// A sender trickling one byte at a time would hold a slot forever; the whole request gets one deadline.
fn read_some(stream: &mut TcpStream, buffer: &mut [u8], deadline: Instant) -> Result<usize, u16> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(408);
    }
    stream.set_read_timeout(Some(left)).map_err(|_| 400u16)?;
    stream.read(buffer).map_err(|_| 408u16)
}

fn read_request(stream: &mut TcpStream) -> Result<Request, u16> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buffer.len() > MAX_HEAD {
            return Err(431);
        }
        let n = read_some(stream, &mut chunk, deadline)?;
        if n == 0 {
            return Err(400);
        }
        buffer.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buffer[..head_end]).map_err(|_| 400u16)?;
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().ok_or(400u16)?.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(400);
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err(400);
    }
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(400u16)?;
        let name = name.trim().to_ascii_lowercase();
        if headers.iter().any(|(n, _): &(String, String)| *n == name && (name == "host" || name == "content-length")) {
            return Err(400);
        }
        headers.push((name, value.trim().to_owned()));
    }
    if headers.iter().any(|(n, _)| n == "transfer-encoding") {
        return Err(400);
    }
    let length = match headers.iter().find(|(n, _)| n == "content-length") {
        Some((_, v)) => v.parse::<usize>().map_err(|_| 400u16)?,
        None => 0,
    };
    if length > MAX_BODY {
        return Err(413);
    }
    let (method, path) = (method.to_owned(), path.to_owned());
    let mut body = buffer.split_off(head_end + 4);
    if body.len() > length {
        return Err(400);
    }
    let mut rest = vec![0u8; length - body.len()];
    let mut filled = 0;
    while filled < rest.len() {
        match read_some(stream, &mut rest[filled..], deadline)? {
            0 => return Err(400),
            n => filled += n,
        }
    }
    body.extend_from_slice(&rest);
    Ok(Request { method, path, headers, body })
}

impl Server {
    fn serve(&self, mut stream: TcpStream) {
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let response = match read_request(&mut stream) {
            Ok(request) => self.route(&request),
            Err(status) => Response::error(status, "bad request"),
        };
        let _ = response.write(&mut stream);
    }

    fn own_origins(&self) -> [String; 2] {
        [format!("127.0.0.1:{}", self.port), format!("localhost:{}", self.port)]
    }

    /// A page on another site can point a name at 127.0.0.1 (DNS rebinding); its Host header gives it away.
    fn host_ok(&self, request: &Request) -> bool {
        request.header("host").is_some_and(|host| self.own_origins().iter().any(|o| o.eq_ignore_ascii_case(host)))
    }

    /// The page keeps the key in sessionStorage, which belongs to this port alone, and sends it as a header.
    /// A cookie would not do: browsers send cookies for 127.0.0.1 to every port on it.
    fn signed_in(&self, request: &Request) -> bool {
        request
            .header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|key| same_secret(key, &self.token))
    }

    /// Changes must come from the dashboard's own page, not from a form or script on another site.
    fn same_origin(&self, request: &Request) -> bool {
        let origin_ok = request.header("origin").is_some_and(|origin| {
            self.own_origins().iter().any(|o| origin.eq_ignore_ascii_case(&format!("http://{o}")))
        });
        let fetch_ok = request.header("sec-fetch-site").is_none_or(|site| site == "same-origin");
        let json = request.header("content-type").is_some_and(|t| t.starts_with("application/json"));
        origin_ok && fetch_ok && json
    }

    fn route(&self, request: &Request) -> Response {
        if !self.host_ok(request) {
            return Response::error(421, "wrong host");
        }
        let method = request.method.as_str();
        let path = request.path.as_str();
        match (method, path) {
            ("GET", "/") => return Response::new(200, "text/html; charset=utf-8", INDEX),
            ("GET", "/app.js") => return Response::new(200, "text/javascript; charset=utf-8", SCRIPT),
            ("GET", "/app.css") => return Response::new(200, "text/css; charset=utf-8", STYLE),
            ("GET", "/nunito.woff2") => return Response::new(200, "font/woff2", FONT),
            _ => {}
        }
        if !self.signed_in(request) {
            return Response::error(401, "open the dashboard link shown in the Zippy window");
        }
        match (method, path) {
            ("GET", "/api/state") => Response::json(200, &state()),
            ("GET", "/api/settings") => self.settings(),
            ("POST", "/api/control" | "/api/settings") if !self.same_origin(request) => {
                Response::error(403, "request did not come from the dashboard")
            }
            ("POST", "/api/control") => control(request),
            ("POST", "/api/settings") => self.change_setting(request),
            (_, "/api/state" | "/api/settings" | "/api/control") => Response::error(405, "method not allowed"),
            _ => Response::error(404, "not found"),
        }
    }

    fn settings(&self) -> Response {
        let _guard = self.settings_lock.lock().unwrap_or_else(PoisonError::into_inner);
        match Settings::load_or_create(&self.settings_path) {
            Ok((settings, _)) => Response::json(200, &settings_json(&settings)),
            Err(e) => Response::error(500, &e),
        }
    }

    fn change_setting(&self, request: &Request) -> Response {
        let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
            return Response::error(400, "expected JSON");
        };
        let (Some(key), Some(value)) = (body["key"].as_str(), body["value"].as_str()) else {
            return Response::error(400, "expected key and value");
        };
        if key.len() > 32 || value.len() > 32 || value.contains(['\r', '\n']) {
            return Response::error(400, "value too long");
        }
        let _guard = self.settings_lock.lock().unwrap_or_else(PoisonError::into_inner);
        let saved = Settings::load_or_create(&self.settings_path)
            .and_then(|(settings, _)| settings.with(key, value))
            .and_then(|changed| changed.save(&self.settings_path).map(|()| changed));
        match saved {
            Ok(settings) => Response::json(200, &settings_json(&settings)),
            Err(e) => Response::error(400, &e),
        }
    }
}

fn settings_json(settings: &Settings) -> Value {
    let values: Vec<Value> = settings.values().into_iter().map(|(key, value)| json!({ "key": key, "value": value })).collect();
    json!({ "values": values })
}

fn control(request: &Request) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
        return Response::error(400, "expected JSON");
    };
    // The dashboard's hidden switch; it lasts until Zippy closes, like `--debug`.
    if body["action"].as_str() == Some("developer_mode") {
        let mut hub = hub::hub();
        if !hub.debug {
            hub.debug = true;
            drop(hub);
            crate::out!("[DEBUG] Developer mode unlocked from the dashboard. F9 now simulates a captcha.");
        }
        return Response::json(200, &json!({ "debug": true }));
    }
    let debug = hub::hub().debug;
    let action = match body["action"].as_str() {
        Some("start") => Control::Start,
        Some("pause") => Control::Pause,
        Some("stop") => Control::Stop,
        Some("refresh") => Control::Refresh,
        Some("simulate_captcha") if debug => Control::SimulateCaptcha,
        Some("dump_chat") if debug => Control::DumpChat,
        _ => return Response::error(400, "unknown action"),
    };
    hub::control(action);
    Response::json(200, &json!({ "queued": body["action"] }))
}

fn state() -> Value {
    let hub = hub::hub();
    let (phase, reason) = match &hub.phase {
        Phase::Idle => ("idle", None),
        Phase::Searching => ("searching", None),
        Phase::Running => ("running", None),
        Phase::Paused(reason) => ("paused", Some(reason.clone())),
        Phase::Banned => ("banned", None),
    };
    let reading = |stat| hub.stats.get(stat).map(|r| json!({ "value": r.value, "at": r.at }));
    let reminders: Vec<Value> = [
        ("huntbot", Stat::HuntbotReadyAt),
        ("daily", Stat::DailyReadyAt),
        ("cookie", Stat::CookieReadyAt),
        ("vote", Stat::VoteReadyAt),
    ]
    .into_iter()
    .map(|(key, stat)| json!({ "key": key, "ready_at": hub.stats.get(stat).map(|r| r.value) }))
    .collect();
    let dailies: Vec<Value> = hub
        .dailies
        .iter()
        .map(|d| json!({ "command": d.command, "left": d.left, "limit": d.limit, "used_up": d.used_up, "retry_at": d.retry_at }))
        .collect();
    let skip = hub.log().count().saturating_sub(LOG_LINES_SENT);
    let log: Vec<Value> = hub.log().skip(skip).map(|(t, text)| json!({ "t": t, "text": text })).collect();
    json!({
        "version": crate::update::current_version(),
        "now": crate::unix_ms_now(),
        "phase": phase,
        "debug": hub.debug,
        "reason": reason,
        "target": hub.target.as_ref().map(|t| json!({ "app": t.app, "title": t.title })),
        "stats": {
            "cash": reading(Stat::Cash),
            "cookies": reading(Stat::Cookies),
            "luck": reading(Stat::Luck),
            "essence": reading(Stat::Essence),
            "streak": reading(Stat::Streak),
            "daily_streak": reading(Stat::DailyStreak),
            "unread_mail": reading(Stat::UnreadMail),
            "boss_tickets": reading(Stat::BossTickets),
            "boss_tickets_max": reading(Stat::BossTicketsMax),
            "prays_sent": hub.stats.prays_sent,
            "wins": hub.stats.wins,
            "losses": hub.stats.losses,
            "draws": hub.stats.draws,
        },
        "reminders": reminders,
        "dailies": dailies,
        "log_seq": hub.log_seq(),
        "log": log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server {
            port: 5000,
            token: "a".repeat(64),
            settings_path: std::env::temp_dir().join(format!("zippy-web-test-{}.ini", std::process::id())),
            settings_lock: Mutex::new(()),
            active: AtomicUsize::new(0),
        }
    }

    fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
        Request {
            method: method.to_owned(),
            path: path.to_owned(),
            headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn signed(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, String)> {
        let mut headers = vec![("host", "127.0.0.1:5000".to_owned()), ("authorization", format!("Bearer {}", "a".repeat(64)))];
        headers.extend(extra.iter().map(|(n, v)| (*n, v.to_string())));
        headers
    }

    fn call(server: &Server, method: &str, path: &str, headers: &[(&str, String)], body: &str) -> u16 {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        server.route(&request(method, path, &headers, body)).status
    }

    #[test]
    fn api_needs_the_exact_key_as_a_bearer_header() {
        let server = server();
        let host = ("host", "127.0.0.1:5000".to_owned());
        assert_eq!(call(&server, "GET", "/api/state", std::slice::from_ref(&host), ""), 401);
        for wrong in [format!("Bearer {}", "a".repeat(63)), format!("Bearer {}", "b".repeat(64)), "a".repeat(64)] {
            assert_eq!(call(&server, "GET", "/api/state", &[host.clone(), ("authorization", wrong)], ""), 401);
        }
        let as_cookie = ("cookie", format!("zippy_session={}", "a".repeat(64)));
        assert_eq!(call(&server, "GET", "/api/state", &[host.clone(), as_cookie], ""), 401, "cookies are not accepted");
        assert_eq!(call(&server, "GET", "/api/state", &signed(&[]), ""), 200);
        assert_eq!(call(&server, "GET", "/", &[host], ""), 200, "the page itself holds no secrets");
    }

    #[test]
    fn every_route_rejects_a_foreign_host() {
        let server = server();
        let rebinding = [("host", "evil.example:5000".to_owned()), ("authorization", format!("Bearer {}", "a".repeat(64)))];
        assert_eq!(call(&server, "GET", "/api/state", &rebinding, ""), 421);
        assert_eq!(call(&server, "GET", "/", &rebinding, ""), 421);
        assert_eq!(call(&server, "GET", "/api/state", &[("authorization", format!("Bearer {}", "a".repeat(64)))], ""), 421);
    }

    #[test]
    fn changes_need_same_origin_json() {
        let server = server();
        let body = r#"{"action":"refresh"}"#;
        assert_eq!(call(&server, "POST", "/api/control", &signed(&[("content-type", "application/json")]), body), 403);
        let cross_site = signed(&[("origin", "http://evil.example"), ("content-type", "application/json")]);
        assert_eq!(call(&server, "POST", "/api/control", &cross_site, body), 403);
        let form = signed(&[("origin", "http://127.0.0.1:5000"), ("content-type", "text/plain")]);
        assert_eq!(call(&server, "POST", "/api/control", &form, body), 403);
        let fetch_site =
            signed(&[("origin", "http://127.0.0.1:5000"), ("content-type", "application/json"), ("sec-fetch-site", "cross-site")]);
        assert_eq!(call(&server, "POST", "/api/control", &fetch_site, body), 403);
        let good = signed(&[("origin", "http://127.0.0.1:5000"), ("content-type", "application/json")]);
        assert_eq!(call(&server, "POST", "/api/control", &good, r#"{"action":"explode"}"#), 400);
        assert_eq!(call(&server, "POST", "/api/control", &good, r#"{"action":"simulate_captcha"}"#), 400, "developer mode is off");
        assert_eq!(call(&server, "POST", "/api/control", &good, "not json"), 400);
        assert_eq!(call(&server, "GET", "/api/control", &signed(&[]), ""), 405);
    }

    #[test]
    fn settings_changes_are_validated_before_saving() {
        let server = server();
        let good = signed(&[("origin", "http://127.0.0.1:5000"), ("content-type", "application/json")]);
        let injected = r#"{"key":"mode","value":"both\nupdate_check = off"}"#;
        assert_eq!(call(&server, "POST", "/api/settings", &good, injected), 400);
        assert_eq!(call(&server, "POST", "/api/settings", &good, r#"{"key":"gem1","value":"99"}"#), 400);
        assert_eq!(call(&server, "POST", "/api/settings", &good, r#"{"key":"mode","value":"battle"}"#), 200);
        let saved = std::fs::read_to_string(&server.settings_path).unwrap();
        assert!(saved.contains("mode = battle") && saved.contains("update_check = on"));
        let _ = std::fs::remove_file(&server.settings_path);
    }

    #[test]
    fn secrets_compare_by_value_only() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "abcd"));
        assert_eq!(random_token().unwrap().len(), 64);
        assert_ne!(random_token().unwrap(), random_token().unwrap());
    }
}
