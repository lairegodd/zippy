//! In dashboard mode the console window waits in the notification area instead of the taskbar. Clicking the
//! icon (or Enter on it) shows the window again; minimizing the window sends it back. The right-click menu
//! (or Shift+F10 on the icon) says whether Discord was found and starts, pauses, stops or quits Zippy.
//!
//! The icon lives on a thread of its own with its own message loop. It only reads the hub under a short lock
//! and posts controls, which the main loop carries out, so it never sends anything itself.

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::{
    GetConsoleWindow, SetConsoleCtrlHandler, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetClassNameW,
    GetMessageW, IsIconic, IsWindowVisible, LoadIconW, PostMessageW, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetTimer, ShowWindow, TrackPopupMenuEx, TranslateMessage, IDI_APPLICATION, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, MSG, SW_HIDE, SW_RESTORE, SW_SHOW, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU, WM_NULL, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

use crate::hub::{self, Control, Phase};

const ICON_ID: u32 = 1;
/// Enter or Space on the icon: `NIN_SELECT | NINF_KEY` in shellapi.h, which the `windows` crate does not export.
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
const WM_TRAY: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 1000;
const ID_TOGGLE: usize = 1;
const ID_STOP: usize = 2;
const ID_EXIT: usize = 3;

/// The icon's window, once the icon is up; 0 before and after.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// Explorer broadcasts this after it restarts, when every program must add its icon again.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static TIP: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Puts the icon up and hides the console window once it is there. If the icon cannot be added, the window
/// stays as it is, so Zippy is never left running with no way to reach it.
pub fn start() {
    std::thread::spawn(|| {
        if let Err(e) = run() {
            out!("[!] Could not put Zippy in the taskbar tray ({e}); this window stays open.");
        }
    });
}

/// Takes the icon down. Safe from any thread and more than once; called before Zippy exits, or the icon would
/// linger until the mouse passes over it.
pub fn remove() {
    let window = WINDOW.swap(0, Ordering::SeqCst);
    if window != 0 {
        let data = icon_data(HWND(window as *mut _));
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        }
    }
}

fn run() -> windows::core::Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("ZippyTray");
        let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
        if RegisterClassW(&wc) == 0 {
            return Err(windows::core::Error::from_win32());
        }
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::SeqCst);
        // A hidden top-level window, not a message-only one: those never hear that Explorer restarted.
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Zippy"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        )?;
        add_icon(window)?;
        WINDOW.store(window.0 as isize, Ordering::SeqCst);
        let _ = SetConsoleCtrlHandler(Some(on_console_close), true);
        SetTimer(window, TIMER_ID, TIMER_MS, None);
        hub::hub().tray = true;
        hide_console();
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

fn icon_data(window: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: ICON_ID,
        ..Default::default()
    }
}

/// Windows' own application icon stands in until Zippy has a logo of its own (DESIGN.md: `[LOGO]`).
fn add_icon(window: HWND) -> windows::core::Result<()> {
    unsafe {
        let mut data = icon_data(window);
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = LoadIconW(None, IDI_APPLICATION)?;
        write_tip(&mut data, &tip_text());
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            return Err(windows::core::Error::from_win32());
        }
        // Version 4 reports a right-click or Shift+F10 as WM_CONTEXTMENU and a click or Enter as a selection.
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
    Ok(())
}

fn write_tip(data: &mut NOTIFYICONDATAW, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip = [0; 128];
    data.szTip[..wide.len()].copy_from_slice(&wide);
}

fn snapshot() -> (Phase, Option<String>) {
    let hub = hub::hub();
    (hub.phase.clone(), hub.target.as_ref().map(|t| t.app.clone()))
}

fn phase_text(phase: &Phase) -> &'static str {
    match phase {
        Phase::Idle => "stopped",
        Phase::Searching => "looking for Discord",
        Phase::Running => "running",
        Phase::Paused(_) => "paused",
        Phase::Banned => "stopped by a ban message",
    }
}

fn tip_text() -> String {
    format!("Zippy: {}", phase_text(&snapshot().0))
}

fn console() -> Option<HWND> {
    let window = unsafe { GetConsoleWindow() };
    (!window.is_invalid()).then_some(window)
}

/// Windows Terminal hands out a stand-in window that cannot really be hidden, so its window is left as it is.
fn is_terminal_stand_in(window: HWND) -> bool {
    let mut class = [0u16; 64];
    let length = unsafe { GetClassNameW(window, &mut class) };
    String::from_utf16_lossy(&class[..length.max(0) as usize]) == "PseudoConsoleWindow"
}

