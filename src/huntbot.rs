//! Starts huntbot again each time it comes back. Before every hunt OwO shows a password as an image, which
//! Zippy does not read: the player types it on the dashboard or in the console, and Zippy sends the rest.

use std::time::{Duration, Instant};

use crate::gems::{ChatMessage, owo_content};
use crate::stats::{self, Reading};

pub const CHECK_COMMAND: &str = "owohb";
const CASH_COMMAND: &str = "owocash";
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);
/// OwO: "Password will reset in 5 minutes".
const PASSWORD_WINDOW: Duration = Duration::from_secs(5 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);
/// After huntbot is seen hunting, its countdown is trusted for at least this long before looking again.
const SETTLE: Duration = Duration::from_secs(5 * 60);
/// The countdown is shown in whole minutes, so huntbot can still be finishing for a moment after it ends.
const READY_GRACE_MS: u64 = 60_000;
/// Commands one start may take (collect, profile, cash, start, retries) before Zippy gives up for a while.
const MAX_SENDS: u32 = 6;
const MAX_PASSWORD: usize = 16;

/// A duration in tenths of an hour, as OwO's Duration trait gives it: 5 (0.5h) to 240 (24h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hours(u64);

impl Hours {
    pub fn from_tenths(tenths: u64) -> Option<Self> {
        (5..=240).contains(&tenths).then_some(Self(tenths))
    }

    /// "7.6h", or "24h" for a whole number.
    pub fn text(self) -> String {
        match self.0 % 10 {
            0 => format!("{}h", self.0 / 10),
            tenth => format!("{}.{tenth}h", self.0 / 10),
        }
    }
}

/// Passwords are a few letters; anything else would type something other than a huntbot command.
pub fn clean_password(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && text.len() <= MAX_PASSWORD && text.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| text.to_ascii_lowercase())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ask {
    /// Plain `owohb`: collects a finished hunt, or shows the profile with the duration and its cost.
    Check,
    Cash { hours: Hours, cost: u64 },
    Start(Hours),
    Submit(Hours, String),
}

impl Ask {
    fn command(&self) -> String {
        match self {
            Ask::Check => CHECK_COMMAND.to_owned(),
            Ask::Cash { .. } => CASH_COMMAND.to_owned(),
            Ask::Start(hours) => format!("{CHECK_COMMAND} {}", hours.text()),
            Ask::Submit(hours, password) => format!("{CHECK_COMMAND} {} {password}", hours.text()),
        }
    }
}

/// Why Zippy is waiting, for the dashboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Hunting,
    NoCowoncy { have: Option<u64>, need: Option<u64> },
    PasswordExpired,
    NoReply,
    NoDuration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Wait { why: Why, until: Option<Instant>, not_before: Instant },
    Send(Ask),
    Await { ask: Ask, after: u64, sent: Instant },
    Password { hours: Hours, until: Instant, wrong: bool },
}

#[derive(Debug, PartialEq, Eq)]
enum Reply {
    Password,
    /// "Please include your password!": the one OwO showed earlier is still valid.
    KeepPassword,
    WrongPassword,
    NoCowoncy,
    Started,
    Back,
    Hunting,
    Profile { hours: Option<u64>, cost: Option<u64> },
    Cash(u64),
}

/// Letters and digits only, upper case, so bold text, emoji and apostrophes do not split a phrase.
fn squash(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_uppercase).collect()
}

fn classify(content: &str, detail: &str, player: &str) -> Option<Reply> {
    let text = format!("{content} | {detail}");
    let s = squash(&text);
    // Collecting and the countdown do not name the player; they only count right after Zippy's own command.
    if s.contains("IAMBACKWITH") {
        return Some(Reply::Back);
    }
    let name = squash(player);
    let named = if name.is_empty() { text.contains(player) } else { s.contains(&name) };
    if s.contains("STILLHUNTING") && !s.contains("YOUSPENT") {
        return Some(Reply::Hunting);
    }
    if !named {
        return None;
    }
    if s.contains("HEREISYOURPASSWORD") {
        Some(Reply::Password)
    } else if s.contains("INCLUDEYOURPASSWORD") {
        Some(Reply::KeepPassword)
    } else if s.contains("PASSWORD") && ["WRONG", "INCORRECT", "INVALID"].iter().any(|w| s.contains(w)) {
        Some(Reply::WrongPassword)
    } else if s.contains("ENOUGHCOWONCY") {
        Some(Reply::NoCowoncy)
    } else if s.contains("YOUSPENT") {
        Some(Reply::Started)
    } else if text.contains(&format!("{player}'s HuntBot")) {
        Some(Reply::Profile { hours: stats::huntbot_hours(&text), cost: stats::huntbot_cost(&text) })
    } else {
        stats::cash_of(content, player).map(Reply::Cash)
    }
}

