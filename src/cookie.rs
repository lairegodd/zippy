//! Gives a cookie to someone the player names on the dashboard. Zippy first types `owocookie @username`;
//! Discord turns a username it knows into a mention when the message is sent. If OwO could not tell who was
//! meant, the dashboard asks for the person's User ID, and `owocookie <@ID>` names them exactly.

use std::time::{Duration, Instant};

use crate::gems::{ChatMessage, owo_content};
use crate::snowflake_unix_ms;
use crate::stats::duration_ms;

const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recipient {
    Username(String),
    Id(u64),
}

impl Recipient {
    /// A Discord username (2 to 32 letters, digits, `_` or `.`, an optional `@` in front), or a User ID
    /// (17 to 20 digits, also as a pasted `<@ID>`). Nothing else is accepted, so only this command is typed.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let id = text.strip_prefix("<@").and_then(|t| t.strip_suffix('>')).map(|t| t.trim_start_matches('!')).unwrap_or(text);
        if (17..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()) {
            return id.parse().ok().map(Recipient::Id);
        }
        let name = text.strip_prefix('@').unwrap_or(text);
        let valid = (2..=32).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
            && !name.contains("..");
        valid.then(|| Recipient::Username(name.to_ascii_lowercase()))
    }

    pub fn command(&self) -> String {
        match self {
            Recipient::Username(name) => format!("owocookie @{name}"),
            Recipient::Id(id) => format!("owocookie <@{id}>"),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Recipient::Username(name) => format!("@{name}"),
            Recipient::Id(id) => format!("the user with ID {id}"),
        }
    }
}

/// What came of a cookie, for the dashboard and the stats.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Given(Recipient),
    /// OwO says the next cookie can be given at `ready_at` (Unix ms), when it says when.
    Wait { ready_at: Option<u64> },
    ToSelf,
    /// OwO could not tell who `@username` was; the dashboard asks for their User ID.
    NeedId(String),
    /// OwO could not tell who the User ID was either.
    Unknown(u64),
    NoReply(Recipient),
}

#[derive(Debug)]
enum Step {
    Idle,
    Queued(Recipient),
    Await { recipient: Recipient, after: u64, sent: Instant },
    NeedId(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Reply {
    Given,
    Wait(Option<u64>),
    ToSelf,
    Unresolved,
}

/// Reads a reply about a cookie, from the player's own exchange only: OwO names the player in each of them.
fn classify(content: &str, player: &str, posted: u64) -> Option<Reply> {
    let lower = content.to_lowercase();
    if !lower.contains("cookie") || !lower.contains(&player.to_lowercase()) {
        return None;
    }
    let upper = content.to_uppercase();
    if let Some(at) = ["NEED TO WAIT ", "SEND A COOKIE IN ", "AGAIN IN "].iter().find_map(|m| upper.split_once(m).map(|(_, r)| r)) {
        return Some(Reply::Wait(duration_ms(at.trim_start_matches(['*', ' '])).map(|w| posted + w)));
    }
    if lower.contains("yourself") {
        return Some(Reply::ToSelf);
    }
    // The plain cookie count means OwO read the command without a person in it.
    let unresolved = ["you currently have", "tag ", "mention", "couldn't find", "could not find", "invalid", "not a user"];
    if unresolved.iter().any(|m| lower.contains(m)) {
        return Some(Reply::Unresolved);
    }
    ["gave", "got a cookie", "received", "sent", "nom"].iter().any(|m| lower.contains(m)).then_some(Reply::Given)
}

/// What the dashboard shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub state: &'static str,
    /// The recipient being sent to, or the username OwO could not find.
    pub recipient: Option<String>,
}

impl View {
    pub fn idle() -> Self {
        Self { state: "idle", recipient: None }
    }
}

pub struct Cookies {
    step: Step,
}

impl Cookies {
    pub fn new() -> Self {
        Self { step: Step::Idle }
    }

    /// Asks for a cookie to go to `recipient`; refused while one is already on its way.
    pub fn give(&mut self, recipient: Recipient) -> Result<(), &'static str> {
        if matches!(self.step, Step::Queued(_) | Step::Await { .. }) {
            return Err("a cookie is already on its way; wait for OwO's answer");
        }
        self.step = Step::Queued(recipient);
        Ok(())
    }

    pub fn due(&self) -> Option<String> {
        match &self.step {
            Step::Queued(recipient) => Some(recipient.command()),
            _ => None,
        }
    }

    /// Discord listed nobody with the username, so nothing was sent; the dashboard asks for a User ID.
    pub fn not_found(&mut self) -> Option<Outcome> {
        let Step::Queued(Recipient::Username(name)) = &self.step else { return None };
        let name = name.clone();
        self.step = Step::NeedId(name.clone());
        Some(Outcome::NeedId(name))
    }

    pub fn sent(&mut self, after: u64, now: Instant) {
        if let Step::Queued(recipient) = std::mem::replace(&mut self.step, Step::Idle) {
            self.step = Step::Await { recipient, after, sent: now };
        }
    }