fn hide_console() {
    let Some(window) = console() else { return };
    if is_terminal_stand_in(window) {
        out!("[!] Zippy runs in Windows Terminal, whose window can't be hidden; it stays open. The tray menu still works.");
        return;
    }
    out!("[OK] This window now waits in the taskbar tray. Click the Zippy icon there to show it again;");
    out!("     minimizing it sends it back. Right-click the icon to start, pause, stop or quit Zippy.");
    unsafe {
        let _ = ShowWindow(window, SW_HIDE);
    }
}

fn show_console() {
    let Some(window) = console() else { return };
    unsafe {
        let _ = ShowWindow(window, if IsIconic(window).as_bool() { SW_RESTORE } else { SW_SHOW });
        let _ = SetForegroundWindow(window);
    }
}

/// Minimizing the shown window puts it back in the tray, as people expect from a tray program.
fn send_minimized_back() {
    let Some(window) = console() else { return };
    unsafe {
        if IsWindowVisible(window).as_bool() && IsIconic(window).as_bool() && !is_terminal_stand_in(window) {
            let _ = ShowWindow(window, SW_HIDE);
        }
    }
}

fn refresh_tip(window: HWND) {
    let text = tip_text();
    let changed = TIP.with(|tip| {
        let mut tip = tip.borrow_mut();
        let changed = *tip != text;
        if changed {
            tip.clone_from(&text);
        }
        changed
    });
    if changed {
        let mut data = icon_data(window);
        data.uFlags = NIF_TIP | NIF_SHOWTIP;
        write_tip(&mut data, &text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    }
}

/// The menu reads the state once, before it opens, and holds no lock while it is up.
fn show_menu(window: HWND, x: i32, y: i32) {
    let (phase, app) = snapshot();
    let status = match (&phase, &app) {
        (Phase::Idle, _) => "Zippy is stopped: press Start to look for Discord".to_owned(),
        (Phase::Searching, _) => "Looking for Discord…".to_owned(),
        (_, Some(app)) => format!("Discord detected ({app}), {}", phase_text(&phase)),
        (_, None) => format!("Zippy is {}", phase_text(&phase)),
    };
    let (toggle, toggle_on) = match phase {
        Phase::Idle => ("Start", true),
        Phase::Searching => ("Start", false),
        Phase::Running => ("Pause", true),
        Phase::Paused(_) | Phase::Banned => ("Resume", true),
    };
    let flags = |on: bool| if on { MF_STRING } else { MF_STRING | MF_GRAYED };
    let labels = [HSTRING::from(status.as_str()), HSTRING::from(toggle), HSTRING::from("Stop"), HSTRING::from("Exit Zippy")];
    let chosen = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(labels[0].as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, flags(toggle_on), ID_TOGGLE, PCWSTR(labels[1].as_ptr()));
        let _ = AppendMenuW(menu, flags(phase != Phase::Idle), ID_STOP, PCWSTR(labels[2].as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, PCWSTR(labels[3].as_ptr()));
        // Without this the menu would not close when the user clicks elsewhere.
        let _ = SetForegroundWindow(window);
        let chosen = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN).0, x, y, window, None);
        let _ = PostMessageW(window, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        chosen.0 as usize
    };
    // The state may have moved on while the menu was open; the main loop ignores a request that no longer fits.
    let control = match chosen {
        ID_TOGGLE if phase == Phase::Running => Control::Pause,
        ID_TOGGLE => Control::Start,
        ID_STOP => Control::Stop,
        ID_EXIT => Control::Quit,
        _ => return,
    };
    if !hub::control(control) {
        out!("[!] Zippy has too many requests waiting; try the tray menu again in a moment.");
    }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_TRAY {
        let event = (lparam.0 as u32) & 0xFFFF;
        if event == NIN_SELECT || event == NIN_KEYSELECT {
            show_console();
        } else if event == WM_CONTEXTMENU {
            let x = (wparam.0 & 0xFFFF) as u16 as i16 as i32;
            let y = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            show_menu(window, x, y);
        }
        return LRESULT(0);
    }
    if message == WM_TIMER {
        refresh_tip(window);
        send_minimized_back();
        return LRESULT(0);
    }
    if message != 0 && message == TASKBAR_CREATED.load(Ordering::SeqCst) {
        let _ = add_icon(window);
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Closing the console window ends Zippy at once; the icon goes first so it does not linger.
unsafe extern "system" fn on_console_close(event: u32) -> BOOL {
    if matches!(event, CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT) {
        remove();
    }
    BOOL(0)
}
