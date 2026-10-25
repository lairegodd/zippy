mod gems;
mod settings;

use std::collections::VecDeque;
use std::mem::size_of;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gems::{ChatMessage, GemKeeper};
use settings::Settings;
use windows::core::{w, Result, BSTR, HSTRING, PWSTR, VARIANT};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, RECT};
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
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_NamePropertyId,
    UIA_ValueValuePropertyId, UIA_E_ELEMENTNOTAVAILABLE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_F10, VK_F8, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, FlashWindowEx, GetForegroundWindow, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    SetForegroundWindow, ShowWindow, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, SW_RESTORE,
};
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

/// OwO's 15 s cooldown plus 3 s for network lag.
const CYCLE: Duration = Duration::from_secs(18);
const GAP_BETWEEN_COMMANDS: Duration = Duration::from_millis(2500);
const SCAN_EVERY: Duration = Duration::from_secs(4);
const SCAN_ATTEMPTS: u32 = 5;
const SCAN_RETRY_DELAY: Duration = Duration::from_millis(300);
const KEY_DELAY: Duration = Duration::from_millis(35);
const CLOCK_SKEW: Duration = Duration::from_secs(10);
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

fn notify(title: &str, body: &str) -> Result<()> {
    let xml = format!(
        r#"<toast scenario="reminder"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual><actions><action content="Dismiss" arguments="dismiss" activationType="system"/></actions><audio src="ms-winsoundevent:Notification.Looping.Alarm" loop="true"/></toast>"#,
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

fn alert(hwnd: HWND, title: &str, body: &str) {
    if let Err(e) = notify(title, body) {
        println!("[!] Could not send the Windows notification: {e}");
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
    fn pressed(&mut self) -> bool {
        let down = unsafe { GetAsyncKeyState(self.vk.0 as i32) } as u16 & 0x8000 != 0;
        let fired = down && !self.was_down;
        self.was_down = down;
        fired
    }
}

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
                url_is_discord: false,
                messages: Vec::new(),
            };
            let mut best_box: Option<(bool, i32)> = None;
            let mut summary_of: Option<u64> = None;

            for i in 0..elements {
                let Ok(el) = found.GetElement(i) else { continue };
                let name = el.CachedName().map(|b| b.to_string()).unwrap_or_default();
                let automation_id = el.CachedAutomationId().map(|b| b.to_string()).unwrap_or_default();
                if let Some(id) = message_id(&automation_id) {
                    scan.newest_message = scan.newest_message.max(Some(id));
                    match classify(&name) {
                        Some(Alarm::Captcha) => scan.newest_captcha = scan.newest_captcha.max(Some(id)),
                        Some(Alarm::Ban) => scan.newest_ban = scan.newest_ban.max(Some(id)),
                        None => {}
                    }
                    summary_of = Some(id);
                    continue;
                }
                if let Some(n) = owo_dm_unread(&name) {
                    scan.owo_dm_unread += n;
                }
                if !name.is_empty()
                    && let Some(id) = summary_of.take()
                {
                    scan.messages.push(ChatMessage { id, summary: name.clone() });
                }
                if el.CachedControlType().ok() != Some(UIA_EditControlTypeId) {
                    continue;
                }
                let value = el
                    .GetCachedPropertyValue(UIA_ValueValuePropertyId)
                    .ok()
                    .and_then(|v: VARIANT| BSTR::try_from(&v).ok())
                    .map(|b| b.to_string().to_lowercase())
                    .unwrap_or_default();
                if value.contains("discord.com") {
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
                }
            }
            Ok(scan)
        }
    }
}

struct Target {
    hwnd: HWND,
    title: String,
    exe: String,
}

fn find_discord_window(uia: &Uia) -> Option<(Target, &'static str)> {
    let mut by_title: Option<Target> = None;
    for hwnd in top_level_windows() {
        let Some(exe) = process_exe(hwnd) else { continue };
        if !BROWSERS.contains(&exe.as_str()) {
            continue;
        }
        let title = window_title(hwnd);
        if title.is_empty() {
            continue;
        }
        if uia.scan(hwnd).is_ok_and(|s| s.url_is_discord) {
            return Some((Target { hwnd, title, exe }, "URL discord.com"));
        }
        if by_title.is_none() && is_discord_title(&title) {
            by_title = Some(Target { hwnd, title, exe });
        }
    }
    by_title.map(|t| (t, "its Discord tab title"))
}

