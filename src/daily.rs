use std::time::{Duration, Instant};

use crate::gems::{ChatMessage, owo_content, split_summary};
use crate::snowflake_unix_ms;

const SPACING: Duration = Duration::from_secs(20);
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
    Run,
    Pup,
    Piku,
    Army,
}

impl Daily {
    pub const ALL: [Daily; 4] = [Daily::Run, Daily::Pup, Daily::Piku, Daily::Army];

    pub fn key(self) -> &'static str {
        match self {
            Daily::Run => "run",
            Daily::Pup => "pup",
            Daily::Piku => "piku",
            Daily::Army => "army",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            Daily::Run => "oworun",
            Daily::Pup => "owopup",
            Daily::Piku => "owopiku",
            Daily::Army => "owoarmy",
        }
    }

    fn markers(self) -> &'static [&'static str] {
        match self {
            Daily::Run => &[":run:", "kilometer"],
            Daily::Pup => &[":puppy:", "pupp"],
            Daily::Piku => &["🥕", "carrot", "pikpik"],
            Daily::Army => &["army emblem", ":rusty:"],
        }
    }

    /// The first use of a day states the daily limit ("You can adopt 3 puppies today"); later uses say "2/3".
    fn remaining(self, content: &str) -> Option<u32> {
        if self == Daily::Run && content.contains("too tired") {
            return Some(0);
        }
        today_lines(content).find_map(|line| {
            used_of_limit(line).or_else(|| match self {
                Daily::Run => None,
                Daily::Pup => number_after(line, "You can adopt ").map(|limit| limit.saturating_sub(1)),
                Daily::Piku => number_after(line, "You can still harvest ").map(|limit| limit.saturating_sub(1)),
                Daily::Army => number_after(line.split_once("Today's remaining")?.1, ":"),
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

fn used_of_limit(line: &str) -> Option<u32> {
    line.split_whitespace().find_map(|word| {
        let (used, limit) = word.split_once('/')?;
        Some(limit.parse::<u32>().ok()?.saturating_sub(used.parse().ok()?))
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
    Left(u32),
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
    pending: Option<Pending>,
    next: Option<Instant>,
    silent: u32,
    sends_today: (i64, u32),
}

impl Tracker {
    fn due(&self, now: Instant, unix_ms: u64) -> bool {
        let (day, sends) = self.sends_today;
        self.pending.is_none()
            && self.next.is_none_or(|next| now >= next)
            && !(day == owo_day(unix_ms) && sends >= MAX_SENDS_PER_DAY)
            && match self.status {
                Status::Unknown | Status::Left(_) => true,
                Status::UsedUp { retry_at } => unix_ms >= retry_at,
            }
    }

    fn sent(&mut self, after_id: u64, now: Instant, unix_ms: u64) {
        let recheck = matches!(self.status, Status::UsedUp { .. });
        self.pending = Some(Pending { after_id, sent: now, recheck });
        let day = owo_day(unix_ms);
        let sends = if self.sends_today.0 == day { self.sends_today.1 + 1 } else { 1 };
        self.sends_today = (day, sends);
        if sends == MAX_SENDS_PER_DAY {
            println!("[DAILY] {}: sent {sends} times today; no more until the reset.", self.daily.command());
        }
    }

    fn stop_until_reset(&mut self, unix_ms: u64, recheck: bool) {
        let retry_at = retry_time(unix_ms, recheck);
        self.status = Status::UsedUp { retry_at };
        let now_ms = crate::unix_ms_now().max(unix_ms);
        println!(
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
            .filter_map(|m| Some((m.id, owo_content(&m.summary)?)))
            .filter(|(_, content)| {
                let content = content.to_lowercase();
                self.daily.markers().iter().any(|marker| content.contains(marker))
            })
            .collect();
        replies.sort_unstable_by_key(|(id, _)| *id);
        let mut unreadable = None;
        for (id, content) in replies {
            let Some(left) = self.daily.remaining(content) else {
                unreadable.get_or_insert(content);
                continue;
            };
            self.pending = None;
            self.silent = 0;
            self.next = Some(now + SPACING);
            if other_echo_near(messages, command, id) {
                println!("[DAILY] {command}: someone else sent it at the same time; checking again later.");
                return;
            }
            // A pause can hold the reply back past the reset; the day it belongs to is the day it was posted.
            let posted = snowflake_unix_ms(id).clamp(unix_ms.saturating_sub(12 * HOUR_MS), unix_ms);
            let stuck = matches!(self.status, Status::Left(before) if left >= before);
            if left > 0 && !stuck {
                self.status = Status::Left(left);
                println!("[DAILY] {command}: {left} left today.");
                return;
            }
            if stuck {
                println!("[DAILY] {command}: OwO still says {left} left after another use, so it stops to be safe.");
            }
            self.stop_until_reset(posted, pending.recheck);
            return;
        }
        if now < pending.sent + REPLY_TIMEOUT {
            return;
        }
        self.pending = None;
        self.next = Some(now + SPACING);
        if let Some(content) = unreadable {
            let excerpt: String = content.chars().take(160).collect();
            println!("[DAILY] {command}: could not read OwO's reply: \"{excerpt}\"");
            self.stop_until_reset(unix_ms, pending.recheck);
            return;
        }
        self.silent += 1;
        if self.silent < SILENT_TRIES {
            println!("[DAILY] {command}: no reply from OwO; trying again later.");
            return;
        }
        self.silent = 0;
        println!("[DAILY] {command}: no reply from OwO after {SILENT_TRIES} tries.");
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
pub struct Dailies {
    trackers: Vec<Tracker>,
}

impl Dailies {
    pub fn new(enabled: impl IntoIterator<Item = Daily>) -> Self {
        let trackers = enabled
            .into_iter()
            .map(|daily| Tracker {
                daily,
                status: Status::Unknown,
                pending: None,
                next: None,
                silent: 0,
                sends_today: (0, 0),
            })
            .collect();
        Self { trackers }
    }

    pub fn next_due(&self, now: Instant, unix_ms: u64) -> Option<Daily> {
        self.trackers.iter().find(|t| t.due(now, unix_ms)).map(|t| t.daily)
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
    const HUNT: &str = "OwO Verified App , 🌱 | Player spent 5 :cowoncy: and caught a :bug: ! , 12:00 PM";
    /// 2026-01-01 12:00 UTC, in Pacific standard time.
    const NOON: u64 = 1_767_268_800_000;
    const JULY_NOON: u64 = 1_782_907_200_000;

    fn msg_at(unix_ms: u64, summary: &str) -> ChatMessage {
        ChatMessage { id: snowflake_at(unix_ms), summary: summary.to_owned() }
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
            Self { dailies: Dailies::new([daily]), start: Instant::now(), unix }
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
    }

    #[test]
    fn remaining_uses_are_read_from_each_reply() {
        let left = |daily: Daily, summary| daily.remaining(owo_content(summary).unwrap());
        assert_eq!(left(Daily::Run, RUN), Some(0));
        assert_eq!(left(Daily::Run, RUN_TIRED), Some(0));
        assert_eq!(left(Daily::Pup, PUP), Some(2));
        assert_eq!(left(Daily::Pup, PUP_AGAIN), Some(1));
        assert_eq!(left(Daily::Piku, PIKU), Some(1));
        assert_eq!(left(Daily::Piku, PIKU_AGAIN), Some(0));
        assert_eq!(left(Daily::Army, ARMY), Some(14));
        assert_eq!(Daily::Pup.remaining("You adopted 3 puppies today! | You can adopt 4 puppies tomorrow!"), None);
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
        assert_eq!(run.status(), Status::Left(2));
        assert!(!run.due(3000), "spaced out");
        assert!(run.due(2000 + 20_000));

        run.send(Daily::Pup, 30_000);
        run.reply(31_000, &[msg_at(NOON + 30_500, PUP_AGAIN)]);
        assert_eq!(run.status(), Status::Left(1));
        run.send(Daily::Pup, 60_000);
        run.reply(61_000, &[msg_at(NOON + 60_500, &PUP_AGAIN.replace("2/3", "3/3"))]);
        assert!(!run.due(HOUR_MS));
        assert!(run.due(next_reset(NOON) - NOON));
    }

    #[test]
    fn a_count_that_does_not_drop_stops_the_command() {
        let mut run = Run::new(Daily::Army, NOON);
        run.send(Daily::Army, 0);
        run.reply(1000, &[msg_at(NOON + 500, ARMY)]);
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
        assert_eq!(run.status(), Status::Left(1));

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
        assert!(run.due(1000 + 20_000));
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
