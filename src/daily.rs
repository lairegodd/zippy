use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::gems::{ChatMessage, owo_content, split_summary};
use crate::snowflake_unix_ms;

pub const FILE_NAME: &str = "zippy-dailies.json";

const REPLY_TIMEOUT: Duration = Duration::from_secs(20);
const SILENT_TRIES: u32 = 2;
const MAX_SENDS_PER_DAY: u32 = 40;
/// Another echo of the same command this close to OwO's reply means the reply may not be ours.
const ECHO_WINDOW_MS: u64 = 10_000;
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;
const RESET_MARGIN_MS: u64 = MINUTE_MS;
/// A command still used up right after the reset is tried again hourly for this long.
const RECHECK_WINDOW_MS: u64 = 3 * HOUR_MS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Daily {
    Claim,
    Run,
    Pup,
    Piku,
    Army,
}

impl Daily {
    pub const ALL: [Daily; 5] = [Daily::Claim, Daily::Run, Daily::Pup, Daily::Piku, Daily::Army];

    pub fn key(self) -> &'static str {
        match self {
            Daily::Claim => "daily",
            Daily::Run => "run",
            Daily::Pup => "pup",
            Daily::Piku => "piku",
            Daily::Army => "army",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            Daily::Claim => "owodaily",
            Daily::Run => "oworun",
            Daily::Pup => "owopup",
            Daily::Piku => "owopiku",
            Daily::Army => "owoarmy",
        }
    }

    /// OwO's cooldown plus two seconds for lag, counted from the send.
    fn cooldown(self) -> Duration {
        match self {
            Daily::Army => Duration::from_secs(12),
            Daily::Claim | Daily::Run | Daily::Pup | Daily::Piku => Duration::from_secs(62),
        }
    }

    fn markers(self) -> &'static [&'static str] {
        match self {
            Daily::Claim => &["here is your daily", "you need to wait", "your next daily"],
            Daily::Run => &[":run:", "kilometer"],
            Daily::Pup => &[":puppy:", "pupp"],
            Daily::Piku => &["🥕", "carrot", "pikpik"],
            Daily::Army => &["army emblem", ":rusty:"],
        }
    }

    /// Uses left today and, when the reply states it, today's limit. The first use of a day states the
    /// limit ("You can adopt 3 puppies today"); later uses say "2/3". `owodaily` is one use a day, and
    /// both its claim and its "wait" reply mean none is left.
    fn remaining(self, content: &str) -> Option<(u32, Option<u32>)> {
        if self == Daily::Claim {
            let lower = content.to_lowercase();
            return (lower.contains("here is your daily") || lower.contains("you need to wait")).then_some((0, Some(1)));
        }
        if self == Daily::Run && content.contains("too tired") {
            return Some((0, None));
        }
        today_lines(content).find_map(|line| {
            used_of_limit(line).or_else(|| {
                let stated = |limit: u32| (limit.saturating_sub(1), Some(limit));
                match self {
                    Daily::Claim | Daily::Run => None,
                    Daily::Pup => number_after(line, "You can adopt ").map(stated),
                    Daily::Piku => number_after(line, "You can still harvest ").map(stated),
                    Daily::Army => number_after(line.split_once("Today's remaining")?.1, ":").map(|left| (left, None)),
                }
            })
        })
    }
}

/// Lines about today; a line about tomorrow's limit must not be read as today's.
fn today_lines(content: &str) -> impl Iterator<Item = &str> {
    content.split('|').filter(|line| {
        let line = line.to_lowercase();
        line.contains("today") && !line.contains("tomorrow")
    })
}

fn used_of_limit(line: &str) -> Option<(u32, Option<u32>)> {
    line.split_whitespace().find_map(|word| {
        let (used, limit) = word.split_once('/')?;
        let limit = limit.parse::<u32>().ok()?;
        Some((limit.saturating_sub(used.parse().ok()?), Some(limit)))
    })
}

fn leading_number(text: &str) -> Option<u32> {
    let end = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    text[..end].parse().ok()
}

