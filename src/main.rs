#[macro_use]
mod hub;
mod daily;
mod gems;
mod menu;
mod settings;
mod stats;
mod update;
mod web;

use std::collections::VecDeque;
use std::io::Write;
use std::mem::size_of;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use daily::Dailies;
use gems::{ChatMessage, GemKeeper};
use hub::{Control, Phase};
use settings::Settings;
use stats::Watcher;
use windows::core::{w, Result, BSTR, HSTRING, PWSTR, VARIANT};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::Foundation::{CloseHandle, GetLastError, BOOL, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Diagnostics::Debug::Beep;
use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateMutexW, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, ReleaseMutex,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId, UIA_ClassNamePropertyId,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_NamePropertyId,
    UIA_ValueValuePropertyId, UIA_E_ELEMENTNOTAVAILABLE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_F10, VK_F8, VK_F9, VK_LWIN, VK_MENU, VK_RETURN,
    VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, FlashWindowEx, GetForegroundWindow, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    SetForegroundWindow, ShowWindow, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, SW_RESTORE,
};
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

/// OwO's 15 s cooldown plus 3 s for network lag.
const CYCLE: Duration = Duration::from_secs(18);
const OWO_EVERY: Duration = Duration::from_secs(11);
const PRAY_EVERY: Duration = Duration::from_secs(5 * 60 + 3);
const PRAY_COMMAND: &str = "owopray";
const OWO_COMMAND: &str = "owo";
const STARVE_AFTER: Duration = Duration::from_secs(120);
const INVENTORY_WAIT: Duration = Duration::from_secs(20);
const AUTO_STATS_MIN: Duration = Duration::from_secs(30 * 60);
const GAP_BETWEEN_COMMANDS: Duration = Duration::from_millis(2500);
const SCAN_EVERY: Duration = Duration::from_secs(4);
const SCAN_ATTEMPTS: u32 = 5;
const SCAN_RETRY_DELAY: Duration = Duration::from_millis(300);
const KEY_DELAY: Duration = Duration::from_millis(35);
const STARTUP_LOOKBACK: Duration = Duration::from_secs(30 * 60);
const CAPTCHA_MARKERS: &[&str] = &["captcha", "verifythatyouarehuman", "areyouarealhuman"];
const BAN_MARKERS: &[&str] = &["youhavebeenbanned", "macrosorbotting"];
const MESSAGE_ID_PREFIX: &str = "chat-messages-";
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;
const TOAST_APP_ID: &str = "Zippy";
const TOAST_APP_NAME: &str = "Zippy";
const BROWSERS: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "opera_gx.exe",
    "vivaldi.exe",
    "chromium.exe",
    "floorp.exe",
    "zen.exe",
];
const DESKTOP_CLIENTS: &[&str] = &["discord.exe", "discordptb.exe", "discordcanary.exe", "discorddevelopment.exe"];
/// Commands whose replies fill the dashboard's stats; each is sent once per Refresh.
const QUERIES: [&str; 5] = ["owocash", "owo cookie", "owoboss t", "owovote", "owohb"];
const MAX_DETAIL: usize = 4096;

/// OwO hides zero-width spaces and lookalike letters inside words; strip them before matching.
fn squash(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'а' => 'a',
            'е' => 'e',
            'о' | 'ο' => 'o',
            'р' => 'p',
            'с' => 'c',
            'у' => 'y',
            'х' => 'x',
            'і' => 'i',
            'ј' => 'j',
            'ѕ' => 's',
            'ԁ' => 'd',
            'һ' => 'h',
            'ν' => 'v',
            '\u{FF41}'..='\u{FF5A}' | '\u{FF10}'..='\u{FF19}' => {
                char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
            }
            _ => c,
        })
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Alarm {
    Captcha,
    Ban,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Author {
    Owo,
    Other,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum HeaderStep {
    #[default]
    Before,
    Name,
    Tag,
    Done,
}

/// Reads who wrote a message from Discord's header: the name, then a bot tag only Discord can grant.
/// A reply preview carries a name and tag of its own before the header, so only the header counts.
#[derive(Default)]
struct HeaderReader {
    step: HeaderStep,
    name: Option<String>,
    verified_app: bool,
}

impl HeaderReader {
    fn read(&mut self, class: &str, name: &str, automation_id: &str) {
        let has = |prefix: &str| class.split_whitespace().any(|c| c.starts_with(prefix));
        let timestamp = automation_id.starts_with("message-timestamp-");
        self.step = match self.step {
            HeaderStep::Before if has("header_") => HeaderStep::Name,
            HeaderStep::Before if timestamp => HeaderStep::Done,
            HeaderStep::Name if has("username_") => {
                self.name = Some(name.to_owned());
                HeaderStep::Tag
            }
            HeaderStep::Tag if has("botTag") => {
                self.verified_app = name == "Verified App";
                HeaderStep::Done
            }
            HeaderStep::Name | HeaderStep::Tag if timestamp => HeaderStep::Done,
            step => step,
        };
    }
}

struct SeenMessage {
    id: u64,
    alarm: Option<Alarm>,
    header: HeaderReader,
}

/// Display names can be copied, the verified bot tag cannot. Discord drops the header on messages that
/// follow one from the same author, so those take the previous message's author; with none, it is unknown.
fn authors<'a>(headers: impl Iterator<Item = &'a HeaderReader>) -> Vec<Author> {
    let mut previous = Author::Unknown;
    headers
        .map(|header| {
            previous = match &header.name {
                Some(name) if name == "OwO" && header.verified_app => Author::Owo,
                Some(_) => Author::Other,
                None => previous,
            };
            previous
        })
        .collect()
}

fn classify(text: &str) -> Option<Alarm> {
    let s = squash(text);
    if BAN_MARKERS.iter().any(|m| s.contains(m)) {
        Some(Alarm::Ban)
    } else if CAPTCHA_MARKERS.iter().any(|m| s.contains(m)) {
        Some(Alarm::Captcha)
    } else {
        None
    }
}

fn message_id(automation_id: &str) -> Option<u64> {
    let rest = automation_id.strip_prefix(MESSAGE_ID_PREFIX)?;
    let (_channel, id) = rest.split_once('-')?;
    id.parse().ok()
}

fn unix_ms_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn snowflake_at(unix_ms: u64) -> u64 {
    unix_ms.saturating_sub(DISCORD_EPOCH_MS) << 22
}

fn snowflake_unix_ms(id: u64) -> u64 {
    (id >> 22) + DISCORD_EPOCH_MS
}

fn age(id: u64) -> String {
    let secs = unix_ms_now().saturating_sub(snowflake_unix_ms(id)) / 1000;
    match secs {
        0..=119 => format!("{secs} s ago"),
        120..=7199 => format!("{} min ago", secs / 60),
        7200..=172_799 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let split = text.len().checked_sub(suffix.len())?;
    text.get(split..)?.eq_ignore_ascii_case(suffix).then(|| &text[..split])
}

fn owo_dm_unread(name: &str) -> Option<u32> {
    let rest = strip_prefix_ignore_case(name, "owo")?.trim_start().strip_prefix(',')?.trim_start();
    let (count, label) = rest.split_once(' ')?;
    strip_prefix_ignore_case(label, "unread message")?;
    count.parse().ok()
}

fn channel_key(title: &str) -> &str {
    let mut t = title.trim_start();
    if let Some((n, rest)) = t.strip_prefix('(').and_then(|r| r.split_once(')'))
        && n.chars().all(|c| c.is_ascii_digit())
    {
        t = rest.trim_start();
    }
    t.strip_prefix('•').map_or(t, str::trim_start)
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Unpackaged apps need a registered AppUserModelID before Windows shows their toasts.
fn register_toast_app() -> Result<()> {
    let subkey = HSTRING::from(format!(r"Software\Classes\AppUserModelId\{TOAST_APP_ID}"));
    let name: Vec<u16> = TOAST_APP_NAME.encode_utf16().chain([0]).collect();
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &subkey,
            w!("DisplayName"),
            REG_SZ.0,
            Some(name.as_ptr().cast()),
            (name.len() * 2) as u32,
        )
        .ok()
    }
}

/// Alarms ring until dismissed; reminders show once with the normal sound.
fn notify(title: &str, body: &str, alarm: bool) -> Result<()> {
    let (scenario, audio) = if alarm {
        (
            r#" scenario="reminder""#,
            r#"<audio src="ms-winsoundevent:Notification.Looping.Alarm" loop="true"/>"#,
        )
    } else {
        ("", "")
    };
    let xml = format!(
        r#"<toast{scenario}><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual><actions><action content="Dismiss" arguments="dismiss" activationType="system"/></actions>{audio}</toast>"#,
        xml_escape(title),
        xml_escape(body)
    );
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(TOAST_APP_ID))?.Show(&toast)
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

fn process_exe(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        ok.ok()?;
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        Some(path.rsplit('\\').next().unwrap_or(&path).to_lowercase())
    }
}

fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                (*(lparam.0 as *mut Vec<HWND>)).push(hwnd);
            }
        }
        BOOL(1)
    }
    let mut list: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

struct MonitorDesc {
    index: usize,
    count: usize,
    primary: bool,
}

fn monitor_of(hwnd: HWND) -> MonitorDesc {
    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, lparam: LPARAM) -> BOOL {
        unsafe { (*(lparam.0 as *mut Vec<HMONITOR>)).push(m) };
        BOOL(1)
    }
    unsafe {
        let mut monitors: Vec<HMONITOR> = Vec::new();
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut monitors as *mut _ as isize));
        let target = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(target, &mut info);
        MonitorDesc {
            index: monitors.iter().position(|&m| m == target).map_or(0, |i| i + 1),
            count: monitors.len(),
            primary: info.dwFlags & 1 != 0,
        }
    }
}

/// Windows refuses SetForegroundWindow from a background process unless input is attached.
fn focus_window(hwnd: HWND) -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground == hwnd {
            return true;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let me = GetCurrentThreadId();
        let fg_thread = GetWindowThreadProcessId(foreground, None);
        let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
        sleep(Duration::from_millis(120));
        GetForegroundWindow() == hwnd
    }
}

fn key_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn send_inputs(inputs: &[INPUT]) -> bool {
    unsafe { SendInput(inputs, size_of::<INPUT>() as i32) as usize == inputs.len() }
}

/// Stops as soon as another window takes focus, so keystrokes never land in it.
fn type_text(hwnd: HWND, text: &str) -> bool {
    for unit in text.encode_utf16() {
        if unsafe { GetForegroundWindow() } != hwnd {
            return false;
        }
        let typed = send_inputs(&[
            key_input(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE),
            key_input(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
        ]);
        if !typed {
            return false;
        }
        sleep(KEY_DELAY);
    }
    true
}

fn press_enter(hwnd: HWND) -> bool {
    let focused = unsafe { GetForegroundWindow() } == hwnd;
    focused
        && send_inputs(&[
            key_input(VK_RETURN, 0, KEYBD_EVENT_FLAGS(0)),
            key_input(VK_RETURN, 0, KEYEVENTF_KEYUP),
        ])
}

/// A held Shift turns Enter into a newline, and Ctrl or Alt turn letters into shortcuts.
fn modifier_held() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|vk| unsafe { GetAsyncKeyState(vk.0 as i32) } as u16 & 0x8000 != 0)
}

/// Discord fills an empty message box with a zero-width no-break space.
fn composer_text(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || matches!(c, '\u{feff}' | '\u{200b}'))
}

fn alert(hwnd: HWND, title: &str, body: &str) {
    if let Err(e) = notify(title, body, true) {
        out!("[!] Could not send the Windows notification: {e}");
    }
    unsafe {
        let flash = FLASHWINFO {
            cbSize: size_of::<FLASHWINFO>() as u32,
            hwnd,
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        let _ = FlashWindowEx(&flash);
        for _ in 0..3 {
            let _ = Beep(1200, 250);
            sleep(Duration::from_millis(120));
        }
    }
}

struct Hotkey {
    vk: VIRTUAL_KEY,
    was_down: bool,
}

impl Hotkey {
    fn new(vk: VIRTUAL_KEY) -> Self {
        Self { vk, was_down: false }
    }
    /// The low bit catches a tap that started and ended during a long scan.
    fn pressed(&mut self) -> bool {
        let state = unsafe { GetAsyncKeyState(self.vk.0 as i32) } as u16;
        let down = state & 0x8000 != 0;
        let fired = (down || state & 1 != 0) && !self.was_down;
        self.was_down = down;
        fired
    }
}

#[derive(Clone)]
struct Uia {
    automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
}

struct PageScan {
    elements: i32,
    newest_message: Option<u64>,
    newest_captcha: Option<u64>,
    newest_ban: Option<u64>,
    owo_dm_unread: u32,
    message_box: Option<IUIAutomationElement>,
    message_box_text: String,
    url_is_discord: bool,
    messages: Vec<ChatMessage>,
}

impl Uia {
    fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let cache = automation.CreateCacheRequest()?;
            cache.AddProperty(UIA_NamePropertyId)?;
            cache.AddProperty(UIA_ControlTypePropertyId)?;
            cache.AddProperty(UIA_AutomationIdPropertyId)?;
            cache.AddProperty(UIA_ValueValuePropertyId)?;
            cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
            cache.AddProperty(UIA_ClassNamePropertyId)?;
            Ok(Self { automation, cache })
        }
    }

    /// Discord re-renders the chat after each send, so UIA briefly fails with 0x80040201.
    fn scan(&self, hwnd: HWND) -> Result<PageScan> {
        let mut attempt = 1;
        loop {
            match self.scan_once(hwnd) {
                Err(e) if attempt < SCAN_ATTEMPTS && e.code().0 as u32 == UIA_E_ELEMENTNOTAVAILABLE => {
                    attempt += 1;
                    sleep(SCAN_RETRY_DELAY);
                }
                result => return result,
            }
        }
    }

    fn scan_once(&self, hwnd: HWND) -> Result<PageScan> {
        unsafe {
            let root = self.automation.ElementFromHandle(hwnd)?;
            let all = self.automation.CreateTrueCondition()?;
            let found = root.FindAllBuildCache(TreeScope_Descendants, &all, &self.cache)?;
            let elements = found.Length()?;

            let mut scan = PageScan {
                elements,
                newest_message: None,
                newest_captcha: None,
                newest_ban: None,
                owo_dm_unread: 0,
                message_box: None,
                message_box_text: String::new(),
                url_is_discord: false,
                messages: Vec::new(),
            };
            let mut best_box: Option<(bool, i32)> = None;
            let mut summary_of: Option<u64> = None;
            let mut open: Option<usize> = None;
            let mut seen: Vec<SeenMessage> = Vec::new();
            let mut message_seen: Vec<usize> = Vec::new();

            for i in 0..elements {
                let Ok(el) = found.GetElement(i) else { continue };
                let name = el.CachedName().map(|b| b.to_string()).unwrap_or_default();
                let automation_id = el.CachedAutomationId().map(|b| b.to_string()).unwrap_or_default();
                if let Some(id) = message_id(&automation_id) {
                    scan.newest_message = scan.newest_message.max(Some(id));
                    seen.push(SeenMessage { id, alarm: classify(&name), header: HeaderReader::default() });
                    summary_of = Some(id);
                    open = None;
                    continue;
                }
                if let Some(message) = seen.last_mut() {
                    let class = el.CachedClassName().map(|b| b.to_string()).unwrap_or_default();
                    message.header.read(&class, &name, &automation_id);
                }
                if let Some(n) = owo_dm_unread(&name) {
                    scan.owo_dm_unread = scan.owo_dm_unread.saturating_add(n);
                }
                if !name.is_empty() {
                    if let Some(id) = summary_of.take() {
                        scan.messages.push(ChatMessage { id, summary: name.clone(), detail: String::new(), from_owo: false });
                        message_seen.push(seen.len() - 1);
                        open = Some(scan.messages.len() - 1);
                    } else if let Some(message) = open.and_then(|i| scan.messages.get_mut(i))
                        && message.detail.len() + name.len() < MAX_DETAIL
                    {
                        message.detail.push_str(" || ");
                        message.detail.push_str(&name);
                    }
                }
                if el.CachedControlType().ok() != Some(UIA_EditControlTypeId) {
                    continue;
                }
                let value = element_value(&el, true).unwrap_or_default();
                if is_discord_url(&value) {
                    scan.url_is_discord = true;
                    continue;
                }
                let Ok(rect) = el.CachedBoundingRectangle() else { continue };
                if rect.right <= rect.left {
                    continue;
                }
                let rank = (name.contains('#') || name.contains('@'), rect.bottom);
                if best_box.is_none_or(|b| rank > b) {
                    best_box = Some(rank);
                    scan.message_box = Some(el);
                    scan.message_box_text = value;
                }
            }
            let authors = authors(seen.iter().map(|s| &s.header));
            for (message, author) in seen.iter().zip(&authors) {
                // Someone else typing "captcha" is ignored; an author Zippy cannot tell still counts, to stay safe.
                if *author == Author::Other {
                    continue;
                }
                match message.alarm {
                    Some(Alarm::Captcha) => scan.newest_captcha = scan.newest_captcha.max(Some(message.id)),
                    Some(Alarm::Ban) => scan.newest_ban = scan.newest_ban.max(Some(message.id)),
                    None => {}
                }
            }
            for (message, &index) in scan.messages.iter_mut().zip(&message_seen) {
                message.from_owo = authors[index] == Author::Owo;
            }
            Ok(scan)
        }
    }
}

