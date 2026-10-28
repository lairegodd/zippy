use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::gems::{ChatMessage, owo_content};
use crate::snowflake_unix_ms;

pub const FILE_NAME: &str = "zippy-stats.jsonl";
const ROTATE_AT: u64 = 1 << 20;
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);
/// OwO never asks to wait longer than a day or two; anything beyond this is not a real reply.
const MAX_WAIT_MS: u64 = 7 * 86_400_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stat {
    Cash,
    Cookies,
    Luck,
    Essence,
    Streak,
    BossTickets,
    BossTicketsMax,
    HuntbotReadyAt,
    /// Huntbot's full duration in tenths of an hour (76 = 7.6h), from its Duration trait.
    HuntbotHours,
    /// Cowoncy a hunt of the full duration costs, from "Current Max Autohunt ... for N cowoncy".
    HuntbotCost,
    DailyReadyAt,
    CookieReadyAt,
    VoteReadyAt,
    DailyStreak,
    UnreadMail,
}

impl Stat {
    pub const ALL: [Stat; 15] = [
        Stat::Cash,
        Stat::Cookies,
        Stat::Luck,
        Stat::Essence,
        Stat::Streak,
        Stat::BossTickets,
        Stat::BossTicketsMax,
        Stat::HuntbotReadyAt,
        Stat::HuntbotHours,
        Stat::HuntbotCost,
        Stat::DailyReadyAt,
        Stat::CookieReadyAt,
        Stat::VoteReadyAt,
        Stat::DailyStreak,
        Stat::UnreadMail,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Stat::Cash => "cash",
            Stat::Cookies => "cookies",
            Stat::Luck => "luck",
            Stat::Essence => "essence",
            Stat::Streak => "streak",
            Stat::BossTickets => "boss_tickets",
            Stat::BossTicketsMax => "boss_tickets_max",
            Stat::HuntbotReadyAt => "huntbot_ready_at",
            Stat::HuntbotHours => "huntbot_hours",
            Stat::HuntbotCost => "huntbot_cost",
            Stat::DailyReadyAt => "daily_ready_at",
            Stat::CookieReadyAt => "cookie_ready_at",
            Stat::VoteReadyAt => "vote_ready_at",
            Stat::DailyStreak => "daily_streak",
            Stat::UnreadMail => "unread_mail",
        }
    }

    fn from_key(key: &str) -> Option<Stat> {
        Stat::ALL.into_iter().find(|s| s.key() == key)
    }

    pub fn reminder(self) -> Option<&'static str> {
        match self {
            Stat::HuntbotReadyAt => Some("Huntbot is back"),
            Stat::DailyReadyAt => Some("owo daily is ready"),
            Stat::CookieReadyAt => Some("You can send a cookie"),
            Stat::VoteReadyAt => Some("You can vote for OwO"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub value: u64,
    /// When OwO posted the reply this came from, in Unix ms.
    pub at: u64,
}

#[derive(Default)]
pub struct Stats {
    readings: [Option<Reading>; Stat::ALL.len()],
    /// Whether each reminder was last seen still counting down, so only the change to ready is announced.
    waiting: [bool; Stat::ALL.len()],
    pub prays_sent: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
}

impl Stats {
    pub fn get(&self, stat: Stat) -> Option<Reading> {
        self.readings[stat as usize]
    }

    /// Older replies never overwrite newer ones; returns whether the value changed.
    fn apply(&mut self, stat: Stat, reading: Reading) -> bool {
        let slot = &mut self.readings[stat as usize];
        if slot.is_some_and(|old| old.at > reading.at || old == reading) {
            return false;
        }
        *slot = Some(reading);
        true
    }

    /// Reminders whose countdown ran out since the last call; a reply that says "ready" outright is shown, not announced.
    pub fn due_reminders(&mut self, now_ms: u64) -> Vec<&'static str> {
        let mut due = Vec::new();
        for stat in Stat::ALL {
            let (Some(label), Some(reading)) = (stat.reminder(), self.get(stat)) else { continue };
            let waiting = &mut self.waiting[stat as usize];
            if reading.value > now_ms {
                *waiting = true;
            } else if *waiting {
                *waiting = false;
                due.push(label);
            }
        }
        due
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Won,
    Lost,
    Draw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Set(Stat, u64),
    Battle(Outcome),
}

fn number(text: &str) -> Option<u64> {
    let digits: String = text
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn number_after(text: &str, marker: &str) -> Option<u64> {
    number(text.split_once(marker)?.1)
}

/// Reads "2H 44M 31S", "3 hours" or "5 minutes" from the start of `text`.
pub fn duration_ms(text: &str) -> Option<u64> {
    let mut words = text.split_whitespace().peekable();
    let mut total: Option<u64> = None;
    while let Some(word) = words.next() {
        let word = word.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        let split = word.find(|c: char| !c.is_ascii_digit()).unwrap_or(word.len());
        let Ok(amount) = word[..split].parse::<u64>() else { break };
        let mut unit = word[split..].to_ascii_lowercase();
        if unit.is_empty() {
            let Some(next) = words.next() else { break };
            unit = next.trim_end_matches(|c: char| !c.is_ascii_alphabetic()).to_ascii_lowercase();
        }
        let scale = match unit.as_str() {
            "d" | "day" | "days" => 86_400_000,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000,
            "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
            "s" | "sec" | "secs" | "second" | "seconds" => 1000,
            _ => break,
        };
        total = Some(total.unwrap_or(0).saturating_add(amount.saturating_mul(scale)));
    }
    total.filter(|&ms| ms <= MAX_WAIT_MS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Query {
    Vote,
    Huntbot,
}

fn query_of(command: &str) -> Option<Query> {
    match command {
        "owovote" => Some(Query::Vote),
        "owohb" => Some(Query::Huntbot),
        _ => None,
    }
}

/// Replies that name the player, so they can be read whoever sent the command.
fn named_events(content: &str, detail: &str, player: &str, posted: u64) -> Vec<Event> {
    let mut events = Vec::new();
    let mut set = |stat, value| events.push(Event::Set(stat, value));
    if content.starts_with(&format!("{player} goes into battle!")) {
        if let Some(streak) = number_after(content, "Streak: ") {
            set(Stat::Streak, streak);
        }
        // "You won in 20 turns!", "You lost in 9 turns!" or "It's a tie in 24 turns!".
        let outcome = content.rsplit("Image").next().unwrap_or(content);
        let result = if outcome.contains("You won") {
            Some(Outcome::Won)
        } else if outcome.contains("You lost") {
            Some(Outcome::Lost)
        } else if outcome.contains("It's a tie") || outcome.contains("It’s a tie") {
            Some(Outcome::Draw)
        } else {
            None
        };
        events.extend(result.map(Event::Battle));
        return events;
    }
    if let Some(cash) = cash_of(content, player) {
        set(Stat::Cash, cash);
    }
    if let Some(cookies) = number_after(content, &format!("{player}! You currently have "))
        .filter(|_| content.contains("cookie"))
    {
        set(Stat::Cookies, cookies);
        if let Some(wait) = content.split_once("You can send a cookie in ").and_then(|(_, rest)| duration_ms(rest)) {
            set(Stat::CookieReadyAt, posted + wait);
        }
    }
    if let Some((_, rest)) = content.split_once(&format!("{player}, you currently have "))
        && let Some((left, rest)) = rest.split_once('/')
        && rest.contains("boss ticket")
        && let (Some(left), Some(max)) = (number(left), number(rest))
    {
        set(Stat::BossTickets, left);
        set(Stat::BossTicketsMax, max);
    }
    if content.contains(&format!("@{player} prays"))
        && let Some(luck) = number_after(content, "You have ")
    {
        set(Stat::Luck, luck);
    }
    if let Some((_, rest)) = content.split_once(&format!("Nu! {player}! You need to wait "))
        && let Some(wait) = duration_ms(rest)
    {
        set(Stat::DailyReadyAt, posted + wait);
    }
    // "Player, Here is your daily 5000 Cowoncy! | You're on a 12 daily streak! | You received a
    // lootbox! | Your next daily is in: 22H 52M 34S". The reward and the item change daily and are not read.
    if content.contains(&format!("{player}, Here is your daily")) {
        if let Some(wait) = wait_after(&content.to_ascii_uppercase(), "NEXT DAILY IS IN") {
            set(Stat::DailyReadyAt, posted + wait);
        }
        if let Some((before, _)) = content.split_once(" daily streak")
            && let Some(streak) = before.rsplit(' ').next().and_then(number)
        {
            set(Stat::DailyStreak, streak);
        }
        // The mail line comes with every daily while mail is unread, so a daily without it means none.
        // When it is a message of its own, it is posted later and replaces this reading.
        if !content.contains("unread mail") {
            set(Stat::UnreadMail, 0);
        }
    }
    // Shown with owodaily while mail is unread: "Player, you have 3 unread mail!"
    if let Some((_, rest)) = content.split_once(&format!("{player}, you have "))
        && rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == ',').starts_with(" unread mail")
        && let Some(mail) = number(rest)
    {
        set(Stat::UnreadMail, mail);
    }
    if content.contains(&format!("{player}'s HuntBot")) {
        let field = detail.split_once("Animal Essence -").map(|(_, rest)| rest.trim_start_matches([' ', '|']));
        if let Some(essence) = field.and_then(number) {
            set(Stat::Essence, essence);
        }
        let text = format!("{content} | {detail}");
        if let Some(at) = huntbot_ready_at(&text, posted) {
            set(Stat::HuntbotReadyAt, at);
        }
        if let Some(tenths) = huntbot_hours(&text) {
            set(Stat::HuntbotHours, tenths);
        }
        if let Some(cost) = huntbot_cost(&text) {
            set(Stat::HuntbotCost, cost);
        }
    }
    // "BEEP BOOP. Player, YOU SPENT 1,216 cowoncy I WILL BE BACK IN 1H 36M WITH 152 ANIMALS, ..."
    let upper = content.to_uppercase();
    if upper.contains("YOU SPENT")
        && upper.contains(&player.to_uppercase())
        && let Some(wait) = wait_after(&upper, "BACK IN ")
    {
        set(Stat::HuntbotReadyAt, posted + wait);
    }
    events
}

/// "Player, you currently have 2,953 cowoncy!"
pub fn cash_of(content: &str, player: &str) -> Option<u64> {
    number_after(content, &format!("| {player}, you currently have ")).filter(|_| content.contains("cowoncy"))
}

/// Huntbot's full duration in tenths of an hour, from its profile: "Duration - 7.6H", else its level ("Lvl 71"),
/// which OwO turns into 0.5h plus 0.1h a level (level 0 is 0.5h, level 235 is 24h).
pub fn huntbot_hours(text: &str) -> Option<u64> {
    let field = text.split_once("Duration -")?.1;
    let start = field.find(|c: char| c.is_ascii_digit()).filter(|&at| at <= 12)?;
    let rest = &field[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
    let tenths = if rest[end..].starts_with(['H', 'h']) {
        let (whole, fraction) = rest[..end].split_once('.').unwrap_or((&rest[..end], "0"));
        let fraction = fraction.get(..1).unwrap_or("0");
        whole.parse::<u64>().ok()?.checked_mul(10)?.checked_add(fraction.parse().ok()?)?
    } else {
        let level: u64 = number(field.split_once("Lvl")?.1)?;
        5 + level
    };
    (5..=240).contains(&tenths).then_some(tenths)
}

/// What a hunt of the full duration costs: "Current Max Autohunt: 722 animals, ... for 5,776 cowoncy".
pub fn huntbot_cost(text: &str) -> Option<u64> {
    let line = text.split_once("Current Max Autohunt")?.1;
    let (before, _) = line.split_once("cowoncy")?;
    number(before.rsplit_once(" for ")?.1.trim_start_matches([' ', '|']))
}

/// The wait after `marker`, skipping the punctuation Discord leaves where bold text was split off.
fn wait_after(upper: &str, marker: &str) -> Option<u64> {
    let rest = upper.split_once(marker)?.1;
    duration_ms(rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ',' | '*' | ':' | '|')))
}

/// When huntbot is ready, from any of its replies. The idle profile ("to get started") shares a message
/// with the hunting countdown, so the countdown is checked first.
fn huntbot_ready_at(text: &str, posted: u64) -> Option<u64> {
    let upper = text.to_ascii_uppercase();
    if upper.contains("I AM BACK WITH") {
        Some(posted)
    } else if upper.contains("BACK IN ") {
        wait_after(&upper, "BACK IN ").map(|wait| posted + wait)
    } else if upper.contains("TO GET STARTED") {
        Some(posted)
    } else {
        None
    }
}

/// Replies without the player's name, read only right after Zippy sent the command.
fn queried_events(query: Query, content: &str, detail: &str, posted: u64) -> Option<Vec<Event>> {
    let upper = format!("{content} | {detail}").to_ascii_uppercase();
    let set = |stat, at: Option<u64>| Some(at.map(|at| vec![Event::Set(stat, at)]).unwrap_or_default());
    match query {
        Query::Vote if upper.contains("AVAILABLE IN ") || upper.contains("AGAIN IN ") => {
            let wait = wait_after(&upper, "AVAILABLE IN ").or_else(|| wait_after(&upper, "AGAIN IN "));
            set(Stat::VoteReadyAt, wait.map(|w| posted + w))
        }
        Query::Vote if upper.contains("VOTE IS AVAILABLE") => set(Stat::VoteReadyAt, Some(posted)),
        Query::Vote if upper.contains("VOTE") => Some(Vec::new()),
        Query::Huntbot if upper.contains("HUNTBOT") || upper.contains("BEEP BOOP") => {
            set(Stat::HuntbotReadyAt, huntbot_ready_at(&upper, posted))
        }
        _ => None,
    }
}

pub struct Watcher {
    last_id: u64,
    /// Battles at or before this message were fought before the run and are not counted in it.
    battles_after: u64,
    /// Battles already counted. A battle reply can be read before its result is on screen, so battles are
    /// looked at again on later scans and counted when the result shows, once each.
    battles_counted: VecDeque<u64>,
    expected: Vec<(Query, u64, Instant)>,
}

/// More than the chat ever shows at once, so a battle on screen is never counted twice.
const BATTLES_REMEMBERED: usize = 128;

impl Watcher {
    pub fn new() -> Self {
        Self { last_id: 0, battles_after: 0, battles_counted: VecDeque::new(), expected: Vec::new() }
    }

    pub fn counting_battles_after(self, id: u64) -> Self {
        Self { battles_after: id, ..self }
    }

    pub fn sent(&mut self, command: &str, after_id: u64, now: Instant) {
        if let Some(query) = query_of(command) {
            self.expected.retain(|(q, _, _)| *q != query);
            self.expected.push((query, after_id, now));
        }
    }

    /// Each message is read once, except a battle without its result yet, which is read again until the
    /// result shows. Until the player's name is known, only expected replies are read.
    pub fn observe(&mut self, messages: &[ChatMessage], player: Option<&str>, now: Instant) -> Vec<(Event, u64)> {
        self.expected.retain(|(_, _, sent)| now < *sent + REPLY_TIMEOUT);
        let mut ordered: Vec<&ChatMessage> = messages.iter().collect();
        ordered.sort_unstable_by_key(|m| m.id);
        let mut events = Vec::new();
        for message in ordered {
            let Some(content) = owo_content(message) else { continue };
            let posted = snowflake_unix_ms(message.id);
            let matched = self.expected.iter().enumerate().find_map(|(i, (query, after, _))| {
                (message.id > *after).then(|| queried_events(*query, content, &message.detail, posted)).flatten().map(|found| (i, found))
            });
            if let Some((i, found)) = matched {
                self.expected.remove(i);
                events.extend(found.into_iter().map(|e| (e, posted)));
            }
            let Some(player) = player else { continue };
            let fresh = message.id > self.last_id;
            for event in named_events(content, &message.detail, player, posted) {
                match event {
                    Event::Battle(_) => {
                        if message.id <= self.battles_after || self.battles_counted.contains(&message.id) {
                            continue;
                        }
                        if self.battles_counted.len() == BATTLES_REMEMBERED {
                            self.battles_counted.pop_front();
                        }
                        self.battles_counted.push_back(message.id);
                        events.push((event, posted));
                    }
                    Event::Set(..) if fresh => events.push((event, posted)),
                    Event::Set(..) => {}
                }
            }
        }
        if player.is_some() {
            self.last_id = messages.iter().map(|m| m.id).max().unwrap_or(0).max(self.last_id);
        }
        events
    }
}

/// An append-only record of every reading, so the dashboard shows the last known values after a restart.
pub struct Log {
    path: PathBuf,
    warned: bool,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        Self { path, warned: false }
    }

    pub fn load(&self, stats: &mut Stats) {
        let Ok(file) = File::open(&self.path) else { return };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            let stat = record["stat"].as_str().and_then(Stat::from_key);
            if let (Some(stat), Some(value), Some(at)) = (stat, record["value"].as_u64(), record["t"].as_u64()) {
                stats.apply(stat, Reading { value, at });
            }
        }
    }

    pub fn append(&mut self, stat: Stat, reading: Reading) {
        if fs::metadata(&self.path).is_ok_and(|m| m.len() > ROTATE_AT) {
            let _ = fs::rename(&self.path, self.path.with_extension("old.jsonl"));
        }
        let line = serde_json::json!({ "t": reading.at, "stat": stat.key(), "value": reading.value });
        let written = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| writeln!(file, "{line}"));
        if let Err(e) = written
            && !self.warned
        {
            self.warned = true;
            crate::out!("[!] Could not write {}: {e}", self.path.display());
        }
    }
}