/// Something the player has to act on, so Zippy rings for it.
#[derive(Debug, PartialEq, Eq)]
pub enum Notice {
    Password { command: String },
    NoCowoncy { have: Option<u64>, need: Option<u64> },
}

/// What the dashboard shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub state: &'static str,
    /// When the wait or the password window ends, in Unix ms.
    pub until: Option<u64>,
    pub wrong_password: bool,
    pub have: Option<u64>,
    pub need: Option<u64>,
    /// The command the password will complete, such as `owohb 7.6h`.
    pub command: Option<String>,
}

impl View {
    pub fn off() -> Self {
        Self::of("off")
    }

    pub fn idle() -> Self {
        Self::of("idle")
    }

    fn of(state: &'static str) -> Self {
        Self { state, until: None, wrong_password: false, have: None, need: None, command: None }
    }
}

pub struct Huntbot {
    step: Step,
    sends: u32,
    password_until: Option<Instant>,
}

impl Huntbot {
    /// Waits for the huntbot countdown Zippy already knows; with none, it looks right away.
    pub fn new(now: Instant) -> Self {
        Self { step: Step::Wait { why: Why::Hunting, until: None, not_before: now }, sends: 0, password_until: None }
    }

    /// The command to send next, if one is due.
    pub fn due(&self) -> Option<String> {
        match &self.step {
            Step::Send(ask) => Some(ask.command()),
            _ => None,
        }
    }

    /// While OwO's password image is the newest thing in the chat, nothing else is sent, so it stays in view.
    pub fn holds(&self) -> bool {
        matches!(
            &self.step,
            Step::Password { .. } | Step::Send(Ask::Submit(..)) | Step::Await { ask: Ask::Start(_) | Ask::Submit(..), .. }
        )
    }

    pub fn sent(&mut self, after: u64, now: Instant) {
        if let Step::Send(ask) = &self.step {
            self.sends += 1;
            self.step = Step::Await { ask: ask.clone(), after, sent: now };
        }
    }