    pub fn observe(&mut self, messages: &[ChatMessage], player: Option<&str>, now: Instant) -> Option<Outcome> {
        let Step::Await { recipient, after, sent } = &self.step else { return None };
        let found = player.and_then(|player| {
            let mut newer: Vec<&ChatMessage> = messages.iter().filter(|m| m.id > *after).collect();
            newer.sort_unstable_by_key(|m| m.id);
            newer.into_iter().find_map(|m| classify(owo_content(m)?, player, snowflake_unix_ms(m.id)))
        });
        let outcome = match (found, recipient) {
            (None, _) if now < *sent + REPLY_TIMEOUT => return None,
            (None, recipient) => Outcome::NoReply(recipient.clone()),
            (Some(Reply::Given), recipient) => Outcome::Given(recipient.clone()),
            (Some(Reply::Wait(ready_at)), _) => Outcome::Wait { ready_at },
            (Some(Reply::ToSelf), _) => Outcome::ToSelf,
            (Some(Reply::Unresolved), Recipient::Username(name)) => Outcome::NeedId(name.clone()),
            (Some(Reply::Unresolved), Recipient::Id(id)) => Outcome::Unknown(*id),
        };
        self.step = match &outcome {
            Outcome::NeedId(name) => Step::NeedId(name.clone()),
            _ => Step::Idle,
        };
        Some(outcome)
    }

    pub fn view(&self) -> View {
        let (state, recipient) = match &self.step {
            Step::Idle => ("idle", None),
            Step::Queued(r) | Step::Await { recipient: r, .. } => ("sending", Some(r.label())),
            Step::NeedId(name) => ("need_id", Some(format!("@{name}"))),
        };
        View { state, recipient }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake_at;

    const T: u64 = 1_767_268_800_000;

    fn msg(at: u64, summary: &str) -> ChatMessage {
        ChatMessage { id: snowflake_at(at), summary: summary.to_owned(), detail: String::new(), from_owo: true }
    }

    fn answer(recipient: Recipient, reply: &str) -> (Option<Outcome>, View) {
        let now = Instant::now();
        let mut cookies = Cookies::new();
        cookies.give(recipient).unwrap();
        cookies.sent(snowflake_at(T), now);
        let outcome = cookies.observe(&[msg(T + 1000, reply)], Some("Ho-oh"), now);
        (outcome, cookies.view())
    }

    #[test]
    fn only_usernames_and_ids_are_typed() {
        assert_eq!(Recipient::parse("@Some_User.1").unwrap().command(), "owocookie @some_user.1");
        assert_eq!(Recipient::parse(" 408785106942164992 ").unwrap().command(), "owocookie <@408785106942164992>");
        assert_eq!(Recipient::parse("<@!408785106942164992>"), Some(Recipient::Id(408_785_106_942_164_992)));
        for bad in ["", "a", "two words", "name\nowo give", "@everyone here", "x".repeat(33).as_str(), "a..b", "<@12>"] {
            assert_eq!(Recipient::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_cookie_given_counts_once_and_others_replies_do_not() {
        let given = "OwO Verified App , 🍪 | Ho-oh gave friend a cookie! nom nom , 6:10 PM";
        let (outcome, view) = answer(Recipient::parse("friend").unwrap(), given);
        assert_eq!(outcome, Some(Outcome::Given(Recipient::Username("friend".into()))));
        assert_eq!(view.state, "idle");
        let (outcome, _) = answer(Recipient::parse("friend").unwrap(), &given.replace("Ho-oh", "Someone"));
        assert_eq!(outcome, None, "a cookie between two other people is not ours");
    }

    #[test]
    fn an_unknown_username_asks_for_the_id_and_a_wait_is_read() {
        let count = "OwO Verified App , :9cookieeat: | Ho-oh! You currently have 3 cookies! Yummy! c:< , 6:10 PM";
        let (outcome, view) = answer(Recipient::parse("@nobody").unwrap(), count);
        assert_eq!(outcome, Some(Outcome::NeedId("nobody".into())));
        assert_eq!((view.state, view.recipient.as_deref()), ("need_id", Some("@nobody")));
        let (outcome, _) = answer(Recipient::Id(408_785_106_942_164_992), count);
        assert_eq!(outcome, Some(Outcome::Unknown(408_785_106_942_164_992)));

        let wait = "OwO Verified App , ⏱ | Nu! Ho-oh! You can send a cookie in 2H 3M! , 6:10 PM";
        let (outcome, _) = answer(Recipient::parse("friend").unwrap(), wait);
        assert_eq!(outcome, Some(Outcome::Wait { ready_at: Some(T + 1000 + (2 * 60 + 3) * 60_000) }));
    }

    #[test]
    fn a_username_discord_cannot_find_asks_for_the_id_without_sending() {
        let mut cookies = Cookies::new();
        cookies.give(Recipient::Username("friend".into())).unwrap();
        assert_eq!(cookies.not_found(), Some(Outcome::NeedId("friend".into())));
        assert_eq!(cookies.due(), None, "nothing is sent until an ID is given");
        assert_eq!(cookies.view().state, "need_id");
        cookies.give(Recipient::Id(408_785_106_942_164_992)).unwrap();
        assert_eq!(cookies.not_found(), None, "an ID is never looked up in the list");
        assert_eq!(cookies.due().as_deref(), Some("owocookie <@408785106942164992>"));
    }

    #[test]
    fn silence_ends_the_attempt_and_a_second_one_waits_its_turn() {
        let now = Instant::now();
        let mut cookies = Cookies::new();
        cookies.give(Recipient::Id(408_785_106_942_164_992)).unwrap();
        assert!(cookies.give(Recipient::Username("other".into())).is_err());
        cookies.sent(snowflake_at(T), now);
        assert_eq!(cookies.observe(&[], Some("Ho-oh"), now), None);
        assert!(matches!(cookies.observe(&[], Some("Ho-oh"), now + REPLY_TIMEOUT), Some(Outcome::NoReply(_))));
        assert!(cookies.give(Recipient::Username("other".into())).is_ok());
    }
}