fn element_value(el: &IUIAutomationElement, cached: bool) -> Option<String> {
    let value = unsafe {
        if cached {
            el.GetCachedPropertyValue(UIA_ValueValuePropertyId)
        } else {
            el.GetCurrentPropertyValue(UIA_ValueValuePropertyId)
        }
    };
    value.ok().and_then(|v: VARIANT| BSTR::try_from(&v).ok()).map(|b| b.to_string())
}

fn is_discord_url(value: &str) -> bool {
    let rest = value.trim();
    let rest = rest.strip_prefix("https://").unwrap_or(rest);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    ["discord.com", "ptb.discord.com", "canary.discord.com"].iter().any(|host| {
        strip_prefix_ignore_case(rest, host).is_some_and(|path| path.is_empty() || path.starts_with('/'))
    })
}

struct Target {
    hwnd: HWND,
    title: String,
    exe: String,
    desktop: bool,
}

impl Target {
    fn accessibility_hint(&self) -> &'static str {
        if self.desktop {
            "Make sure a channel is open in the Discord app. If this persists, restart Discord."
        } else {
            "Make sure the Discord tab is visible. If this persists, start the browser\n    with the --force-renderer-accessibility flag."
        }
    }
}

/// Picks the frontmost Discord window: the desktop app, or a browser tab on discord.com.
fn find_discord_window(uia: &Uia) -> Option<(Target, &'static str)> {
    let mut by_title: Option<Target> = None;
    for hwnd in top_level_windows() {
        let Some(exe) = process_exe(hwnd) else { continue };
        let desktop = DESKTOP_CLIENTS.contains(&exe.as_str());
        if !desktop && !BROWSERS.contains(&exe.as_str()) {
            continue;
        }
        let title = window_title(hwnd);
        if title.is_empty() {
            continue;
        }
        if desktop {
            if is_desktop_title(&title) {
                return Some((Target { hwnd, title, exe, desktop }, "the Discord app"));
            }
            continue;
        }
        if uia.scan(hwnd).is_ok_and(|s| s.url_is_discord) {
            return Some((Target { hwnd, title, exe, desktop }, "URL discord.com"));
        }
        if by_title.is_none() && is_discord_title(&title) {
            by_title = Some(Target { hwnd, title, exe, desktop });
        }
    }
    by_title.map(|t| (t, "its Discord tab title"))
}

/// The app's main window is titled "Discord" or "<page> - Discord"; its other windows are not.
fn is_desktop_title(title: &str) -> bool {
    strip_suffix_ignore_case(channel_key(title), "discord").is_some_and(|rest| rest.is_empty() || rest.ends_with(" - "))
}

/// Discord web titles start with "Discord" (after unread badges); other pages only mention it.
fn is_discord_title(title: &str) -> bool {
    strip_prefix_ignore_case(channel_key(title), "discord")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(" |") || rest.starts_with(" -"))
}

/// Chromium stops exposing a page nobody has looked at for a while; bringing its window up wakes it.
fn wake(hwnd: HWND) {
    if modifier_held() {
        return;
    }
    let previous = unsafe { GetForegroundWindow() };
    if focus_window(hwnd) {
        sleep(Duration::from_millis(300));
        if previous != hwnd && !previous.is_invalid() {
            focus_window(previous);
        }
    }
}

fn wait_for_page(uia: &Uia, hwnd: HWND) -> Result<PageScan> {
    let mut scan = uia.scan(hwnd)?;
    for attempt in 0..20 {
        if scan.newest_message.is_some() {
            break;
        }
        if attempt == 2 {
            wake(hwnd);
        }
        sleep(Duration::from_millis(500));
        scan = uia.scan(hwnd)?;
    }
    Ok(scan)
}

fn report_target(t: &Target, reason: &str) {
    let m = monitor_of(t.hwnd);
    out!("[OK] Found Discord by {reason}");
    out!("     {} : {}", if t.desktop { "App    " } else { "Browser" }, t.exe);
    out!("     Title   : {}", t.title);
    out!("     Monitor : #{} of {}{}", m.index, m.count, if m.primary { " [primary]" } else { "" });
    hub::hub().target = Some(hub::Target { app: t.exe.clone(), title: t.title.clone() });
}

enum Sent {
    Yes,
    No,
    /// Focus moved mid-command, so part of it may be left in the message box.
    Interrupted,
    /// The box did not hold exactly the command, so Enter was not pressed.
    Unconfirmed,
}

enum State {
    Running,
    Paused,
    Banned,
}

struct Timer {
    command: &'static str,
    every: Duration,
    next: Instant,
}

fn timers(settings: &Settings, now: Instant) -> Vec<Timer> {
    let owo = settings.owo.then_some((OWO_COMMAND, OWO_EVERY));
    let pray = settings.pray.then_some((PRAY_COMMAND, PRAY_EVERY));
    owo.into_iter()
        .chain(settings.mode.commands().iter().map(|&command| (command, CYCLE)))
        .chain(pray)
        .map(|(command, every)| Timer { command, every, next: now })
        .collect()
}