    /// Starts a new attempt now, whatever Zippy was waiting for. Refused while OwO is answering, or while the
    /// password it showed is still valid: a new `owohb` would only be asked for that same password again.
    pub fn retry(&mut self) -> Result<(), &'static str> {
        match self.step {
            Step::Send(_) | Step::Await { .. } => Err("Zippy is already waiting for OwO's huntbot reply"),
            Step::Password { .. } => Err("OwO is waiting for the password from its image; type that instead"),
            Step::Wait { .. } => {
                self.sends = 0;
                self.step = Step::Send(Ask::Check);
                Ok(())
            }
        }
    }

    pub fn submit(&mut self, password: &str) -> Result<String, &'static str> {
        let Step::Password { hours, .. } = self.step else {
            return Err("huntbot is not waiting for a password right now");
        };
        let password = clean_password(password).ok_or("the password is the letters in OwO's image, nothing else")?;
        let ask = Ask::Submit(hours, password);
        let command = ask.command();
        self.step = Step::Send(ask);
        Ok(command)
    }

    fn wait(&mut self, why: Why, until: Option<Instant>, not_before: Instant) {
        self.step = Step::Wait { why, until, not_before };
    }

    /// Moves on with time: a countdown that ran out, a reply that never came, a password that expired.
    pub fn tick(&mut self, now: Instant, now_ms: u64, ready_at: Option<Reading>) {
        match &self.step {
            Step::Wait { until, not_before, .. } if now >= *not_before => {
                let due = match until {
                    Some(until) => now >= *until,
                    None => ready_at.is_none_or(|r| r.value.saturating_add(READY_GRACE_MS) <= now_ms),
                };
                if due {
                    self.sends = 0;
                    self.step = Step::Send(Ask::Check);
                }
            }
            Step::Await { ask, sent, .. } if now >= *sent + REPLY_TIMEOUT => {
                if self.sends < MAX_SENDS {
                    out!("[HUNTBOT] No reply to {}; sending it again.", ask.command());
                    self.step = Step::Send(ask.clone());
                } else {
                    out!("[HUNTBOT] OwO did not answer the huntbot commands; trying again in 30 minutes.");
                    self.wait(Why::NoReply, Some(now + RETRY_AFTER), now);
                }
            }
            Step::Password { until, .. } if now >= *until => {
                out!("[HUNTBOT] No password came within 5 minutes, so OwO reset it. Asking again in 30 minutes.");
                self.password_until = None;
                self.wait(Why::PasswordExpired, Some(now + RETRY_AFTER), now);
            }
            _ => {}
        }
    }

    /// Reads OwO's answer to the command Zippy sent last.
    pub fn observe(&mut self, messages: &[ChatMessage], player: Option<&str>, cash: Option<u64>, now: Instant) -> Option<Notice> {
        let Step::Await { ask, after, .. } = &self.step else { return None };
        let player = player?;
        let mut newer: Vec<&ChatMessage> = messages.iter().filter(|m| m.id > *after).collect();
        newer.sort_unstable_by_key(|m| m.id);
        let reply = newer.into_iter().find_map(|m| {
            let reply = classify(owo_content(m)?, &m.detail, player)?;
            let fits = match ask {
                Ask::Cash { .. } => matches!(reply, Reply::Cash(_)),
                Ask::Check => !matches!(reply, Reply::Cash(_)),
                Ask::Start(_) | Ask::Submit(..) => !matches!(reply, Reply::Cash(_) | Reply::Profile { .. }),
            };
            fits.then_some(reply)
        })?;
        let ask = ask.clone();
        self.answer(ask, reply, cash, now)
    }

    fn answer(&mut self, ask: Ask, reply: Reply, cash: Option<u64>, now: Instant) -> Option<Notice> {
        let hours = match &ask {
            Ask::Cash { hours, .. } | Ask::Start(hours) | Ask::Submit(hours, _) => Some(*hours),
            Ask::Check => None,
        };
        let need = match &ask {
            Ask::Cash { cost, .. } => Some(*cost),
            _ => None,
        };
        match reply {
            Reply::Back => {
                out!("[HUNTBOT] Huntbot is back; reading its profile next.");
                self.step = Step::Send(Ask::Check);
            }
            Reply::Hunting => {
                self.wait(Why::Hunting, None, now + SETTLE);
            }
            Reply::Started => {
                out!("[HUNTBOT] Huntbot is hunting again.");
                self.sends = 0;
                self.password_until = None;
                self.wait(Why::Hunting, None, now + SETTLE);
            }
            Reply::Profile { hours: None, .. } => {
                out!("[HUNTBOT] Could not read the duration from the owohb profile; trying again in 30 minutes.");
                self.wait(Why::NoDuration, Some(now + RETRY_AFTER), now);
            }
            Reply::Profile { hours: Some(tenths), cost } => {
                let hours = Hours::from_tenths(tenths)?;
                self.step = Step::Send(match cost {
                    Some(cost) => Ask::Cash { hours, cost },
                    None => Ask::Start(hours),
                });
            }
            Reply::Cash(have) => {
                let hours = hours?;
                if need.is_some_and(|need| have < need) {
                    return Some(self.short(Some(have), need, now));
                }
                self.step = Step::Send(Ask::Start(hours));
            }
            Reply::NoCowoncy => return Some(self.short(cash, need, now)),
            Reply::Password | Reply::KeepPassword | Reply::WrongPassword => {
                let hours = hours?;
                let wrong = reply == Reply::WrongPassword;
                let until = match (&reply, self.password_until) {
                    (Reply::Password, _) | (_, None) => now + PASSWORD_WINDOW,
                    (_, Some(until)) => until,
                };
                self.password_until = Some(until);
                self.step = Step::Password { hours, until, wrong };
                let command = Ask::Start(hours).command();
                return match reply {
                    Reply::WrongPassword => {
                        out!("[HUNTBOT] OwO did not accept that password. Check the image and type it again.");
                        None
                    }
                    Reply::KeepPassword => {
                        out!("[HUNTBOT] OwO still has the password it showed earlier. Type it to finish {command}.");
                        Some(Notice::Password { command })
                    }
                    _ => {
                        out!("[HUNTBOT] OwO posted a password image. Type its letters on the dashboard, or here, and press Enter.");
                        Some(Notice::Password { command })
                    }
                };
            }
        }
        None
    }

    fn short(&mut self, have: Option<u64>, need: Option<u64>, now: Instant) -> Notice {
        let amounts = match (have, need) {
            (Some(have), Some(need)) => format!(" (a full hunt costs {need}, you have {have})"),
            _ => String::new(),
        };
        out!("[HUNTBOT] Not enough cowoncy for huntbot{amounts}. Trying again in 30 minutes.");
        self.password_until = None;
        self.wait(Why::NoCowoncy { have, need }, Some(now + RETRY_AFTER), now);
        Notice::NoCowoncy { have, need }
    }

    pub fn view(&self, now: Instant, now_ms: u64) -> View {
        let at = |instant: Instant| now_ms + instant.saturating_duration_since(now).as_millis() as u64;
        match &self.step {
            Step::Wait { why, until, .. } => {
                let (state, have, need) = match *why {
                    Why::Hunting => ("waiting", None, None),
                    Why::NoCowoncy { have, need } => ("no_cowoncy", have, need),
                    Why::PasswordExpired => ("expired", None, None),
                    Why::NoReply => ("no_reply", None, None),
                    Why::NoDuration => ("no_duration", None, None),
                };
                View { until: until.map(at), have, need, ..View::of(state) }
            }
            Step::Send(_) | Step::Await { .. } => View::of("working"),
            Step::Password { hours, until, wrong } => View {
                until: Some(at(*until)),
                wrong_password: *wrong,
                command: Some(Ask::Start(*hours).command()),
                ..View::of("password")
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake_at;

    const T: u64 = 1_767_268_800_000;
    // Summaries below are copied from `zippy --check --debug` on a real channel, except PASSWORD and BROKE,
    // which follow the screenshots of those replies.
    const PROFILE: &str = "OwO Verified App , Ho-oh's HuntBot BEEP. BOOP. I AM HUNTBOT. I WILL HUNT FOR YOU MASTER. Use the command owo autohunt {cowoncy} to get started. You can use owo upgrade {trait} {count} to upgrade the traits below. To obtain more essence, use owo sacrifice {animal} {count}. ⏱️ Efficiency - 95/H Lvl 70 [493/17218] [■□□□□□□□□□□□□□□□] ⏳ Duration - 7.6H Lvl 71 [8271/14370] [■■■■■■■■■■□□□□□□] :cowoncy: Cost - 8 , 6:43 PM";
    const PROFILE_DETAIL: &str = "Efficiency - || 95/H || Lvl 70 || Duration - || 7.6H || Lvl 71 || [8271/14370] || Cost - || 8 cowoncy || Animal Essence - || 21,026 || Current Max Autohunt: 722 animals, 9,310 essence, and 12,502 xp for 5,776 cowoncy";
    const PASSWORD: &str = "OwO Verified App , 🔐 | Ho-oh, Here is your password! :blank: | Use the command owo autohunt {amount} {password} Image , 6:04 PM";
    const INCLUDE: &str = "OwO Verified App , 🚫 | Ho-oh, Please include your password! The command is owo autohunt 1216 {password}! :blank: | Password will reset in 5 minutes , 6:09 PM";
    const BROKE: &str = "OwO Verified App , 🚫 | Ho-oh, You don't have enough cowoncy! , 6:04 PM";
    const SPENT: &str = "OwO Verified App , :rbot: | BEEP BOOP. Ho-oh, YOU SPENT 1,216 cowoncy :blank: | I WILL BE BACK IN 1H 36M WITH 152 ANIMALS, :blank: | 1960 ESSENCE, AND 2632 EXPERIENCE , 6:10 PM";
    const BACK: &str = "OwO Verified App , :mbot: | BEEP BOOP. I AM BACK WITH 722 ANIMALS, :blank: | 9310 ESSENCE, AND 12502 EXPERIENCE , 6:10 PM";
    const CASH: &str = "OwO Verified App , :cowoncy: | Ho-oh, you currently have 3,953 cowoncy! , 6:09 PM";

    fn msg(at: u64, summary: &str, detail: &str) -> ChatMessage {
        ChatMessage { id: snowflake_at(at), summary: summary.to_owned(), detail: detail.to_owned(), from_owo: true }
    }

    /// Sends what is due and answers it with `reply`, posted `offset` ms after the start.
    fn exchange(bot: &mut Huntbot, offset: u64, reply: &ChatMessage, now: Instant) -> (String, Option<Notice>) {
        let command = bot.due().expect("a command is due");
        bot.sent(snowflake_at(T + offset - 1), now);
        let notice = bot.observe(std::slice::from_ref(reply), Some("Ho-oh"), None, now);
        (command, notice)
    }

    #[test]
    fn durations_are_written_as_owo_reads_them() {
        assert_eq!(Hours::from_tenths(76).unwrap().text(), "7.6h");
        assert_eq!(Hours::from_tenths(5).unwrap().text(), "0.5h");
        assert_eq!(Hours::from_tenths(240).unwrap().text(), "24h");
        assert_eq!(Hours::from_tenths(241), None);
        assert_eq!(stats::huntbot_hours(PROFILE_DETAIL), Some(76));
        assert_eq!(stats::huntbot_hours("Duration - || Lvl 71 || [8271/14370]"), Some(76), "from the level alone");
        assert_eq!(stats::huntbot_hours("Duration - || Lvl 235"), Some(240));
        assert_eq!(stats::huntbot_hours("Duration - || 99.0H"), None);
        assert_eq!(stats::huntbot_cost(PROFILE_DETAIL), Some(5776));
    }

    #[test]
    fn passwords_are_letters_only() {
        assert_eq!(clean_password("  Sured "), Some("sured".to_owned()));
        assert_eq!(clean_password(""), None);
        assert_eq!(clean_password("su red"), None);
        assert_eq!(clean_password("sured\nowo give"), None);
        assert_eq!(clean_password(&"a".repeat(17)), None);
    }

    #[test]
    fn a_full_start_reads_the_profile_checks_cash_and_asks_for_the_password() {
        let now = Instant::now();
        let mut bot = Huntbot::new(now);
        assert!(bot.due().is_none());
        bot.tick(now, T, Some(Reading { value: T - READY_GRACE_MS, at: T - 100 }));
        assert_eq!(exchange(&mut bot, 1000, &msg(T + 1000, BACK, ""), now).0, "owohb");
        assert_eq!(exchange(&mut bot, 2000, &msg(T + 2000, PROFILE, PROFILE_DETAIL), now).0, "owohb");
        let cash = msg(T + 3000, &CASH.replace("3,953", "9,000"), "");
        assert_eq!(exchange(&mut bot, 3000, &cash, now).0, "owocash");
        let (command, notice) = exchange(&mut bot, 4000, &msg(T + 4000, PASSWORD, ""), now);
        assert_eq!(command, "owohb 7.6h");
        assert_eq!(notice, Some(Notice::Password { command: "owohb 7.6h".to_owned() }));
        assert!(bot.holds(), "nothing else is sent while the password image is up");
        assert_eq!(bot.view(now, T).state, "password");
        assert!(bot.retry().is_err(), "a new owohb would throw away the password OwO is waiting for");

        assert!(bot.submit("su red").is_err());
        assert_eq!(bot.submit("Sured"), Ok("owohb 7.6h sured".to_owned()));
        let (command, notice) = exchange(&mut bot, 5000, &msg(T + 5000, SPENT, ""), now);
        assert_eq!((command.as_str(), notice), ("owohb 7.6h sured", None));
        assert!(!bot.holds());
        assert_eq!(bot.view(now, T).state, "waiting");
    }

    #[test]
    fn too_little_cowoncy_waits_before_the_password_is_asked() {
        let now = Instant::now();
        let mut bot = Huntbot::new(now);
        bot.tick(now, T, None);
        exchange(&mut bot, 1000, &msg(T + 1000, PROFILE, PROFILE_DETAIL), now);
        let (_, notice) = exchange(&mut bot, 2000, &msg(T + 2000, CASH, ""), now);
        assert_eq!(notice, Some(Notice::NoCowoncy { have: Some(3953), need: Some(5776) }));
        assert!(bot.due().is_none() && !bot.holds());
        bot.tick(now + RETRY_AFTER, T, None);
        assert_eq!(bot.due().as_deref(), Some("owohb"), "tries again after 30 minutes");
    }

    #[test]
    fn owo_refusing_for_cowoncy_after_the_password_stops_until_the_retry() {
        let now = Instant::now();
        let mut bot = Huntbot::new(now);
        bot.tick(now, T, None);
        exchange(&mut bot, 1000, &msg(T + 1000, PROFILE, PROFILE_DETAIL.split(" || Current").next().unwrap()), now);
        let (command, _) = exchange(&mut bot, 2000, &msg(T + 2000, PASSWORD, ""), now);
        assert_eq!(command, "owohb 7.6h", "without a known cost the hunt is tried straight away");
        bot.submit("sured").unwrap();
        let (_, notice) = exchange(&mut bot, 3000, &msg(T + 3000, BROKE, ""), now);
        assert!(matches!(notice, Some(Notice::NoCowoncy { .. })));
        assert_eq!(bot.view(now, T).state, "no_cowoncy");
    }

    #[test]
    fn a_password_still_valid_keeps_its_window_and_a_late_one_expires() {
        let now = Instant::now();
        let mut bot = Huntbot::new(now);
        bot.tick(now, T, None);
        exchange(&mut bot, 1000, &msg(T + 1000, &PROFILE.replace("7.6H Lvl 71", "1.6H Lvl 11"), ""), now);
        let (_, notice) = exchange(&mut bot, 2000, &msg(T + 2000, INCLUDE, ""), now);
        assert_eq!(notice, Some(Notice::Password { command: "owohb 1.6h".to_owned() }));
        assert!(bot.submit("sured").is_ok());
        bot.sent(snowflake_at(T + 2500), now);
        let wrong = "OwO Verified App , 🚫 | Ho-oh , Wrong password! , 6:10 PM";
        assert_eq!(bot.observe(&[msg(T + 3000, wrong, "")], Some("Ho-oh"), None, now), None);
        assert!(bot.view(now, T).wrong_password);
        bot.tick(now + PASSWORD_WINDOW, T, None);
        assert_eq!(bot.view(now, T).state, "expired");
        assert!(bot.submit("sured").is_err());
    }

    #[test]
    fn replies_for_someone_else_or_from_before_the_command_are_ignored() {
        let now = Instant::now();
        let mut bot = Huntbot::new(now);
        bot.tick(now, T, None);
        bot.sent(snowflake_at(T + 1000), now);
        let early = msg(T + 500, PASSWORD, "");
        let other = msg(T + 2000, &PASSWORD.replace("Ho-oh", "Someone"), "");
        let mut forged = msg(T + 3000, PASSWORD, "");
        forged.from_owo = false;
        assert_eq!(bot.observe(&[early, other, forged], Some("Ho-oh"), None, now), None);
        assert_eq!(bot.view(now, T).state, "working");
    }

    #[test]
    fn a_hunting_huntbot_is_left_alone_and_silence_gives_up_after_a_few_tries() {
        let now = Instant::now();
        let mut bot = Huntbot::new(now);
        bot.tick(now, T, Some(Reading { value: T + 3_600_000, at: T }));
        assert!(bot.due().is_none(), "the countdown is still running");
        assert!(bot.retry().is_ok());
        let hunting = "OwO Verified App , HUNTBOT is currently hunting! BEEP BOOP. I AM STILL HUNTING. I WILL BE BACK IN 18H 7M , 6:23 PM";
        exchange(&mut bot, 1000, &msg(T + 1000, hunting, ""), now);
        bot.tick(now + SETTLE, T + READY_GRACE_MS, Some(Reading { value: T, at: T }));
        assert_eq!(bot.due().as_deref(), Some("owohb"));
        let mut later = now + SETTLE;
        for _ in 0..MAX_SENDS {
            bot.sent(0, later);
            later += REPLY_TIMEOUT;
            bot.tick(later, T, None);
        }
        assert_eq!(bot.view(later, T).state, "no_reply");
    }
}