/// Discord web titles start with "Discord" (after unread badges); other pages only mention it.
fn is_discord_title(title: &str) -> bool {
    strip_prefix_ignore_case(channel_key(title), "discord")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(" |") || rest.starts_with(" -"))
}

fn wait_for_page(uia: &Uia, hwnd: HWND) -> Result<PageScan> {
    let mut scan = uia.scan(hwnd)?;
    for _ in 0..20 {
        if scan.newest_message.is_some() {
            break;
        }
        sleep(Duration::from_millis(500));
        scan = uia.scan(hwnd)?;
    }
    Ok(scan)
}

fn report_target(t: &Target, reason: &str) {
    let m = monitor_of(t.hwnd);
    println!("[OK] Found Discord by {reason}");
    println!("     Browser : {}", t.exe);
    println!("     Title   : {}", t.title);
    println!("     Monitor : #{} of {}{}", m.index, m.count, if m.primary { " [primary]" } else { "" });
}

enum Sent {
    Yes,
    No,
    /// Focus moved mid-command, so part of it may be left in the message box.
    Interrupted,
}

enum State {
    Running,
    Paused,
    Banned,
}

struct Zippy {
    uia: Uia,
    target: Target,
    channel: String,
    acknowledged: u64,
    dm_baseline: u32,
    newest_seen: u64,
    state: State,
    next_command: usize,
    next_at: Instant,
    cycle_start: Instant,
    next_scan: Instant,
    unreadable_scans: u32,
    scrolled_up_reported: bool,
    commands: &'static [&'static str],
    gems: Option<GemKeeper>,
    gem_queue: VecDeque<String>,
    next_gem_at: Instant,
}

impl Zippy {
    fn pause(&mut self, reason: &str, alarm: bool) {
        self.state = State::Paused;
        println!("\n[{}] Macro has Stopped Temporarily! {reason}", clock());
        println!("       Solve the captcha or go back to the original channel, then press F8 to resume.");
        if alarm {
            alert(
                self.target.hwnd,
                "Zippy stopped!",
                &format!("{reason}\nSolve the verification, then press F8 in the Zippy window."),
            );
        }
    }

    fn ban(&mut self, id: u64) {
        self.state = State::Banned;
        println!("\n[{}] OwO ban message detected ({}). Zippy stopped.", clock(), age(id));
        println!("       Check who sent it. If it is not from OwO (fake) or the ban is over,");
        println!("       press F8 to resume. F10 to quit.");
        alert(
            self.target.hwnd,
            "OwO: BAN message detected",
            "Zippy stopped. If the ban message is fake or the ban is over, press F8 to resume.",
        );
    }

    fn resume(&mut self) {
        let confirming_ban = matches!(self.state, State::Banned);
        let title = window_title(self.target.hwnd);
        if channel_key(&title) != self.channel {
            println!("[!] The active tab/channel differs from the one at startup:");
            println!("    now     : {}", channel_key(&title));
            println!("    startup : {}", self.channel);
            println!("    Open the original channel again, then press F8.");
            return;
        }
        let scan = match self.uia.scan(self.target.hwnd) {
            Ok(scan) => scan,
            Err(e) => {
                println!("[!] Could not read the page: {e}");
                return;
            }
        };
        let Some(newest) = scan.newest_message else {
            println!("[!] Discord chat messages are not readable yet; try F8 again in a moment.");
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
        self.next_command = 0;
        self.next_at = Instant::now();
        self.next_scan = Instant::now() + SCAN_EVERY;
        println!("[{}] Zippy running again.", clock());
    }

    fn check(&mut self) -> Option<PageScan> {
        let hwnd = self.target.hwnd;
        if !unsafe { IsWindow(hwnd) }.as_bool() {
            self.pause("Discord window closed.", true);
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
                println!("[!] Could not read the page: {e}");
                return None;
            }
        };
        if let Some(id) = scan.newest_ban.filter(|&id| id > self.acknowledged) {
            self.ban(id);
            return None;
        }
        if let Some(id) = scan.newest_captcha.filter(|&id| id > self.acknowledged) {
            self.pause(&format!("OwO CAPTCHA message detected in the channel ({}).", age(id)), true);
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
            if self.unreadable_scans == 5 {
                println!(
                    "[!] Chat messages are not readable ({} accessibility elements); waiting without typing.",
                    scan.elements
                );
                println!("    Make sure the Discord tab is visible. If this persists, start the browser");
                println!("    with the --force-renderer-accessibility flag.");
            }
            return None;
        };
        self.unreadable_scans = 0;
        if newest < self.newest_seen {
            if !self.scrolled_up_reported {
                self.scrolled_up_reported = true;
                println!("[!] The chat is scrolled up, so the newest messages are hidden. Waiting.");
                println!("    Scroll to the bottom (or press F8 twice if the newest message was deleted).");
            }
            return None;
        }
        self.scrolled_up_reported = false;
        self.newest_seen = newest;
        if let Some(gems) = &mut self.gems {
            for action in gems.observe(&scan.messages) {
                let command = action.command();
                if !self.gem_queue.contains(&command) {
                    self.gem_queue.push_back(command);
                }
            }
        }
        Some(scan)
    }