fn number_after(content: &str, marker: &str) -> Option<u32> {
    leading_number(content.split_once(marker)?.1.trim_start())
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

fn year_of(days: i64) -> i64 {
    let z = days + 719_468;
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_based_month = (5 * day_of_year + 2) / 153;
    year_of_era + z.div_euclid(146_097) * 400 + i64::from(march_based_month >= 10)
}

fn sunday_on_or_after(days: i64) -> i64 {
    days + (7 - (days + 4).rem_euclid(7)) % 7
}

/// US daylight time runs from the second Sunday of March to the first Sunday of November, switching at 2 AM,
/// so midnight belongs to daylight time on the days strictly after the first and up to the second.
fn pacific_midnight_ms(days: i64) -> u64 {
    let year = year_of(days);
    let starts = sunday_on_or_after(days_from_civil(year, 3, 8));
    let ends = sunday_on_or_after(days_from_civil(year, 11, 1));
    let offset = if days > starts && days <= ends { 7 * HOUR_MS } else { 8 * HOUR_MS };
    days as u64 * DAY_MS + offset
}

/// Days since 1970 of the OwO day (US Pacific date) that `unix_ms` falls in.
fn owo_day(unix_ms: u64) -> i64 {
    let day = (unix_ms.saturating_sub(8 * HOUR_MS) / DAY_MS) as i64;
    if unix_ms >= pacific_midnight_ms(day + 1) { day + 1 } else { day }
}

fn next_reset(unix_ms: u64) -> u64 {
    pacific_midnight_ms(owo_day(unix_ms) + 1) + RESET_MARGIN_MS
}

fn retry_time(unix_ms: u64, recheck: bool) -> u64 {
    let since_reset = unix_ms.saturating_sub(pacific_midnight_ms(owo_day(unix_ms)));
    if recheck && since_reset < RECHECK_WINDOW_MS { unix_ms + HOUR_MS } else { next_reset(unix_ms) }
}

fn wait_text(ms: u64) -> String {
    let minutes = ms.div_ceil(MINUTE_MS);
    if minutes < 60 { format!("{minutes} min") } else { format!("{} h {} min", minutes / 60, minutes % 60) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Unknown,
    /// Uses left, as read on OwO day `day`; a new day's count is not compared with an old one.
    Left { left: u32, day: i64 },
    UsedUp { retry_at: u64 },
}

#[derive(Clone, Copy)]
struct Pending {
    after_id: u64,
    sent: Instant,
    recheck: bool,
}

struct Tracker {
    daily: Daily,
    status: Status,
    /// Today's limit, as stated by OwO or inferred from the most uses seen left; kept across days as a first guess.
    limit: Option<u32>,
    pending: Option<Pending>,
    next: Option<Instant>,
    silent: u32,
    sends_today: (i64, u32),
}

impl Tracker {
    fn new(daily: Daily) -> Self {
        Self { daily, status: Status::Unknown, limit: None, pending: None, next: None, silent: 0, sends_today: (0, 0) }
    }

    /// A count is only restored on the OwO day it was read.
    fn restore(&mut self, entry: &Value, unix_ms: u64) {
        let number = |key: &str| entry[key].as_u64().and_then(|v| u32::try_from(v).ok());
        let today = owo_day(unix_ms);
        let on_today = |key: &str| entry[key].as_i64() == Some(today);
        self.limit = number("limit").filter(|&limit| limit > 0);
        if on_today("sends_day")
            && let Some(sends) = number("sends")
        {
            self.sends_today = (today, sends);
        }
        // A hand-edited or corrupt time must not hold a command back for longer than one OwO day.
        let retry_at = entry["retry_at"].as_u64().filter(|&at| at > unix_ms && at <= next_reset(unix_ms) + HOUR_MS);
        if let Some(retry_at) = retry_at {
            self.status = Status::UsedUp { retry_at };
        } else if on_today("left_day")
            && let Some(left) = number("left")
        {
            self.status = Status::Left { left, day: today };
        }
    }

    fn snapshot(&self) -> Value {
        let (left, left_day) = match self.status {
            Status::Left { left, day } => (Some(left), Some(day)),
            _ => (None, None),
        };
        let (sends_day, sends) = self.sends_today;
        json!({
            "left": left,
            "left_day": left_day,
            "retry_at": if let Status::UsedUp { retry_at } = self.status { Some(retry_at) } else { None },
            "limit": self.limit,
            "sends": sends,
            "sends_day": sends_day,
        })
    }

    fn due(&self, now: Instant, unix_ms: u64) -> bool {
        self.blocked(now, unix_ms).is_none()
    }

    /// Why this command cannot be sent now, if it cannot.
    fn blocked(&self, now: Instant, unix_ms: u64) -> Option<&'static str> {
        let (day, sends) = self.sends_today;
        if self.pending.is_some() {
            Some("Zippy is still waiting for OwO's reply to it")
        } else if self.next.is_some_and(|next| now < next) {
            Some("its cooldown has not ended yet")
        } else if day == owo_day(unix_ms) && sends >= MAX_SENDS_PER_DAY {
            Some("it was sent the most times Zippy allows in a day")
        } else if matches!(self.status, Status::UsedUp { retry_at } if unix_ms < retry_at) {
            Some("it is used up for today")
        } else {
            None
        }
    }

    fn sent(&mut self, after_id: u64, now: Instant, unix_ms: u64) {
        let recheck = matches!(self.status, Status::UsedUp { .. });
        self.pending = Some(Pending { after_id, sent: now, recheck });
        let day = owo_day(unix_ms);
        let sends = if self.sends_today.0 == day { self.sends_today.1 + 1 } else { 1 };
        self.sends_today = (day, sends);
        if sends == MAX_SENDS_PER_DAY {
            out!("[DAILY] {}: sent {sends} times today; no more until the reset.", self.daily.command());
        }
    }

    fn stop_until_reset(&mut self, unix_ms: u64, recheck: bool) {
        let retry_at = retry_time(unix_ms, recheck);
        self.status = Status::UsedUp { retry_at };
        let now_ms = crate::unix_ms_now().max(unix_ms);
        out!(
            "[DAILY] {}: used up for today; next try in {}.",
            self.daily.command(),
            wait_text(retry_at.saturating_sub(now_ms))
        );
    }

    /// Takes the first readable OwO reply in this command's wording after the send.
    fn observe(&mut self, messages: &[ChatMessage], now: Instant, unix_ms: u64) {
        let Some(pending) = self.pending else { return };
        let command = self.daily.command();
        let mut replies: Vec<(u64, &str)> = messages
            .iter()
            .filter(|m| m.id > pending.after_id)
            .filter_map(|m| Some((m.id, owo_content(m)?)))
            .filter(|(_, content)| {
                let content = content.to_lowercase();
                self.daily.markers().iter().any(|marker| content.contains(marker))
            })
            .collect();
        replies.sort_unstable_by_key(|(id, _)| *id);
        let mut unreadable = None;
        for (id, content) in replies {
            let Some((left, stated)) = self.daily.remaining(content) else {
                unreadable.get_or_insert(content);
                continue;
            };
            self.pending = None;
            self.silent = 0;
            self.next = Some(pending.sent + self.daily.cooldown());
            if other_echo_near(messages, command, id) {
                out!("[DAILY] {command}: someone else sent it at the same time; checking again later.");
                return;
            }
            self.limit = stated.or(match self.limit {
                Some(limit) => Some(limit.max(left + 1)),
                None => (left > 0).then_some(left + 1),
            });
            // A pause can hold the reply back past the reset; the day it belongs to is the day it was posted.
            let posted = snowflake_unix_ms(id).clamp(unix_ms.saturating_sub(12 * HOUR_MS), unix_ms);
            let day = owo_day(posted);
            let stuck = matches!(self.status, Status::Left { left: before, day: then } if then == day && left >= before);
            if left > 0 && !stuck {
                self.status = Status::Left { left, day };
                out!("[DAILY] {command}: {left} left today.");
                return;
            }
            if stuck {
                out!("[DAILY] {command}: OwO still says {left} left after another use, so it stops to be safe.");
            }
            self.stop_until_reset(posted, pending.recheck);
            return;
        }
        if now < pending.sent + REPLY_TIMEOUT {
            return;
        }
        self.pending = None;
        self.next = Some((pending.sent + self.daily.cooldown()).max(now));
        if let Some(content) = unreadable {
            let excerpt: String = content.chars().take(160).collect();
            out!("[DAILY] {command}: could not read OwO's reply: \"{excerpt}\"");
            self.stop_until_reset(unix_ms, pending.recheck);
            return;
        }
        self.silent += 1;
        if self.silent < SILENT_TRIES {
            out!("[DAILY] {command}: no reply from OwO; trying again later.");
            return;
        }
        self.silent = 0;
        out!("[DAILY] {command}: no reply from OwO after {SILENT_TRIES} tries.");
        self.stop_until_reset(unix_ms, false);
    }
}

fn other_echo_near(messages: &[ChatMessage], command: &str, reply_id: u64) -> bool {
    let since = snowflake_unix_ms(reply_id).saturating_sub(ECHO_WINDOW_MS);
    let echoes = messages
        .iter()
        .filter(|m| m.id < reply_id && snowflake_unix_ms(m.id) >= since)
        .filter(|m| split_summary(&m.summary).is_some_and(|(_, content)| content == command))
        .count();
    echoes > 1
}

/// Daily limits are read from OwO's replies, so a limit that grows from one day to the next is followed.
/// What is known is saved to `store`, so a restart on the same OwO day neither forgets nor re-sends it.
pub struct Dailies {
    trackers: Vec<Tracker>,
    store: Option<PathBuf>,
    /// Saved entries, including those of commands turned off this run, so turning one off keeps its count.
    saved: serde_json::Map<String, Value>,
    warned: bool,
}

impl Dailies {
    pub fn new(enabled: impl IntoIterator<Item = Daily>, store: Option<PathBuf>, unix_ms: u64) -> Self {
        let file: Value = store
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let saved = file["commands"].as_object().cloned().unwrap_or_default();
        let trackers = enabled
            .into_iter()
            .map(|daily| {
                let mut tracker = Tracker::new(daily);
                if let Some(entry) = saved.get(daily.command()) {
                    tracker.restore(entry, unix_ms);
                }
                tracker
            })
            .collect();
        Self { trackers, store, saved, warned: false }
    }

    /// Writes the file when anything in it changed, through a temporary file so a crash never leaves half of one.
    pub fn save(&mut self) {
        let Some(path) = &self.store else { return };
        let mut commands = self.saved.clone();
        for tracker in &self.trackers {
            commands.insert(tracker.daily.command().to_owned(), tracker.snapshot());
        }
        if commands == self.saved {
            return;
        }
        let temporary = path.with_extension("json.tmp");
        let body = json!({ "commands": commands }).to_string();
        let written = fs::write(&temporary, body).and_then(|()| fs::rename(&temporary, path));
        match written {
            Ok(()) => self.saved = commands,
            Err(e) => {
                let _ = fs::remove_file(&temporary);
                if !self.warned {
                    self.warned = true;
                    out!("[!] Could not save {}: {e}", path.display());
                }
            }
        }
    }

    pub fn next_due(&self, now: Instant, unix_ms: u64) -> Option<Daily> {
        self.trackers.iter().find(|t| t.due(now, unix_ms)).map(|t| t.daily)
    }

    /// Why `daily` cannot be sent now; `Err(None)` means it is off in the settings.
    pub fn blocked(&self, daily: Daily, now: Instant, unix_ms: u64) -> Result<(), Option<&'static str>> {
        let tracker = self.trackers.iter().find(|t| t.daily == daily).ok_or(None)?;
        tracker.blocked(now, unix_ms).map_or(Ok(()), |why| Err(Some(why)))
    }

    pub fn sent(&mut self, daily: Daily, after_id: u64, now: Instant, unix_ms: u64) {
        if let Some(tracker) = self.trackers.iter_mut().find(|t| t.daily == daily) {
            tracker.sent(after_id, now, unix_ms);
        }
    }

    pub fn observe(&mut self, messages: &[ChatMessage], now: Instant, unix_ms: u64) {
        for tracker in &mut self.trackers {
            tracker.observe(messages, now, unix_ms);
        }
    }

    pub fn view(&self) -> Vec<crate::hub::DailyView> {
        self.trackers
            .iter()
            .map(|t| crate::hub::DailyView {
                command: t.daily.command(),
                left: if let Status::Left { left, .. } = t.status { Some(left) } else { None },
                limit: t.limit,
                used_up: matches!(t.status, Status::UsedUp { .. }),
                retry_at: if let Status::UsedUp { retry_at } = t.status { Some(retry_at) } else { None },
            })
            .collect()
    }

    /// After a pause the reply may never come, which says nothing about the limit.
    pub fn forget_pending(&mut self) {
        for tracker in &mut self.trackers {
            tracker.pending = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake_at;

    const RUN: &str = "OwO Verified App , :run: | You stopped your run a bit early yesterday. :blank: | You ran 1/1 kilometers today! :blank: | Total distance traveled: 115km , 12:00 PM";
    const RUN_TIRED: &str = "OwO Verified App , 🚫 | You are too tired to run! :blank: | You ran 1 kilometer(s) today! :blank: | You can run 2 kilometers tomorrow! :blank: | You ran 115 in total! , 12:01 PM";
    const PIKU: &str = "OwO Verified App , 🥕 | You picked one PikPik carrot! :blank: | Yesterday you let 2 carrots spoil... :blank: | You can still harvest 2 today. :blank: | You have 96 carrots in total! , 12:00 PM";
    const PIKU_AGAIN: &str = "OwO Verified App , 🥕 | You picked one PikPik carrot! :blank: | You harvested 2/2 today! :blank: | You have 97 carrots in total! , 12:01 PM";
    const ARMY: &str = "OwO Verified App , :rusty: | Scavenging through the messages, you found the rusty ol' Rusty Army Emblem! :blank: | Today's remaining Broken Army Emblem : 14 , 12:00 PM";
    const PUP: &str = "OwO Verified App , :puppy: | You picked up one puppy! :blank: | You can adopt 3 puppies today. :blank: | You adopted 128 puppies in total! , 12:00 PM";
    const PUP_AGAIN: &str = "OwO Verified App , :puppy: | You picked up one puppy!! :blank: | You adopted 2/3 puppies today! :blank: | You adopted 129 puppies in total! , 12:01 PM";
    const CLAIM: &str = "OwO Verified App , 💰 | Player, Here is your daily :cowoncy: 5000 Cowoncy! :blank: | You're on a 1013 daily streak! 📦 | You received a lootbox! ⏱ | Your next daily is in: 22H 52M 34S 📪 | Player, you have 3 unread mail! , 3:07 PM";
    const CLAIM_WAIT: &str = "OwO Verified App , ⏱ | Nu! Player! You need to wait 2H 44M 11S , 12:00 PM";
    const HUNT: &str = "OwO Verified App , 🌱 | Player spent 5 :cowoncy: and caught a :bug: ! , 12:00 PM";
    /// 2026-01-01 12:00 UTC, in Pacific standard time.
    const NOON: u64 = 1_767_268_800_000;
    const JULY_NOON: u64 = 1_782_907_200_000;

    fn msg_at(unix_ms: u64, summary: &str) -> ChatMessage {
        ChatMessage {
            id: snowflake_at(unix_ms),
            summary: summary.to_owned(),
            detail: String::new(),
            from_owo: crate::gems::reply_text(summary).is_some(),
        }
    }

    fn days(unix_ms: u64) -> i64 {
        (unix_ms / DAY_MS) as i64
    }

    struct Run {
        dailies: Dailies,
        start: Instant,
        unix: u64,
    }

    impl Run {
        fn new(daily: Daily, unix: u64) -> Self {
            Self { dailies: Dailies::new([daily], None, unix), start: Instant::now(), unix }
        }
        fn at(&self, later_ms: u64) -> (Instant, u64) {
            (self.start + Duration::from_millis(later_ms), self.unix + later_ms)
        }
        fn due(&self, later_ms: u64) -> bool {
            let (now, unix) = self.at(later_ms);
            self.dailies.next_due(now, unix).is_some()
        }
        fn send(&mut self, daily: Daily, later_ms: u64) {
            let (now, unix) = self.at(later_ms);
            self.dailies.sent(daily, snowflake_at(unix), now, unix);
        }
        fn reply(&mut self, later_ms: u64, messages: &[ChatMessage]) {
            let (now, unix) = self.at(later_ms);
            self.dailies.observe(messages, now, unix);
        }
        fn status(&self) -> Status {
            self.dailies.trackers[0].status
        }
        fn left(&self) -> Option<u32> {
            self.dailies.view()[0].left
        }
    }

    #[test]
    fn remaining_uses_are_read_from_each_reply() {
        let left = |daily: Daily, summary| daily.remaining(crate::gems::reply_text(summary).unwrap());
        assert_eq!(left(Daily::Run, RUN), Some((0, Some(1))));
        assert_eq!(left(Daily::Run, RUN_TIRED), Some((0, None)));
        assert_eq!(left(Daily::Pup, PUP), Some((2, Some(3))));
        assert_eq!(left(Daily::Pup, PUP_AGAIN), Some((1, Some(3))));
        assert_eq!(left(Daily::Piku, PIKU), Some((1, Some(2))));
        assert_eq!(left(Daily::Piku, PIKU_AGAIN), Some((0, Some(2))));
        assert_eq!(left(Daily::Army, ARMY), Some((14, None)));
        assert_eq!(left(Daily::Claim, CLAIM), Some((0, Some(1))));
        assert_eq!(left(Daily::Claim, CLAIM_WAIT), Some((0, Some(1))));
        assert_eq!(Daily::Pup.remaining("You adopted 3 puppies today! | You can adopt 4 puppies tomorrow!"), None);
    }

    #[test]
    fn owodaily_is_sent_once_then_waits_for_the_reset() {
        let mut run = Run::new(Daily::Claim, NOON);
        assert!(run.due(0));
        run.send(Daily::Claim, 0);
        run.reply(1000, &[msg_at(NOON + 500, CLAIM_WAIT)]);
        assert!(matches!(run.status(), Status::UsedUp { .. }));
        assert!(!run.due(HOUR_MS));
        assert!(run.due(next_reset(NOON) - NOON));
    }

    #[test]
    fn counts_survive_a_restart_on_the_same_owo_day_only() {
        let path = std::env::temp_dir().join(format!("zippy-dailies-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let start = Instant::now();
        let mut dailies = Dailies::new([Daily::Army, Daily::Pup], Some(path.clone()), NOON);
        dailies.sent(Daily::Army, snowflake_at(NOON), start, NOON);
        dailies.observe(&[msg_at(NOON + 500, ARMY)], start, NOON + 1000);
        dailies.sent(Daily::Pup, snowflake_at(NOON + 2000), start, NOON + 2000);
        dailies.observe(&[msg_at(NOON + 2500, &PUP_AGAIN.replace("2/3", "3/3"))], start, NOON + 3000);
        dailies.save();

        let view = |d: &Dailies| d.view().into_iter().map(|v| (v.left, v.limit, v.used_up)).collect::<Vec<_>>();
        let later = Dailies::new([Daily::Army, Daily::Pup], Some(path.clone()), NOON + HOUR_MS);
        assert_eq!(view(&later), [(Some(14), Some(15), false), (None, Some(3), true)]);

        let mut only_army = Dailies::new([Daily::Army], Some(path.clone()), NOON + HOUR_MS);
        only_army.sent(Daily::Army, snowflake_at(NOON + HOUR_MS), start, NOON + HOUR_MS);
        only_army.observe(&[msg_at(NOON + HOUR_MS + 500, &ARMY.replace(": 14", ": 13"))], start, NOON + HOUR_MS + 1000);
        only_army.save();
        let tomorrow = next_reset(NOON) + HOUR_MS;
        let next_day = Dailies::new([Daily::Army, Daily::Pup], Some(path.clone()), tomorrow);
        assert_eq!(view(&next_day), [(None, Some(15), false), (None, Some(3), false)], "pup, off for a run, keeps its limit");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn reset_follows_midnight_us_pacific_with_daylight_time() {
        assert_eq!(next_reset(NOON) - RESET_MARGIN_MS, (days(NOON) as u64 + 1) * DAY_MS + 8 * HOUR_MS);
        assert_eq!(next_reset(JULY_NOON) - RESET_MARGIN_MS, (days(JULY_NOON) as u64 + 1) * DAY_MS + 7 * HOUR_MS);
        let march_8 = days_from_civil(2026, 3, 8);
        let november_1 = days_from_civil(2026, 11, 1);
        assert_eq!(pacific_midnight_ms(march_8) % DAY_MS, 8 * HOUR_MS);
        assert_eq!(pacific_midnight_ms(march_8 + 1) % DAY_MS, 7 * HOUR_MS);
        assert_eq!(pacific_midnight_ms(november_1) % DAY_MS, 7 * HOUR_MS);
        assert_eq!(pacific_midnight_ms(november_1 + 1) % DAY_MS, 8 * HOUR_MS);
        assert_eq!(year_of(days_from_civil(2028, 2, 29)), 2028);
        assert_eq!(year_of(days_from_civil(2026, 12, 31)), 2026);
    }

    #[test]
    fn command_repeats_until_its_reply_says_none_left() {
        let mut run = Run::new(Daily::Pup, NOON);
        assert!(run.due(0));
        run.send(Daily::Pup, 0);
        assert!(!run.due(1), "waits for the reply");
        run.reply(1000, &[msg_at(NOON - 1000, PUP), msg_at(NOON + 500, HUNT)]);
        assert_eq!(run.status(), Status::Unknown, "older and unrelated replies are ignored");
        run.reply(2000, &[msg_at(NOON + 1500, PUP)]);
        assert_eq!(run.left(), Some(2));
        assert!(!run.due(61_000), "owopup has a one-minute cooldown");
        assert!(run.due(62_000));

        run.send(Daily::Pup, 70_000);
        run.reply(71_000, &[msg_at(NOON + 70_500, PUP_AGAIN)]);
        assert_eq!(run.left(), Some(1));
        run.send(Daily::Pup, 140_000);
        run.reply(141_000, &[msg_at(NOON + 140_500, &PUP_AGAIN.replace("2/3", "3/3"))]);
        assert!(!run.due(HOUR_MS));
        assert!(run.due(next_reset(NOON) - NOON));
    }

    #[test]
    fn a_count_that_does_not_drop_stops_the_command() {
        let mut run = Run::new(Daily::Army, NOON);
        run.send(Daily::Army, 0);
        run.reply(1000, &[msg_at(NOON + 500, ARMY)]);
        assert!(!run.due(11_000) && run.due(12_000), "owoarmy has a ten-second cooldown");
        run.send(Daily::Army, 30_000);
        run.reply(31_000, &[msg_at(NOON + 30_500, ARMY)]);
        assert!(matches!(run.status(), Status::UsedUp { .. }));
        assert!(!run.due(2 * HOUR_MS));
    }

    #[test]
    fn still_used_up_after_the_reset_is_rechecked_hourly() {
        let mut run = Run::new(Daily::Run, NOON);
        run.send(Daily::Run, 0);
        run.reply(1000, &[msg_at(NOON + 500, RUN)]);
        let reset = next_reset(NOON) - NOON;
        assert!(!run.due(reset - 1));
        assert!(run.due(reset));
        run.send(Daily::Run, reset);
        run.reply(reset + 1000, &[msg_at(NOON + reset + 500, RUN_TIRED)]);
        assert!(!run.due(reset + HOUR_MS));
        assert!(run.due(reset + 1000 + HOUR_MS));
    }

    #[test]
    fn unreadable_reply_waits_for_a_readable_one_then_gives_up() {
        let stray = "OwO Verified App , 🥕 | Your carrots are not ready yet! , 12:00 PM";
        let mut run = Run::new(Daily::Piku, NOON);
        run.send(Daily::Piku, 0);
        run.reply(1000, &[msg_at(NOON + 500, stray)]);
        assert_eq!(run.status(), Status::Unknown, "keeps waiting before the timeout");
        run.reply(2000, &[msg_at(NOON + 500, stray), msg_at(NOON + 1500, PIKU)]);
        assert_eq!(run.left(), Some(1));

        run.send(Daily::Piku, 30_000);
        run.reply(30_000 + 20_000, &[msg_at(NOON + 30_500, stray)]);
        assert!(matches!(run.status(), Status::UsedUp { .. }));
    }

    #[test]
    fn unanswered_command_stops_after_two_tries() {
        let mut run = Run::new(Daily::Army, NOON);
        run.send(Daily::Army, 0);
        run.reply(20_000, &[]);
        assert!(run.due(40_000));
        run.send(Daily::Army, 40_000);
        run.reply(60_000, &[]);
        assert!(!run.due(2 * HOUR_MS));
        assert!(run.due(next_reset(NOON) - NOON));
    }

    #[test]
    fn reply_with_another_users_echo_nearby_is_not_trusted() {
        let mut run = Run::new(Daily::Pup, NOON);
        run.send(Daily::Pup, 0);
        let messages = [
            msg_at(NOON + 200, "Player , owopup , 12:00 PM"),
            msg_at(NOON + 300, "Other , owopup , 12:00 PM"),
            msg_at(NOON + 800, &PUP_AGAIN.replace("2/3", "3/3")),
        ];
        run.reply(1000, &messages);
        assert_eq!(run.status(), Status::Unknown);
        assert!(run.due(62_000));
    }

    #[test]
    fn a_new_days_count_is_not_mistaken_for_a_stuck_one() {
        let late = next_reset(NOON) - RESET_MARGIN_MS - 2 * MINUTE_MS;
        let mut run = Run::new(Daily::Pup, late);
        run.send(Daily::Pup, 0);
        run.reply(1000, &[msg_at(late + 500, PUP_AGAIN)]);
        assert_eq!(run.left(), Some(1));
        let after_midnight = 3 * MINUTE_MS;
        run.send(Daily::Pup, after_midnight);
        run.reply(after_midnight + 1000, &[msg_at(late + after_midnight + 500, PUP)]);
        assert_eq!(run.left(), Some(2), "a fresh day's 2 left is not \"no change\" from yesterday's 1");
    }

    #[test]
    fn sends_are_capped_per_day() {
        let mut run = Run::new(Daily::Army, NOON);
        for i in 0..MAX_SENDS_PER_DAY as u64 {
            let at = i * 30_000;
            assert!(run.due(at));
            run.send(Daily::Army, at);
            let reply = ARMY.replace(": 14", &format!(": {}", 1000 - i));
            run.reply(at + 1000, &[msg_at(NOON + at + 500, &reply)]);
        }
        assert!(!run.due(MAX_SENDS_PER_DAY as u64 * 30_000));
        assert!(run.due(next_reset(NOON) - NOON));
    }

    #[test]
    fn a_pause_forgets_the_outstanding_send_without_counting_it() {
        let mut run = Run::new(Daily::Run, NOON);
        run.send(Daily::Run, 0);
        run.dailies.forget_pending();
        assert!(run.due(20_000));
        assert_eq!(run.dailies.trackers[0].silent, 0);
    }
}
