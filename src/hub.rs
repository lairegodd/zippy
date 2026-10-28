use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use crate::daily::Daily;
use crate::huntbot;
use crate::stats::Stats;

const LOG_LINES: usize = 300;
const MAX_CONTROLS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    Start,
    Pause,
    Stop,
    Refresh(Reads),
    /// Start huntbot now instead of waiting for its countdown or a retry.
    Huntbot,
    /// The letters from OwO's huntbot password image, typed by the player.
    HuntbotPassword(String),
    /// Send this command ahead of the routine ones, from the dashboard's right-click menu.
    Send(Command),
    /// Developer mode only: behave exactly as if OwO had posted a captcha.
    SimulateCaptcha,
    /// Developer mode only: log the newest chat messages as Zippy reads them.
    DumpChat,
}

/// Which dashboard panel a refresh reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reads {
    Statistics,
    Reminders,
}

/// The commands the dashboard's right-click menu can ask for. The browser only names one of these, so no text
/// from it is ever typed into Discord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Huntbot,
    Daily(Daily),
    Cash,
    Cookie,
    BossTickets,
    Vote,
    Pray,
    Battle,
}

impl Command {
    const ALL: [Command; 12] = [
        Command::Huntbot,
        Command::Daily(Daily::Claim),
        Command::Daily(Daily::Run),
        Command::Daily(Daily::Pup),
        Command::Daily(Daily::Piku),
        Command::Daily(Daily::Army),
        Command::Cash,
        Command::Cookie,
        Command::BossTickets,
        Command::Vote,
        Command::Pray,
        Command::Battle,
    ];

    pub fn text(self) -> &'static str {
        match self {
            Command::Huntbot => "owohb",
            Command::Daily(daily) => daily.command(),
            Command::Cash => "owocash",
            Command::Cookie => "owo cookie",
            Command::BossTickets => "owoboss t",
            Command::Vote => "owovote",
            Command::Pray => "owopray",
            Command::Battle => "owob",
        }
    }

    pub fn from_text(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.text() == text)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Searching,
    Running,
    Paused(String),
    Banned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DailyView {
    pub command: &'static str,
    pub left: Option<u32>,
    pub limit: Option<u32>,
    pub used_up: bool,
    pub retry_at: Option<u64>,
}

pub struct Target {
    pub app: String,
    pub title: String,
}

pub struct Hub {
    pub phase: Phase,
    pub target: Option<Target>,
    pub stats: Stats,
    pub dailies: Vec<DailyView>,
    pub huntbot: huntbot::View,
    /// Developer mode (`--debug`): captcha simulation and raw chat dumps.
    pub debug: bool,
    log: VecDeque<(u64, String)>,
    log_seq: u64,
    controls: VecDeque<Control>,
}

static HUB: LazyLock<Mutex<Hub>> = LazyLock::new(|| {
    Mutex::new(Hub {
        phase: Phase::Idle,
        target: None,
        stats: Stats::default(),
        dailies: Vec::new(),
        debug: false,
        huntbot: huntbot::View::off(),
        log: VecDeque::new(),
        log_seq: 0,
        controls: VecDeque::new(),
    })
});

/// A panic while the lock was held must not take the dashboard down with it.
pub fn hub() -> MutexGuard<'static, Hub> {
    HUB.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Prints a line and keeps it for the dashboard's log.
pub fn say(line: String) {
    println!("{line}");
    let text = without_clock(line.trim_start_matches('\n'));
    if text.is_empty() {
        return;
    }
    let mut hub = hub();
    hub.log_seq += 1;
    let entry = (crate::unix_ms_now(), text.to_owned());
    if hub.log.len() == LOG_LINES {
        hub.log.pop_front();
    }
    hub.log.push_back(entry);
}

/// The dashboard stamps every line itself, so the "[12:34:56] " some console lines start with goes.
fn without_clock(text: &str) -> &str {
    let bytes = text.as_bytes();
    let is_clock = bytes.len() >= 11
        && bytes[0] == b'['
        && bytes[9] == b']'
        && bytes[10] == b' '
        && bytes[1..9].iter().enumerate().all(|(i, b)| if i % 3 == 2 { *b == b':' } else { b.is_ascii_digit() });
    if is_clock { &text[11..] } else { text }
}

#[macro_export]
macro_rules! out {
    () => { $crate::hub::say(String::new()) };
    ($($arg:tt)*) => { $crate::hub::say(format!($($arg)*)) };
}

impl Hub {
    pub fn log(&self) -> impl Iterator<Item = &(u64, String)> {
        self.log.iter()
    }

    pub fn log_seq(&self) -> u64 {
        self.log_seq
    }

    pub fn take_control(&mut self) -> Option<Control> {
        self.controls.pop_front()
    }
}

/// Repeated clicks collapse, so a flood of requests cannot grow the queue. A command already waiting to be
/// asked for is not queued twice, wherever it sits. Returns false when the queue is full and nothing was added.
pub fn control(control: Control) -> bool {
    let mut hub = hub();
    let duplicate = match &control {
        Control::Send(_) => hub.controls.contains(&control),
        _ => hub.controls.back() == Some(&control),
    };
    if duplicate {
        return true;
    }
    if hub.controls.len() >= MAX_CONTROLS {
        return false;
    }
    hub.controls.push_back(control);
    true
}

pub fn set_phase(phase: Phase) {
    hub().phase = phase;
}

#[cfg(test)]
mod tests {
    use super::without_clock;

    #[test]
    fn console_clock_is_dropped_but_other_brackets_stay() {
        assert_eq!(without_clock("[19:20:01] Zippy running again."), "Zippy running again.");
        assert_eq!(without_clock("[OK] Dashboard opened."), "[OK] Dashboard opened.");
        assert_eq!(without_clock("[DAILY] owoarmy: 13 left today."), "[DAILY] owoarmy: 13 left today.");
    }
}