    fn send(&mut self, command: &str, message_box: &IUIAutomationElement) -> Sent {
        let hwnd = self.target.hwnd;
        let previous = unsafe { GetForegroundWindow() };
        if !focus_window(hwnd) {
            println!("[!] Windows refused to focus the Discord window; retrying later.");
            return Sent::No;
        }
        if unsafe { message_box.SetFocus() }.is_err() {
            println!("[!] The Discord message box could not take focus; retrying later.");
            return Sent::No;
        }
        sleep(Duration::from_millis(80));
        if unsafe { GetForegroundWindow() } != hwnd {
            println!("[!] Focus moved before typing; cancelled.");
            return Sent::No;
        }
        if !type_text(hwnd, command) || !press_enter(hwnd) {
            return Sent::Interrupted;
        }
        sleep(Duration::from_millis(150));
        if previous != hwnd && !previous.is_invalid() {
            focus_window(previous);
        }
        Sent::Yes
    }

    /// Always scan right before typing, so nothing is sent while a captcha is up.
    fn scan_and_send(&mut self, command: &str) -> bool {
        let Some(scan) = self.check() else { return false };
        self.next_scan = Instant::now() + SCAN_EVERY;
        let Some(message_box) = scan.message_box else {
            println!("[!] Message box not found. Make sure the Discord tab is active and a channel is open.");
            return false;
        };
        let earliest_echo = snowflake_at(unix_ms_now().saturating_sub(CLOCK_SKEW.as_millis() as u64));
        match self.send(command, &message_box) {
            Sent::Yes => {}
            Sent::No => return false,
            Sent::Interrupted => {
                self.pause(
                    &format!("Focus moved while typing \"{command}\". Clear any leftover text in the message box."),
                    false,
                );
                return false;
            }
        }
        if let Some(gems) = &mut self.gems {
            gems.sent(command, earliest_echo);
        }
        println!("[{}] sent: {command}", clock());
        true
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let gem_slot = self.next_command == 0
            && !self.gem_queue.is_empty()
            && now >= self.next_gem_at
            && now + GAP_BETWEEN_COMMANDS <= self.next_at;
        if now >= self.next_at {
            let command = self.commands[self.next_command];
            if !self.scan_and_send(command) {
                self.next_at = Instant::now() + SCAN_EVERY;
                return;
            }
            if self.next_command == 0 {
                self.cycle_start = now;
            }
            self.next_command += 1;
            if self.next_command == self.commands.len() {
                self.next_command = 0;
                self.next_at = self.cycle_start + CYCLE;
            } else {
                self.next_at = Instant::now() + GAP_BETWEEN_COMMANDS;
            }
            self.next_gem_at = Instant::now() + GAP_BETWEEN_COMMANDS;
        } else if gem_slot {
            let command = self.gem_queue[0].clone();
            if self.scan_and_send(&command) {
                self.gem_queue.pop_front();
                self.next_gem_at = Instant::now() + GAP_BETWEEN_COMMANDS;
            } else {
                self.next_gem_at = Instant::now() + SCAN_EVERY;
            }
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

fn main() -> Result<()> {
    let check_only = std::env::args().any(|a| a == "--check");
    let test_notification = std::env::args().any(|a| a == "--test-notification");
    let settings_path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(settings::FILE_NAME)))
        .unwrap_or_else(|| settings::FILE_NAME.into());
    let settings = match Settings::load_or_create(&settings_path) {
        Ok((settings, created)) => {
            if created {
                println!("[OK] Created {} with the default settings.", settings_path.display());
            }
            settings
        }
        Err(e) => {
            println!("[!] Settings: {e}");
            println!("    Fix {} or delete it to get the defaults back.", settings_path.display());
            std::process::exit(1);
        }
    };
    println!(
        "Zippy: {} every {} seconds. F8 = pause/resume, F10 = quit.",
        settings.mode.commands().join("/"),
        CYCLE.as_secs()
    );
    println!("Settings: {} ({})\n", settings.describe(), settings_path.display());
    let uia = Uia::new()?;
    if let Err(e) = register_toast_app() {
        println!("[!] Could not register Windows notifications: {e}");
    }
    if test_notification {
        notify("Zippy: test notification", "If you can see this, captcha alerts will show up here too.")?;
        println!("[OK] Test notification sent; check Notification Center (click the clock on the taskbar).");
        return Ok(());
    }
    let mut f8 = Hotkey::new(VK_F8);
    let mut f10 = Hotkey::new(VK_F10);

    let (target, reason) = loop {
        if let Some(found) = find_discord_window(&uia) {
            break found;
        }
        println!("[..] No Discord tab found in any browser yet. Looking again in 3 seconds...");
        for _ in 0..30 {
            sleep(Duration::from_millis(100));
            if f10.pressed() {
                return Ok(());
            }
        }
    };
    report_target(&target, reason);

    let initial = wait_for_page(&uia, target.hwnd)?;
    if check_only {
        let box_name = initial
            .message_box
            .as_ref()
            .and_then(|b| unsafe { b.CachedName() }.ok())
            .map_or_else(|| "NOT FOUND".to_owned(), |n| format!("\"{n}\""));
        println!("     Page elements  : {}", initial.elements);
        println!("     URL discord.com: {}", initial.url_is_discord);
        println!("     Message box    : {box_name}");
        println!("     Newest message : {}", describe(initial.newest_message));
        println!("     Newest captcha : {}", describe(initial.newest_captcha));
        println!("     Newest ban     : {}", describe(initial.newest_ban));
        println!("     Unread OwO DMs : {}", initial.owo_dm_unread);
        return Ok(());
    }

    let channel = channel_key(&target.title).to_owned();
    let now = Instant::now();
    let mut m = Zippy {
        uia,
        target,
        channel,
        acknowledged: snowflake_at(unix_ms_now().saturating_sub(STARTUP_LOOKBACK.as_millis() as u64)),
        dm_baseline: initial.owo_dm_unread,
        newest_seen: initial.newest_message.unwrap_or(0),
        state: State::Running,
        next_command: 0,
        next_at: now,
        cycle_start: now,
        next_scan: now,
        unreadable_scans: 0,
        scrolled_up_reported: false,
        commands: settings.mode.commands(),
        gems: settings.mode.hunts().then(|| GemKeeper::new(snowflake_at(unix_ms_now()), settings.gems)),
        gem_queue: VecDeque::new(),
        next_gem_at: now,
    };
    println!("     Locked to channel: {}\n", m.channel);

    if let Some(id) = initial.newest_ban.filter(|&id| id > m.acknowledged) {
        m.ban(id);
    } else if let Some(id) = initial.newest_captcha.filter(|&id| id > m.acknowledged) {
        m.pause(&format!("New OwO captcha message visible at startup ({}).", age(id)), true);
    } else if initial.newest_message.is_none() {
        m.pause("Discord chat messages are not readable (browser accessibility off?).", false);
    } else if initial.owo_dm_unread > 0 {
        m.pause(
            &format!("{} unread OwO DMs, check them for a captcha first, if there isn't any, ignore this message.", initial.owo_dm_unread),
            false,
        );
    } else {
        println!("{} page elements read. Starting...", initial.elements);
    }

    loop {
        if f10.pressed() {
            println!("Quitting.");
            return Ok(());
        }
        if f8.pressed() {
            match m.state {
                State::Running => m.pause("Paused manually (F8).", false),
                State::Paused | State::Banned => m.resume(),
            }
        }
        if let State::Running = m.state {
            m.tick();
        }
        sleep(Duration::from_millis(50));
    }
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