impl Stats {
    /// Returns the reading to write to the log, if it changed anything. Writing is left to the caller
    /// so it happens outside the dashboard lock.
    pub fn record(&mut self, event: Event, posted: u64) -> Option<(Stat, Reading)> {
        match event {
            Event::Set(stat, value) => {
                let reading = Reading { value, at: posted };
                self.apply(stat, reading).then_some((stat, reading))
            }
            Event::Battle(outcome) => {
                match outcome {
                    Outcome::Won => self.wins += 1,
                    Outcome::Lost => self.losses += 1,
                    Outcome::Draw => self.draws += 1,
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake_at;

    const CASH: &str = "OwO Verified App , :cowoncy: | Player, you currently have 408,909 cowoncy! , 11:14 AM";
    const COOKIE: &str = "OwO Verified App , :9cookieeat: | Player! You currently have 424 cookies! Yummy! c:< :blank: | You can send a cookie in 2H 44M 31S! , 11:15 AM";
    const BOSS: &str = "OwO Verified App , :bticket: :bticket: :bticket: , Player, you currently have 3/3 boss tickets! Replenishes in 3 hours , , , 11:15 AM";
    const BATTLE: &str = "OwO Verified App , Player goes into battle! 'team' L. 61 :hsnake: - :legendary: :lgreatsword: Image You won in 20 turns! | +200 xp | Streak: 2,592 , 12:43 AM";
    const PRAY: &str = "OwO Verified App , 🙏 | @Player prays... May luck be in your favor. :blank: | You have 3153 luck point(s)! , 11:17 AM";
    const DAILY_WAIT: &str = "OwO Verified App , ⏱ | Nu! Player! You need to wait 2H 44M 11S , 11:15 AM";
    const HUNTBOT_IDLE: &str = "OwO Verified App , Player's HuntBot BEEP. BOOP. I AM HUNTBOT. I WILL HUNT FOR YOU MASTER. Use the command owo autohunt {cowoncy} to get started. , 11:16 AM";
    const HUNTBOT_DETAIL: &str = "Radar - || 0.0004 % || :essence: ||  Animal Essence -  || 120,000 || Current Max Autohunt";
    const HUNTBOT_BACK: &str = "OwO Verified App , :mbot: | BEEP BOOP. I AM BACK WITH 5760 ANIMALS, :blank: | 120000 ESSENCE, AND 168000 EXPERIENCE , 11:15 AM";
    const VOTE_READY: &str = "OwO Verified App , ✅ | Your daily vote is available! :blank: | You can vote every 12 hours! Vote Now! , , , 11:15 AM";
    const VOTE_WAIT: &str = "OwO Verified App , ✅ | Click the link to vote and gain 100+ cowoncy! :blank: | You can vote every 12 hours! :blank: | Your daily vote is available in 7 H , Vote Now! , , , 6:23 PM";
    const HUNTBOT_HUNTING: &str = "OwO Verified App , HUNTBOT is currently hunting! BEEP BOOP. I AM STILL HUNTING. I WILL BE BACK IN 18H 7M 24.51% DONE | 1412 ANIMALS CAPTURED [■■■■■■□□□□] Rank #2,620 • @someone , 6:23 PM";
    const T: u64 = 1_767_268_800_000;

    fn msg(at: u64, summary: &str, detail: &str) -> ChatMessage {
        let from_owo = crate::gems::reply_text(summary).is_some();
        ChatMessage { id: snowflake_at(at), summary: summary.to_owned(), detail: detail.to_owned(), from_owo }
    }

    fn sets(events: &[(Event, u64)]) -> Vec<(Stat, u64)> {
        events.iter().filter_map(|(e, _)| if let Event::Set(s, v) = e { Some((*s, *v)) } else { None }).collect()
    }

    #[test]
    fn durations_read_owo_formats() {
        assert_eq!(duration_ms("2H 44M 31S! more"), Some((2 * 3600 + 44 * 60 + 31) * 1000));
        assert_eq!(duration_ms("3 hours"), Some(3 * 3_600_000));
        assert_eq!(duration_ms("5 minutes and"), Some(300_000));
        assert_eq!(duration_ms("18446744073709551615H"), None, "forged waits are refused, not wrapped");
        assert_eq!(duration_ms("9 days"), None);
        assert_eq!(duration_ms("soon"), None);
        assert_eq!(duration_ms("99999999999999999999H"), None);
    }

    #[test]
    fn named_replies_are_read_for_the_player_only() {
        let mut watcher = Watcher::new();
        let now = Instant::now();
        let messages = [
            msg(T, CASH, ""),
            msg(T + 1000, COOKIE, ""),
            msg(T + 2000, BOSS, ""),
            msg(T + 3000, BATTLE, ""),
            msg(T + 4000, PRAY, ""),
            msg(T + 5000, DAILY_WAIT, ""),
            msg(T + 6000, HUNTBOT_IDLE, HUNTBOT_DETAIL),
        ];
        let events = watcher.observe(&messages, Some("Player"), now);
        assert_eq!(
            sets(&events),
            [
                (Stat::Cash, 408_909),
                (Stat::Cookies, 424),
                (Stat::CookieReadyAt, T + 1000 + (2 * 3600 + 44 * 60 + 31) * 1000),
                (Stat::BossTickets, 3),
                (Stat::BossTicketsMax, 3),
                (Stat::Streak, 2592),
                (Stat::Luck, 3153),
                (Stat::DailyReadyAt, T + 5000 + (2 * 3600 + 44 * 60 + 11) * 1000),
                (Stat::Essence, 120_000),
                (Stat::HuntbotReadyAt, T + 6000),
            ]
        );
        assert!(events.iter().any(|(e, _)| *e == Event::Battle(Outcome::Won)));
        assert!(watcher.observe(&messages, Some("Player"), now).is_empty(), "each message is read once");

        let mut other = Watcher::new();
        assert!(other.observe(&messages, Some("Someone"), now).is_empty());
    }

    #[test]
    fn a_hunting_huntbot_is_not_read_as_ready() {
        let now = Instant::now();
        let detail = format!("{HUNTBOT_DETAIL} || HUNTBOT is currently hunting! || BEEP BOOP. I AM STILL HUNTING. I WILL BE BACK IN 17H 2M || 24.51% DONE");
        let back = T + 1000 + (17 * 60 + 2) * 60_000;
        let mut watcher = Watcher::new();
        watcher.sent("owohb", snowflake_at(T), now);
        let events = watcher.observe(&[msg(T + 1000, HUNTBOT_IDLE, &detail)], Some("Player"), now);
        let huntbot: Vec<u64> = sets(&events).into_iter().filter(|(s, _)| *s == Stat::HuntbotReadyAt).map(|(_, v)| v).collect();
        assert_eq!(huntbot, [back, back]);

        let split = "OwO Verified App , HUNTBOT is currently hunting! BEEP BOOP. I WILL BE BACK IN , 17H 2M , 6:23 PM";
        let mut nameless = Watcher::new();
        nameless.sent("owohb", snowflake_at(T), now);
        assert_eq!(sets(&nameless.observe(&[msg(T + 1000, split, "")], None, now)), [(Stat::HuntbotReadyAt, back)]);
    }

    #[test]
    fn a_started_huntbot_sets_its_return_and_the_profile_its_duration_and_cost() {
        let spent = "OwO Verified App , :rbot: | BEEP BOOP. Ho-oh, YOU SPENT 1,216 cowoncy :blank: | I WILL BE BACK IN 1H 36M WITH 152 ANIMALS, :blank: | 1960 ESSENCE, AND 2632 EXPERIENCE , 6:10 PM";
        let profile = "OwO Verified App , Ho-oh's HuntBot BEEP. BOOP. I AM HUNTBOT. ⏳ Duration - 7.6H Lvl 71 [8271/14370] :cowoncy: Cost - 8 , 6:43 PM";
        let detail = "Animal Essence - || 21,026 || Current Max Autohunt: 722 animals, 9,310 essence, and 12,502 xp for 5,776 cowoncy";
        let events = Watcher::new().observe(&[msg(T, spent, ""), msg(T + 1000, profile, detail)], Some("Ho-oh"), Instant::now());
        let huntbot = sets(&events);
        assert!(huntbot.contains(&(Stat::HuntbotReadyAt, T + 96 * 60_000)));
        assert!(huntbot.contains(&(Stat::HuntbotHours, 76)));
        assert!(huntbot.contains(&(Stat::HuntbotCost, 5776)));
        assert!(Watcher::new().observe(&[msg(T, spent, "")], Some("Someone"), Instant::now()).is_empty());
    }

    #[test]
    fn a_claimed_daily_sets_the_next_one_the_streak_and_unread_mail() {
        let claim = "OwO Verified App , 💰 | Player, Here is your daily :cowoncy: 5000 Cowoncy! :blank: | You're on a 1013 daily streak! 📦 | You received a lootbox! ⏱ | Your next daily is in: 22H 52M 34S 📪 | Player, you have 3 unread mail! , 3:07 PM";
        let events = Watcher::new().observe(&[msg(T, claim, "")], Some("Player"), Instant::now());
        assert_eq!(
            sets(&events),
            [(Stat::DailyReadyAt, T + (22 * 3600 + 52 * 60 + 34) * 1000), (Stat::DailyStreak, 1013), (Stat::UnreadMail, 3)]
        );
        let crate_day = claim.replace("a lootbox", "a weapon crate").replace(" 📪 | Player, you have 3 unread mail!", "");
        let events = Watcher::new().observe(&[msg(T, &crate_day, "")], Some("Player"), Instant::now());
        assert!(sets(&events).contains(&(Stat::UnreadMail, 0)), "a daily without the mail line means none is unread");
    }

    #[test]
    fn a_reply_matches_its_own_query_while_another_still_waits() {
        let now = Instant::now();
        let mut watcher = Watcher::new();
        watcher.sent("owovote", snowflake_at(T), now);
        watcher.sent("owohb", snowflake_at(T + 500), now);
        let events = watcher.observe(&[msg(T + 1000, HUNTBOT_BACK, "")], None, now);
        assert_eq!(sets(&events), [(Stat::HuntbotReadyAt, T + 1000)]);
    }

    #[test]
    fn a_battle_read_before_its_result_is_counted_when_the_result_shows() {
        let now = Instant::now();
        let tie = "OwO Verified App , Player goes into battle! shi L. 35 :hedgebot: - :mythic: :msstaff: 《°w》 L. 31 :gowl: - :epic: :erstaff: Image It's a tie in 24 turns! | +100 xp | Streak: 851 , 2:21 PM";
        let pending = "OwO Verified App , Player goes into battle! shi L. 35 :hedgebot: - :mythic: :msstaff: 《°w》 L. 31 :gowl: - :epic: :erstaff: Image , 2:21 PM";
        let mut watcher = Watcher::new();
        assert!(watcher.observe(&[msg(T, pending, "")], Some("Player"), now).is_empty());
        let events = watcher.observe(&[msg(T, tie, "")], Some("Player"), now);
        assert_eq!(events.iter().filter(|(e, _)| *e == Event::Battle(Outcome::Draw)).count(), 1);
        assert!(watcher.observe(&[msg(T, tie, "")], Some("Player"), now).is_empty(), "counted once");
    }

    #[test]
    fn battles_from_before_the_run_are_not_counted() {
        let old = msg(T, BATTLE, "");
        let new = msg(T + 1000, BATTLE, "");
        let mut watcher = Watcher::new().counting_battles_after(old.id);
        let events = watcher.observe(&[old, new], Some("Player"), Instant::now());
        assert_eq!(events.iter().filter(|(e, _)| matches!(e, Event::Battle(_))).count(), 1);
    }

    #[test]
    fn a_tie_counts_as_a_draw() {
        let tie = "OwO Verified App , Player goes into battle! shi L. 35 :hedgebot: - :mythic: :msstaff: ? ? ? L. 29 :gspider: - :mythic: :mrstaff: Image It's a tie in 24 turns! | +100 xp | Streak: 828 , 9:41 PM";
        let mut stats = Stats::default();
        for (event, posted) in Watcher::new().observe(&[msg(T, tie, "")], Some("Player"), Instant::now()) {
            stats.record(event, posted);
        }
        assert_eq!((stats.wins, stats.losses, stats.draws), (0, 0, 1));
    }

    #[test]
    fn messages_not_written_by_owo_are_ignored_whatever_their_text() {
        let now = Instant::now();
        let mut forged = msg(T, CASH, "");
        forged.from_owo = false;
        let mut watcher = Watcher::new();
        watcher.sent("owovote", snowflake_at(T - 1000), now);
        let mut fake_vote = msg(T + 500, VOTE_READY, "");
        fake_vote.from_owo = false;
        assert!(watcher.observe(&[forged, fake_vote], Some("Player"), now).is_empty());
    }

    #[test]
    fn nameless_replies_need_a_command_from_zippy() {
        let now = Instant::now();
        let reply = [msg(T + 1000, HUNTBOT_BACK, ""), msg(T + 2000, VOTE_READY, "")];
        let mut watcher = Watcher::new();
        assert!(watcher.observe(&reply, None, now).is_empty());

        let mut watcher = Watcher::new();
        watcher.sent("owohb", snowflake_at(T), now);
        watcher.sent("owovote", snowflake_at(T + 500), now);
        let events = watcher.observe(&reply, None, now);
        assert_eq!(sets(&events), [(Stat::HuntbotReadyAt, T + 1000), (Stat::VoteReadyAt, T + 2000)]);

        let mut waiting = Watcher::new();
        waiting.sent("owohb", snowflake_at(T), now);
        waiting.sent("owovote", snowflake_at(T + 500), now);
        let events = waiting.observe(&[msg(T + 1000, HUNTBOT_HUNTING, ""), msg(T + 2000, VOTE_WAIT, "")], None, now);
        assert_eq!(
            sets(&events),
            [(Stat::HuntbotReadyAt, T + 1000 + (18 * 60 + 7) * 60_000), (Stat::VoteReadyAt, T + 2000 + 7 * 3_600_000)]
        );

        let mut late = Watcher::new();
        late.sent("owovote", snowflake_at(T), now);
        assert!(late.observe(&reply[1..], None, now + REPLY_TIMEOUT).is_empty());
    }

    #[test]
    fn older_readings_never_replace_newer_ones_and_reminders_fire_once() {
        let mut stats = Stats::default();
        assert!(stats.apply(Stat::Cash, Reading { value: 10, at: 200 }));
        assert!(!stats.apply(Stat::Cash, Reading { value: 5, at: 100 }));
        assert_eq!(stats.get(Stat::Cash).unwrap().value, 10);

        stats.apply(Stat::VoteReadyAt, Reading { value: T - 5, at: T - 10 });
        assert!(stats.due_reminders(T).is_empty(), "already ready when first seen");
        stats.apply(Stat::VoteReadyAt, Reading { value: T + 100, at: T });
        assert!(stats.due_reminders(T + 99).is_empty());
        assert_eq!(stats.due_reminders(T + 100), ["You can vote for OwO"]);
        assert!(stats.due_reminders(T + 101).is_empty());
        stats.apply(Stat::VoteReadyAt, Reading { value: T + 200, at: T + 200 });
        assert!(stats.due_reminders(T + 300).is_empty(), "a later 'ready now' reply is not announced again");
    }

    #[test]
    fn log_round_trips_and_ignores_garbage() {
        let path = std::env::temp_dir().join(format!("zippy-stats-test-{}.jsonl", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut log = Log::new(path.clone());
        let mut stats = Stats::default();
        let (stat, reading) = stats.record(Event::Set(Stat::Cash, 7), T).unwrap();
        log.append(stat, reading);
        assert_eq!(stats.record(Event::Set(Stat::Cash, 7), T), None, "same reading is not written twice");
        fs::write(&path, format!("{}not json\n{{\"stat\":\"cash\",\"value\":\"x\",\"t\":1}}\n", fs::read_to_string(&path).unwrap())).unwrap();
        let mut loaded = Stats::default();
        Log::new(path.clone()).load(&mut loaded);
        assert_eq!(loaded.get(Stat::Cash), Some(Reading { value: 7, at: T }));
        let _ = fs::remove_file(&path);
    }
}