fn every_text(every: Duration) -> String {
    match every.as_secs() {
        s if s < 60 => format!("{s} s"),
        s => format!("{} min", s / 60),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    Timer(usize),
    Gem,
    Daily(daily::Daily),
    Query,
}

struct Candidate {
    tier: u8,
    urgent: bool,
    due: Instant,
    job: Job,
}

/// Tier 0 is owoh, owob and the gem upkeep they depend on (gem commands first); tier 1 is owo and owopray;
/// tier 2 is stats and the daily commands. Within a tier the job waiting longest goes first, and a job
/// left waiting past `STARVE_AFTER` joins tier 0 so nothing waits forever. While `owo inv` is being
/// answered (`holding`), only tier 0 may be sent.
fn pick(candidates: impl IntoIterator<Item = Candidate>, now: Instant, holding: bool) -> Option<Job> {
    candidates
        .into_iter()
        .filter(|c| !holding || c.tier == 0)
        .min_by_key(|c| {
            let tier = if now.saturating_duration_since(c.due) > STARVE_AFTER { 0 } else { c.tier };
            (tier, !c.urgent, c.due)
        })
        .map(|c| c.job)
}

struct Zippy {
    uia: Uia,
    target: Target,
    channel: String,
    acknowledged: u64,
    dm_baseline: u32,
    newest_seen: u64,
    state: State,
    timers: Vec<Timer>,
    next_send: Instant,
    next_scan: Instant,
    unreadable_scans: u32,
    scrolled_up_reported: bool,
    gems: GemKeeper,
    gem_upkeep: bool,
    gem_queue: VecDeque<String>,
    gem_since: Instant,
    /// Until when nothing but owoh and owob may be sent, so the `owo inv` reply is read and its `owo use` goes next.
    gem_hold: Option<Instant>,
    dailies: Dailies,
    daily_since: Option<Instant>,
    watcher: Watcher,
    stats_log: stats::Log,
    queries: VecDeque<&'static str>,
    query_since: Instant,
    /// When stats are next read on their own, if `stats = on`.
    next_auto_stats: Option<Instant>,
    /// Developer mode: the next check acts as if OwO had posted a captcha.
    simulated_captcha: bool,
}

/// A random wait of 30 to 60 minutes, so the automatic stats reads don't land on a fixed beat.
fn auto_stats_wait() -> Duration {
    use std::hash::BuildHasher;
    let random = std::hash::RandomState::new().hash_one(Instant::now());
    AUTO_STATS_MIN + Duration::from_secs(random % (AUTO_STATS_MIN.as_secs() + 1))
}

impl Zippy {
    fn pause(&mut self, reason: &str, alarm: bool) {
        self.state = State::Paused;
        hub::set_phase(Phase::Paused(reason.to_owned()));
        out!("\n[{}] Zippy has Stopped Temporarily! {reason}", clock());
        out!("       Solve the captcha or go back to the original channel, then press F8 (or Start) to resume.");
        if alarm {
            alert(
                self.target.hwnd,
                "Zippy stopped!",
                &format!("{reason}\nSolve the verification, then press F8 or Start on the dashboard."),
            );
        }
    }

    fn ban(&mut self, id: u64) {
        self.state = State::Banned;
        hub::set_phase(Phase::Banned);
        out!("\n[{}] OwO ban message detected ({}). Zippy stopped.", clock(), age(id));
        out!("       Check who sent it. If it is not from OwO (fake) or the ban is over,");
        out!("       press F8 to resume. F10 to quit.");
        alert(
            self.target.hwnd,
            "OwO: BAN message detected",
            "Zippy stopped. If the ban message is fake or the ban is over, press F8 to resume.",
        );
    }

    fn resume(&mut self) {
        let confirming_ban = matches!(self.state, State::Banned);
        // The desktop app recreates its window after it restarts for an update.
        if !unsafe { IsWindow(self.target.hwnd) }.as_bool()
            && let Some((target, _)) = find_discord_window(&self.uia)
            && channel_key(&target.title) == self.channel
        {
            out!("[OK] Found the Discord window again ({}).", target.exe);
            self.target = target;
        }
        let title = window_title(self.target.hwnd);
        if channel_key(&title) != self.channel {
            out!("[!] The active tab/channel differs from the one at startup:");
            out!("    now     : {}", channel_key(&title));
            out!("    startup : {}", self.channel);
            out!("    Open the original channel again, then press F8.");
            return;
        }
        let scan = match self.uia.scan(self.target.hwnd) {
            Ok(scan) => scan,
            Err(e) => {
                out!("[!] Could not read the page: {e}");
                return;
            }
        };
        let Some(newest) = scan.newest_message else {
            out!("[!] Discord chat messages are not readable yet; try F8 again in a moment.");
            return;
        };
        // From a normal pause a new ban is shown first; only F8 in the ban state acknowledges it.
        if let Some(id) = scan.newest_ban.filter(|&id| id > self.acknowledged && !confirming_ban) {
            self.ban(id);
            return;
        }
        self.acknowledged = self
            .acknowledged
            .max(scan.newest_captcha.unwrap_or(0))
            .max(scan.newest_ban.unwrap_or(0));
        self.dm_baseline = scan.owo_dm_unread;
        self.newest_seen = newest;
        self.scrolled_up_reported = false;
        self.state = State::Running;
        hub::set_phase(Phase::Running);
        self.next_send = Instant::now();
        self.dailies.forget_pending();
        self.next_scan = Instant::now() + SCAN_EVERY;
        out!("[{}] Zippy running again.", clock());
    }

    fn check(&mut self) -> Option<PageScan> {
        let hwnd = self.target.hwnd;
        if !unsafe { IsWindow(hwnd) }.as_bool() {
            self.pause("Discord window closed.", true);
            return None;
        }
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            self.pause("Discord window is hidden (closed to the tray?). Open it again.", false);
            return None;
        }
        let title = window_title(hwnd);
        if channel_key(&title) != self.channel {
            self.pause(&format!("Tab/channel changed to \"{}\".", channel_key(&title)), false);
            return None;
        }
        let scan = match self.uia.scan(hwnd) {
            Ok(s) => s,
            Err(e) => {
                out!("[!] Could not read the page: {e}");
                return None;
            }
        };
        if let Some(id) = scan.newest_ban.filter(|&id| id > self.acknowledged) {
            self.ban(id);
            return None;
        }
        if let Some(id) = scan.newest_captcha.filter(|&id| id > self.acknowledged) {
            if hub::hub().debug
                && let Some(message) = scan.messages.iter().find(|m| m.id == id)
            {
                out!("[DEBUG] Captcha message as Zippy reads it:");
                dump_messages(std::slice::from_ref(message));
            }
            self.pause(&format!("OwO CAPTCHA message detected in the channel ({}).", age(id)), true);
            return None;
        }
        if std::mem::take(&mut self.simulated_captcha) {
            self.pause("Simulated captcha (developer mode). Nothing was posted in Discord.", true);
            return None;
        }
        if scan.owo_dm_unread > self.dm_baseline {
            self.pause(
                &format!(
                    "New DM from OwO ({} -> {} unread); probably a captcha.",
                    self.dm_baseline, scan.owo_dm_unread
                ),
                true,
            );
            return None;
        }
        self.dm_baseline = scan.owo_dm_unread;

        let Some(newest) = scan.newest_message else {
            // If no message is readable, a captcha would not be either: do not type.
            self.unreadable_scans += 1;
            if self.unreadable_scans == 3 {
                wake(hwnd);
            }
            if self.unreadable_scans == 5 {
                out!(
                    "[!] Chat messages are not readable ({} accessibility elements); waiting without typing.",
                    scan.elements
                );
                out!("    {}", self.target.accessibility_hint());
            }
            return None;
        };
        self.unreadable_scans = 0;
        if newest < self.newest_seen {
            if !self.scrolled_up_reported {
                self.scrolled_up_reported = true;
                out!("[!] The chat is scrolled up, so the newest messages are hidden. Waiting.");
                out!("    Scroll to the bottom (or press F8 twice if the newest message was deleted).");
            }
            return None;
        }
        self.scrolled_up_reported = false;
        self.newest_seen = newest;
        for action in self.gems.observe(&scan.messages) {
            let command = action.command();
            if self.gem_upkeep && !self.gem_queue.contains(&command) {
                if self.gem_queue.is_empty() {
                    self.gem_since = Instant::now();
                }
                self.gem_queue.push_back(command);
            }
        }
        let now = Instant::now();
        self.dailies.observe(&scan.messages, now, unix_ms_now());
        self.dailies.save();
        let events = self.watcher.observe(&scan.messages, self.gems.player(), now);
        self.record(events);
        hub::hub().dailies = self.dailies.view();
        Some(scan)
    }

    fn record(&mut self, events: Vec<(stats::Event, u64)>) {
        if events.is_empty() {
            return;
        }
        let changed: Vec<_> = {
            let mut hub = hub::hub();
            events.into_iter().filter_map(|(event, posted)| hub.stats.record(event, posted)).collect()
        };
        for (stat, reading) in changed {
            self.stats_log.append(stat, reading);
        }
    }

    fn refresh(&mut self) {
        if self.next_auto_stats.is_some() {
            self.next_auto_stats = Some(Instant::now() + auto_stats_wait());
        }
        for query in QUERIES {
            if !self.queries.contains(&query) {
                if self.queries.is_empty() {
                    self.query_since = Instant::now();
                }
                self.queries.push_back(query);
            }
        }
        out!("[STATS] Reading stats: {}.", QUERIES.join(", "));
    }

    fn send(&mut self, command: &str, message_box: &IUIAutomationElement) -> Sent {
        let hwnd = self.target.hwnd;
        if modifier_held() {
            out!("[!] A Shift, Ctrl, Alt or Windows key is held down; retrying later.");
            return Sent::No;
        }
        let previous = unsafe { GetForegroundWindow() };
        if !focus_window(hwnd) {
            out!("[!] Windows refused to focus the Discord window; retrying later.");
            return Sent::No;
        }
        if unsafe { message_box.SetFocus() }.is_err() {
            out!("[!] The Discord message box could not take focus; retrying later.");
            return Sent::No;
        }
        sleep(Duration::from_millis(80));
        if unsafe { GetForegroundWindow() } != hwnd {
            out!("[!] Focus moved before typing; cancelled.");
            return Sent::No;
        }
        if !type_text(hwnd, command) {
            return Sent::Interrupted;
        }
        // Keys pressed by the user while typing, or a blocked SendInput, show up as a mismatch here.
        let typed = (0..6).any(|attempt| {
            if attempt > 0 {
                sleep(Duration::from_millis(50));
            }
            element_value(message_box, false).is_some_and(|v| composer_text(&v) == command)
        });
        if !typed {
            return Sent::Unconfirmed;
        }
        if modifier_held() || !press_enter(hwnd) {
            return Sent::Interrupted;
        }
        sleep(Duration::from_millis(150));
        if previous != hwnd && !previous.is_invalid() {
            focus_window(previous);
        }
        Sent::Yes
    }

    /// Always scan right before typing, so nothing is sent while a captcha is up.
    /// Returns the newest message ID seen before typing, which every reply to the command comes after.
    fn scan_and_send(&mut self, command: &str) -> Option<u64> {
        let scan = self.check()?;
        self.next_scan = Instant::now() + SCAN_EVERY;
        let Some(message_box) = scan.message_box else {
            out!("[!] Message box not found. Make sure a Discord channel is open.");
            return None;
        };
        if !composer_text(&scan.message_box_text).is_empty() {
            self.pause("The Discord message box is not empty (a draft?). Clear it, then press F8.", false);
            return None;
        }
        let before = self.newest_seen;
        match self.send(command, &message_box) {
            Sent::Yes => {}
            Sent::No => return None,
            Sent::Interrupted => {
                self.pause(
                    &format!("Focus moved while typing \"{command}\". Clear any leftover text in the message box."),
                    false,
                );
                return None;
            }
            Sent::Unconfirmed => {
                self.pause(
                    &format!(
                        "\"{command}\" did not appear in the message box as typed, so it was not sent. Clear the \
                         message box, then press F8. If Discord runs as administrator, run Zippy as administrator too."
                    ),
                    false,
                );
                return None;
            }
        }
        self.gems.sent(command, before + 1);
        self.watcher.sent(command, before, Instant::now());
        if command == PRAY_COMMAND {
            hub::hub().stats.prays_sent += 1;
        }
        out!("[{}] sent: {command}", clock());
        Some(before)
    }

    fn next_job(&self, now: Instant, daily: Option<daily::Daily>) -> Option<Job> {
        let holding = self.gem_hold.is_some_and(|until| now < until) && self.gems.awaiting_inventory();
        let mut candidates = Vec::new();
        if !self.gem_queue.is_empty() {
            candidates.push(Candidate { tier: 0, urgent: true, due: self.gem_since, job: Job::Gem });
        }
        for (i, timer) in self.timers.iter().enumerate() {
            if now >= timer.next {
                let tier = if timer.command == PRAY_COMMAND || timer.command == OWO_COMMAND { 1 } else { 0 };
                candidates.push(Candidate { tier, urgent: false, due: timer.next, job: Job::Timer(i) });
            }
        }
        if !self.queries.is_empty() {
            candidates.push(Candidate { tier: 2, urgent: false, due: self.query_since, job: Job::Query });
        }
        if let (Some(daily), Some(since)) = (daily, self.daily_since) {
            candidates.push(Candidate { tier: 2, urgent: false, due: since, job: Job::Daily(daily) });
        }
        pick(candidates, now, holding)
    }

    fn run(&mut self, job: Job) {
        let sent = match job {
            Job::Timer(i) => {
                let sent = self.scan_and_send(self.timers[i].command).is_some();
                if sent {
                    self.timers[i].next = Instant::now() + self.timers[i].every;
                }
                sent
            }
            Job::Gem => {
                let command = self.gem_queue[0].clone();
                let sent = self.scan_and_send(&command).is_some();
                if sent {
                    self.gem_queue.pop_front();
                    self.gem_since = Instant::now();
                    if command == gems::INVENTORY_COMMAND {
                        self.gem_hold = Some(Instant::now() + INVENTORY_WAIT);
                    }
                }
                sent
            }
            Job::Daily(daily) => match self.scan_and_send(daily.command()) {
                Some(before) => {
                    self.dailies.sent(daily, before, Instant::now(), unix_ms_now());
                    self.dailies.save();
                    self.daily_since = None;
                    true
                }
                None => false,
            },
            Job::Query => {
                let command = self.queries[0];
                let sent = self.scan_and_send(command).is_some();
                if sent {
                    self.queries.pop_front();
                    self.query_since = Instant::now();
                }
                sent
            }
        };
        self.next_send = Instant::now() + if sent { GAP_BETWEEN_COMMANDS } else { SCAN_EVERY };
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let daily = self.dailies.next_due(now, unix_ms_now());
        self.daily_since = daily.and(self.daily_since.or(Some(now)));
        if self.next_auto_stats.is_some_and(|at| now >= at) && self.queries.is_empty() {
            self.refresh();
        }
        if now >= self.next_send
            && let Some(job) = self.next_job(now, daily)
        {
            self.run(job);
        } else if now >= self.next_scan {
            self.next_scan = now + SCAN_EVERY;
            self.check();
        }
    }
}

fn clock() -> String {
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

fn describe(id: Option<u64>) -> String {
    id.map_or_else(|| "none".to_owned(), |id| format!("{id} ({})", age(id)))
}

fn ask(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    // A closed stdin (read of 0 bytes) is not a yes.
    matches!(std::io::stdin().read_line(&mut line), Ok(n) if n > 0)
        && matches!(line.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

/// Returns the path of the new exe once it is installed.
fn update(confirmed: bool) -> Option<std::path::PathBuf> {
    let release = match update::check() {
        Ok(Some(release)) => release,
        Ok(None) => {
            if confirmed {
                out!("[OK] Zippy {} is the latest version.", update::current_version());
            }
            return None;
        }
        Err(e) => {
            out!("[!] Could not check for updates: {e}");
            return None;
        }
    };
    out!("[UPDATE] Zippy {} is available (you have {}).", release.tag, update::current_version());
    if !confirmed && !ask("         Download and install it now? [Y/n] ") {
        out!("         Skipped. Run zippy.exe --update to install it later.\n");
        return None;
    }
    match update::install(&release) {
        Ok(exe) => {
            out!("[OK] Updated to {}.", release.tag);
            Some(exe)
        }
        Err(e) => {
            out!("[!] Update failed: {e}");
            None
        }
    }
}

/// A second instance would type every command twice into the same channel.
struct InstanceLock(HANDLE);

impl InstanceLock {
    fn acquire() -> Result<Option<Self>> {
        unsafe {
            let handle = CreateMutexW(None, true, w!("Local\\Zippy.SingleInstance"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Ok(None);
            }
            Ok(Some(Self(handle)))
        }
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

fn load_settings(path: &Path) -> Option<Settings> {
    match Settings::load_or_create(path) {
        Ok((settings, created)) => {
            if created {
                out!("[OK] Created {} with the default settings.", path.display());
            }
            Some(settings)
        }
        Err(e) => {
            out!("[!] Settings: {e}");
            out!("    Fix {} or delete it to get the defaults back.", path.display());
            None
        }
    }
}

fn describe_plan(settings: &Settings) {
    out!("Zippy: F8 = pause/resume, F10 = quit.");
    for timer in timers(settings, Instant::now()) {
        out!("  {:<8} every {}", timer.command, every_text(timer.every));
    }
    let dailies: Vec<&str> = settings.enabled_dailies().map(|d| d.command()).collect();
    if !dailies.is_empty() {
        out!("  {}: until today's limit is used up", dailies.join(", "));
    }
    if !settings.owo && !settings.pray && dailies.is_empty() {
        out!("  owo, owopray and the daily commands are off. Turn them on in Settings.");
    }
    out!("Settings: {}", settings.describe());
}

fn find_and_wait(uia: &Uia, f10: &mut Hotkey) -> Result<Option<(Target, PageScan)>> {
    let (target, reason) = loop {
        if let Some(found) = find_discord_window(uia) {
            break found;
        }
        out!("[..] No Discord app window or browser tab found yet. Looking again in 3 seconds...");
        for _ in 0..30 {
            sleep(Duration::from_millis(100));
            if f10.pressed() {
                return Ok(None);
            }
        }
    };
    report_target(&target, reason);
    let initial = wait_for_page(uia, target.hwnd)?;
    Ok(Some((target, initial)))
}

fn check_page() -> Result<()> {
    let uia = Uia::new()?;
    let Some((_, initial)) = find_and_wait(&uia, &mut Hotkey::new(VK_F10))? else { return Ok(()) };
    let box_name = initial
        .message_box
        .as_ref()
        .and_then(|b| unsafe { b.CachedName() }.ok())
        .map_or_else(|| "NOT FOUND".to_owned(), |n| format!("\"{n}\""));
    out!("     Page elements  : {}", initial.elements);
    out!("     URL discord.com: {}", initial.url_is_discord);
    out!("     Message box    : {box_name}");
    out!("     Box text       : \"{}\"", initial.message_box_text);
    out!("     Newest message : {}", describe(initial.newest_message));
    out!("     Newest captcha : {}", describe(initial.newest_captcha));
    out!("     Newest ban     : {}", describe(initial.newest_ban));
    out!("     Unread OwO DMs : {}", initial.owo_dm_unread);
    let from_owo = initial.messages.iter().filter(|m| m.from_owo).count();
    out!("     From real OwO  : {from_owo} of {} messages (name OwO + Verified App tag)", initial.messages.len());
    if hub::hub().debug {
        dump_messages(&initial.messages);
    }
    Ok(())
}

const DUMP_MESSAGES: usize = 15;
const DUMP_CHARS: usize = 400;

/// Developer mode: the newest messages exactly as Zippy reads them, for writing and checking parsers.
fn dump_messages(messages: &[ChatMessage]) {
    let mut newest: Vec<&ChatMessage> = messages.iter().collect();
    newest.sort_unstable_by_key(|m| m.id);
    let skip = newest.len().saturating_sub(DUMP_MESSAGES);
    out!("[DEBUG] The newest {} messages as Zippy reads them:", newest.len() - skip);
    for message in &newest[skip..] {
        let author = if message.from_owo { "OwO" } else { "not OwO" };
        let summary: String = message.summary.chars().take(DUMP_CHARS).collect();
        out!("[DEBUG] {} ({author}): {summary}", message.id);
        if !message.detail.is_empty() {
            let detail: String = message.detail.chars().take(DUMP_CHARS).collect();
            out!("[DEBUG]     detail: {detail}");
        }
    }
}

fn start_session(uia: &Uia, target: Target, initial: PageScan, settings: &Settings, stats_path: &Path) -> Zippy {
    let channel = channel_key(&target.title).to_owned();
    let now = Instant::now();
    // A PC clock running ahead would otherwise mark real captchas as already seen.
    let now_ms = initial.newest_message.map_or(unix_ms_now(), |id| unix_ms_now().min(snowflake_unix_ms(id)));
    let mut m = Zippy {
        uia: uia.clone(),
        target,
        channel,
        acknowledged: snowflake_at(now_ms.saturating_sub(STARTUP_LOOKBACK.as_millis() as u64)),
        dm_baseline: initial.owo_dm_unread,
        newest_seen: initial.newest_message.unwrap_or(0),
        state: State::Running,
        timers: timers(settings, now),
        next_send: now,
        next_scan: now,
        unreadable_scans: 0,
        scrolled_up_reported: false,
        gems: GemKeeper::new(initial.newest_message.unwrap_or_else(|| snowflake_at(now_ms)), settings.gems),
        gem_upkeep: settings.mode.hunts(),
        gem_queue: VecDeque::new(),
        gem_since: now,
        gem_hold: None,
        dailies: Dailies::new(settings.enabled_dailies(), Some(stats_path.with_file_name(daily::FILE_NAME)), unix_ms_now()),
        daily_since: None,
        watcher: Watcher::new().counting_battles_after(initial.newest_message.unwrap_or(0)),
        stats_log: stats::Log::new(stats_path.to_owned()),
        queries: VecDeque::new(),
        query_since: now,
        next_auto_stats: settings.stats.then(|| now + auto_stats_wait()),
        simulated_captcha: false,
    };
    out!("     Locked to channel: {}\n", m.channel);
    hub::set_phase(Phase::Running);
    hub::hub().dailies = m.dailies.view();
    if let Some(at) = m.next_auto_stats {
        out!("     Stats are read on their own in {} min.", at.duration_since(now).as_secs() / 60);
    }
    if let Some(id) = initial.newest_ban.filter(|&id| id > m.acknowledged) {
        m.ban(id);
    } else if let Some(id) = initial.newest_captcha.filter(|&id| id > m.acknowledged) {
        m.pause(&format!("New OwO captcha message visible at startup ({}).", age(id)), true);
    } else if initial.newest_message.is_none() {
        m.pause("Discord chat messages are not readable (accessibility off?).", false);
        out!("       {}", m.target.accessibility_hint());
    } else if initial.owo_dm_unread > 0 {
        m.pause(
            &format!(
                "{} unread OwO DMs. Check them for a captcha first; if there is none, resume.",
                initial.owo_dm_unread
            ),
            false,
        );
    } else {
        out!("{} page elements read. Starting...", initial.elements);
    }
    m
}

fn remind(settings: &Settings) {
    let due = hub::hub().stats.due_reminders(unix_ms_now());
    for label in due {
        out!("[{}] [REMINDER] {label}.", clock());
        if settings.reminders
            && let Err(e) = notify("Zippy reminder", label, false)
        {
            out!("[!] Could not send the Windows notification: {e}");
        }
    }
}

enum Session {
    Idle,
    Searching(Instant),
    Active(Box<Zippy>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ui {
    Console,
    Dashboard,
}

/// Runs until F10. The dashboard and the hotkeys both steer it through `hub::Control`.
fn run(ui: Ui, settings_path: &Path) -> Result<()> {
    let uia = Uia::new()?;
    let stats_path = settings_path.with_file_name(stats::FILE_NAME);
    {
        let mut hub = hub::hub();
        stats::Log::new(stats_path.clone()).load(&mut hub.stats);
    }
    let mut settings = load_settings(settings_path).unwrap_or_default();
    let dailies_path = stats_path.with_file_name(daily::FILE_NAME);
    hub::hub().dailies = Dailies::new(settings.enabled_dailies(), Some(dailies_path), unix_ms_now()).view();
    let mut session = Session::Idle;
    hub::set_phase(Phase::Idle);
    if ui == Ui::Console {
        hub::control(Control::Start);
    } else {
        out!("Press Start on the dashboard to begin. F10 here quits Zippy.");
    }
    let mut f8 = Hotkey::new(VK_F8);
    let mut f10 = Hotkey::new(VK_F10);
    let mut f9 = Hotkey::new(VK_F9);
    if hub::hub().debug {
        out!("[DEBUG] Developer mode: F9 (or Settings on the dashboard) simulates an OwO captcha while Zippy is running.");
    }
    let mut next_reminder = Instant::now();
    loop {
        if f10.pressed() {
            out!("Quitting.");
            return Ok(());
        }
        let mut control = hub::hub().take_control();
        if f8.pressed() {
            let running = matches!(&session, Session::Active(z) if matches!(z.state, State::Running));
            control = Some(if running { Control::Pause } else { Control::Start });
        }
        if f9.pressed() && hub::hub().debug {
            control = Some(Control::SimulateCaptcha);
        }
        match (control, &mut session) {
            (Some(Control::Start), Session::Idle) => {
                if let Some(loaded) = load_settings(settings_path) {
                    settings = loaded;
                    describe_plan(&settings);
                    session = Session::Searching(Instant::now());
                    hub::set_phase(Phase::Searching);
                }
            }
            (Some(Control::Start), Session::Active(z)) if !matches!(z.state, State::Running) => z.resume(),
            (Some(Control::Pause), Session::Active(z)) if matches!(z.state, State::Running) => {
                z.pause("Paused manually.", false)
            }
            (Some(Control::Stop), Session::Searching(_) | Session::Active(_)) => {
                session = Session::Idle;
                hub::set_phase(Phase::Idle);
                hub::hub().target = None;
                out!("[{}] Stopped.", clock());
            }
            (Some(Control::Refresh), Session::Active(z)) => z.refresh(),
            (Some(Control::Refresh), _) => out!("[!] Press Start first: reading stats sends commands in Discord."),
            (Some(Control::SimulateCaptcha), Session::Active(z)) if matches!(z.state, State::Running) => {
                out!("[DEBUG] Simulating an OwO captcha; Zippy reacts at its next check.");
                z.simulated_captcha = true;
                z.next_scan = Instant::now();
            }
            (Some(Control::SimulateCaptcha), _) => {
                out!("[DEBUG] A captcha can only be simulated while Zippy is running. Press Start first.");
            }
            (Some(Control::DumpChat), Session::Active(z)) => match z.uia.scan(z.target.hwnd) {
                Ok(scan) => dump_messages(&scan.messages),
                Err(e) => out!("[!] Could not read the page: {e}"),
            },
            (Some(Control::DumpChat), _) => out!("[DEBUG] Press Start first, so Zippy knows which Discord window to read."),
            _ => {}
        }
        let now = Instant::now();
        match &mut session {
            Session::Searching(next_try) if now >= *next_try => match find_discord_window(&uia) {
                Some((target, reason)) => {
                    report_target(&target, reason);
                    session = match wait_for_page(&uia, target.hwnd) {
                        Ok(initial) => {
                            Session::Active(Box::new(start_session(&uia, target, initial, &settings, &stats_path)))
                        }
                        Err(e) => {
                            out!("[!] Could not read the page: {e}");
                            Session::Searching(now + Duration::from_secs(3))
                        }
                    };
                }
                None => {
                    out!("[..] No Discord app window or browser tab found yet. Looking again in 3 seconds...");
                    *next_try = now + Duration::from_secs(3);
                }
            },
            Session::Active(z) if matches!(z.state, State::Running) => z.tick(),
            _ => {}
        }
        if now >= next_reminder {
            next_reminder = now + Duration::from_secs(1);
            remind(&settings);
        }
        sleep(Duration::from_millis(50));
    }
}

fn main() -> Result<()> {
    let has_flag = |flag: &str| std::env::args().any(|a| a == flag);
    hub::hub().debug = has_flag("--debug");
    if has_flag("--check") {
        return check_page();
    }
    if let Err(e) = register_toast_app() {
        out!("[!] Could not register Windows notifications: {e}");
    }
    if has_flag("--test-notification") {
        notify("Zippy: test notification", "If you can see this, captcha alerts will show up here too.", true)?;
        out!("[OK] Test notification sent; check Notification Center (click the clock on the taskbar).");
        return Ok(());
    }
    let Some(lock) = InstanceLock::acquire()? else {
        out!("[!] Zippy is already running. Use that window, or press F10 there to quit it first.");
        return Ok(());
    };
    update::remove_previous();
    if has_flag("--update") {
        if update(true).is_some() {
            out!("     Start zippy.exe again to use it.");
        }
        return Ok(());
    }
    let settings_path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(settings::FILE_NAME)))
        .unwrap_or_else(|| settings::FILE_NAME.into());
    let ui = if has_flag("--dashboard") {
        Ui::Dashboard
    } else if has_flag("--console") {
        Ui::Console
    } else {
        match menu::choose(&settings_path) {
            menu::Choice::Dashboard => Ui::Dashboard,
            menu::Choice::Console => Ui::Console,
        }
    };
    let settings = load_settings(&settings_path).unwrap_or_default();
    if settings.update_check
        && let Some(exe) = update(false)
    {
        out!("     Restarting...\n");
        drop(lock);
        let flag = if ui == Ui::Dashboard { "--dashboard" } else { "--console" };
        let mut restart = std::process::Command::new(&exe);
        restart.arg(flag);
        if hub::hub().debug {
            restart.arg("--debug");
        }
        if let Err(e) = restart.spawn() {
            out!("[!] Could not restart: {e}. Start zippy.exe again.");
        }
        return Ok(());
    }
    if ui == Ui::Dashboard {
        match web::start(settings_path.clone()) {
            Ok(url) => {
                // Printed only, so the secret link never reaches the dashboard's own log.
                println!("[OK] Dashboard: {url}");
                out!("[OK] Dashboard opened in your browser. Keep this window open; closing it stops Zippy.");
                web::open_in_browser(&url);
            }
            Err(e) => {
                out!("[!] Could not start the dashboard: {e}. Running here instead.");
                return run(Ui::Console, &settings_path);
            }
        }
    }
    run(ui, &settings_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_sees_through_owo_zero_width_obfuscation() {
        let warning = "OwO1/1/26, 12:00 PM | @Player, a\u{200b}re y\u{200b}ou a\u{200b} re\u{200b}al huma\u{200b}n? Ple\u{200b}ase u\u{200b}se th\u{200b}e l\u{200b}ink belo\u{200b}w";
        let captcha = "OwO1/1/26, 12:00 PM | @Player! Plea\u{200b}se comp\u{200b}lete yo\u{200b}ur captch\u{200b}a t\u{200b}o ver\u{200b}ify t\u{200b}hat yo\u{200b}u ar\u{200b}e h\u{200b}uman! (1/5)";
        let ban = "OwO1/1/26, 12:01 PM☠ | Player!  You have been banned for 36H for macros or botting!";
        assert_eq!(classify(warning), Some(Alarm::Captcha));
        assert_eq!(classify(captcha), Some(Alarm::Captcha));
        assert_eq!(classify(ban), Some(Alarm::Ban));
        assert_eq!(classify("Please complete your сaptсha"), Some(Alarm::Captcha));
        assert_eq!(classify("OwO | Player, hunt is empowered by [800/1000] | You found:"), None);
        assert_eq!(classify("Player goes into battle! You won in 5 turns!"), None);
    }

    #[test]
    fn message_id_reads_only_chat_list_items() {
        assert_eq!(
            message_id("chat-messages-100000000000000001-1300000000000000002"),
            Some(1300000000000000002)
        );
        assert_eq!(message_id("message-timestamp-1300000000000000003"), None);
        assert_eq!(message_id("chat-messages-100000000000000001"), None);
        assert_eq!(message_id(""), None);
    }

    #[test]
    fn snowflake_time_roundtrip_orders_messages_by_time() {
        let id = 1300000000000000002;
        let at = snowflake_at(snowflake_unix_ms(id));
        assert!(at <= id && id - at < 1 << 22);
        assert!(snowflake_at(snowflake_unix_ms(id) + 1) > id);
    }

    #[test]
    fn owo_dm_unread_reads_only_the_owo_dm_bubble() {
        assert_eq!(owo_dm_unread("owo , 10 unread messages"), Some(10));
        assert_eq!(owo_dm_unread("owo , 1 unread message"), Some(1));
        assert_eq!(owo_dm_unread("OwO , 2 Unread Messages"), Some(2));
        assert_eq!(owo_dm_unread("ow"), None);
        assert_eq!(owo_dm_unread("owo"), None);
        assert_eq!(owo_dm_unread("owo verified app , 🌱 | player, hunt is empowered by"), None);
        assert_eq!(owo_dm_unread("owobot , 3 unread messages"), None);
        assert_eq!(owo_dm_unread("carl-bot , 2 unread messages"), None);
        assert_eq!(owo_dm_unread("1 mention, owo"), None);
    }

    #[test]
    fn desktop_title_matches_only_the_main_window() {
        assert!(is_desktop_title("#owo | Server - Discord"));
        assert!(is_desktop_title("(2) • Friends - Discord"));
        assert!(is_desktop_title("Discord"));
        assert!(!is_desktop_title("Discord Updater"));
        assert!(!is_desktop_title("MyDiscord"));
        assert!(!is_desktop_title(""));
    }

    #[test]
    fn only_the_address_bar_counts_as_a_discord_url() {
        assert!(is_discord_url("discord.com/channels/1/2"));
        assert!(is_discord_url("https://ptb.discord.com/channels/@me"));
        assert!(is_discord_url("discord.com"));
        assert!(!is_discord_url("check discord.com/invite/abc"));
        assert!(!is_discord_url("discord.community/forum"));
    }

    #[test]
    fn empty_message_box_ignores_discords_placeholder_character() {
        assert_eq!(composer_text("\u{feff}"), "");
        assert_eq!(composer_text("owoh\u{feff}\n"), "owoh");
        assert_eq!(composer_text("half a draft"), "half a draft");
    }

    /// Feeds (class, name, automation id) rows in the order UI Automation lists them.
    fn header(rows: &[(&str, &str, &str)]) -> HeaderReader {
        let mut reader = HeaderReader::default();
        for (class, name, automation_id) in rows {
            reader.read(class, name, automation_id);
        }
        reader
    }

    #[test]
    fn main_commands_and_gems_go_first_and_owo_inv_is_not_interrupted() {
        let now = Instant::now();
        let ago = |s: u64| now - Duration::from_secs(s);
        let c = |tier, urgent, due, job| Candidate { tier, urgent, due, job };
        let piku = Job::Daily(daily::Daily::Piku);
        let waiting = || [c(2, false, ago(50), piku), c(1, false, ago(30), Job::Timer(0)), c(0, false, ago(5), Job::Timer(1))];
        assert_eq!(pick(waiting(), now, false), Some(Job::Timer(1)), "owoh beats older owo and owopiku");

        let with_gem = waiting().into_iter().chain([c(0, true, ago(1), Job::Gem)]);
        assert_eq!(pick(with_gem, now, false), Some(Job::Gem), "a queued owo use goes before the next owoh");

        let after_inv = [c(2, false, ago(50), piku), c(1, false, ago(30), Job::Timer(0))];
        assert_eq!(pick(after_inv, now, true), None, "nothing cuts in while the owo inv reply is awaited");
        assert_eq!(pick(waiting(), now, true), Some(Job::Timer(1)), "owoh and owob still run meanwhile");

        let starved = [c(2, false, ago(200), piku), c(0, false, ago(5), Job::Timer(1))];
        assert_eq!(pick(starved, now, false), Some(piku), "a job left waiting too long gets its turn");
    }

    #[test]
    fn only_the_verified_owo_header_counts_as_owo() {
        let owo = header(&[
            ("header_c19a55", "OwO Verified App 12:33 PM", ""),
            ("username_c19a55 clickable_c19a55", "OwO", ""),
            ("botTagCozy_c19a55 botTag_c19a55", "Verified App", ""),
            ("", "", "message-timestamp-1"),
        ]);
        let impostor = header(&[
            ("header_c19a55", "OwO Verified App 12:33 PM", ""),
            ("username_c19a55", "OwO Verified App", ""),
            ("", "", "message-timestamp-2"),
        ]);
        let impostor_named_owo = header(&[("header_c19a55", "OwO 12:33 PM", ""), ("username_c19a55", "OwO", ""), ("", "", "message-timestamp-3")]);
        let reply_to_owo = header(&[
            ("repliedMessage_c19a55", "NeonUtil replying to OwO", "message-reply-context-1"),
            ("botTagCompact_c19a55 botTag_c19a55", "Verified App", ""),
            ("username_c19a55", "@OwO", ""),
            ("header_c19a55", "NeonUtil Verified App 12:29 PM", ""),
            ("username_c19a55", "NeonUtil", ""),
            ("botTagCozy_c19a55 botTag_c19a55", "Verified App", ""),
        ]);
        let continuation = header(&[("", "owoh", ""), ("", "", "message-timestamp-4")]);
        let found = authors([&continuation, &owo, &continuation, &impostor, &continuation, &impostor_named_owo, &reply_to_owo].into_iter());
        use Author::*;
        assert_eq!(found, [Unknown, Owo, Owo, Other, Other, Other, Other]);
    }

    #[test]
    fn discord_title_matches_discord_tabs_not_pages_mentioning_it() {
        assert!(is_discord_title("(3) • Discord | #owo | Server - Brave"));
        assert!(is_discord_title("Discord | @someone - Google Chrome"));
        assert!(is_discord_title("Discord - Brave"));
        assert!(!is_discord_title("user/repo: Automated bot for farming Owo bot on Discord. - Brave"));
        assert!(!is_discord_title("Discord bots guide - Google Search - Brave"));
        assert!(!is_discord_title("Discordia - Wikipedia - Brave"));
    }

    #[test]
    fn channel_key_ignores_badges_but_not_channel_changes() {
        let base = "Discord | #owo | Server - Google Chrome";
        assert_eq!(channel_key("(12) Discord | #owo | Server - Google Chrome"), base);
        assert_eq!(channel_key("• Discord | #owo | Server - Google Chrome"), base);
        assert_eq!(channel_key("(1) • Discord | #owo | Server - Google Chrome"), base);
        assert_ne!(channel_key("(1) Discord | @OwO - Google Chrome"), base);
        assert_eq!(channel_key("(beta) Discord"), "(beta) Discord");
    }
}
