use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;

pub const GEM_TYPES: [(u8, RangeInclusive<u16>); 3] = [(1, 51..=57), (3, 65..=71), (4, 72..=78)];
pub const HUNT_COMMAND: &str = "owoh";
const USE_PREFIX: &str = "owo use ";
const OWO_AUTHOR: &str = "OwO Verified App";
const MAX_INVENTORY_REQUESTS: u32 = 3;

pub struct ChatMessage {
    pub id: u64,
    pub summary: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Inventory,
    Use(Vec<u16>),
}

impl Action {
    pub fn command(&self) -> String {
        match self {
            Action::Inventory => "owo inv".to_owned(),
            Action::Use(ids) => {
                let ids: Vec<String> = ids.iter().map(u16::to_string).collect();
                format!("{USE_PREFIX}{}", ids.join(" "))
            }
        }
    }
}

/// A queued use can wait behind other commands, so only a hunt sent after it can show whether it worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UseState {
    Queued,
    Sent,
    HuntSentAfter,
}

pub struct GemKeeper {
    player: Option<String>,
    player_probe: Option<(String, u64)>,
    stock: Option<BTreeMap<u16, u32>>,
    last_hunt: u64,
    missing: BTreeSet<u8>,
    inventory_after: Option<u64>,
    inventory_requests: u32,
    pending: BTreeMap<u8, (u16, UseState)>,
    reported_empty: BTreeSet<u8>,
    preferred: [u16; 3],
}

impl GemKeeper {
    pub fn new(start: u64, preferred: [u16; 3]) -> Self {
        Self {
            player: None,
            player_probe: None,
            stock: None,
            last_hunt: start,
            missing: BTreeSet::new(),
            inventory_after: None,
            inventory_requests: 0,
            pending: BTreeMap::new(),
            reported_empty: BTreeSet::new(),
            preferred,
        }
    }

    pub fn sent(&mut self, command: &str, earliest_id: u64) {
        if self.player.is_none() {
            self.player_probe = Some((command.to_owned(), earliest_id));
        }
        if command == HUNT_COMMAND {
            for (_, state) in self.pending.values_mut() {
                if *state == UseState::Sent {
                    *state = UseState::HuntSentAfter;
                }
            }
        } else if let Some(ids) = command.strip_prefix(USE_PREFIX) {
            let ids: Vec<u16> = ids.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            for (id, state) in self.pending.values_mut() {
                if *state == UseState::Queued && ids.contains(id) {
                    *state = UseState::Sent;
                }
            }
        }
    }

    pub fn observe(&mut self, messages: &[ChatMessage]) -> Vec<Action> {
        self.learn_player(messages);
        let Some(player) = self.player.clone() else { return Vec::new() };

        if self.stock.is_none()
            && let Some(after) = self.inventory_after
            && let Some(gems) = find_inventory(messages, &player, after)
        {
            println!("[GEM] Inventory read: {}", describe_stock(&gems));
            self.stock = Some(gems);
            self.inventory_after = None;
        }

        let hunt = messages
            .iter()
            .filter(|m| m.id > self.last_hunt)
            .filter_map(|m| Some((m.id, hunt_active_gems(owo_content(&m.summary)?, &player)?)))
            .max_by_key(|(id, _)| *id);
        let fresh_hunt = hunt.is_some();
        if let Some((id, active)) = hunt {
            self.last_hunt = id;
            self.record_hunt(&active);
        }
        self.resolve(fresh_hunt)
    }

    /// Others in the channel send the same commands, so only an echo from a single author is trusted.
    fn learn_player(&mut self, messages: &[ChatMessage]) {
        let Some((command, earliest)) = &self.player_probe else { return };
        let mut authors = messages
            .iter()
            .filter(|m| m.id >= *earliest)
            .filter_map(|m| split_summary(&m.summary))
            .filter(|(_, content)| content == command)
            .map(|(author, _)| author);
        let Some(author) = authors.next() else { return };
        if authors.any(|other| other != author) {
            return;
        }
        println!("[GEM] Player name: {author}");
        self.player = Some(author.to_owned());
        self.player_probe = None;
    }

    fn record_hunt(&mut self, active: &BTreeSet<u8>) {
        for (kind, _) in &GEM_TYPES {
            if active.contains(kind) {
                self.pending.remove(kind);
                self.missing.remove(kind);
                continue;
            }
            match self.pending.get(kind) {
                // A gem used before this hunt that is still inactive was not in the inventory.
                Some(&(id, UseState::HuntSentAfter)) => {
                    self.pending.remove(kind);
                    println!("[GEM] owo use {id} did not activate gem{kind}; treating ID {id} as used up.");
                    if let Some(stock) = &mut self.stock {
                        stock.insert(id, 0);
                    }
                }
                Some(_) => continue,
                None => {}
            }
            if self.missing.insert(*kind) {
                println!("[GEM] gem{kind} ran out.");
            }
        }
    }

    fn resolve(&mut self, fresh_hunt: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.missing.is_empty() {
            return actions;
        }
        let Some(stock) = &mut self.stock else {
            if fresh_hunt && self.inventory_requests < MAX_INVENTORY_REQUESTS {
                self.inventory_requests += 1;
                self.inventory_after = Some(self.last_hunt);
                actions.push(Action::Inventory);
            } else if fresh_hunt && self.inventory_requests == MAX_INVENTORY_REQUESTS {
                self.inventory_requests += 1;
                println!("[GEM] Could not read the owo inv reply after {MAX_INVENTORY_REQUESTS} tries; gem feature off.");
            }
            return actions;
        };
        let mut uses = Vec::new();
        for (slot, (kind, ids)) in GEM_TYPES.iter().enumerate() {
            if !self.missing.contains(kind) || self.pending.contains_key(kind) {
                continue;
            }
            let allowed = *ids.start()..=self.preferred[slot];
            let best = stock.range(allowed).rev().find(|&(_, &count)| count > 0).map(|(&id, _)| id);
            match best {
                Some(id) => {
                    *stock.entry(id).or_default() -= 1;
                    self.pending.insert(*kind, (id, UseState::Queued));
                    self.missing.remove(kind);
                    self.reported_empty.remove(kind);
                    uses.push(id);
                }
                None => {
                    if self.reported_empty.insert(*kind) {
                        println!("[GEM] No gem{kind} at or below ID {} left in the inventory.", self.preferred[slot]);
                    }
                }
            }
        }
        if !uses.is_empty() {
            actions.push(Action::Use(uses));
        }
        actions
    }
}

fn describe_stock(stock: &BTreeMap<u16, u32>) -> String {
    if stock.is_empty() {
        return "no gems".to_owned();
    }
    stock.iter().map(|(id, count)| format!("{id}x{count}")).collect::<Vec<_>>().join(", ")
}

pub fn split_summary(summary: &str) -> Option<(&str, &str)> {
    let (author, rest) = summary.split_once(" , ")?;
    let content = rest.rsplit_once(" , ").map_or(rest, |(content, _time)| content);
    let author = author.split_once(" Server Tag:").map_or(author, |(name, _)| name);
    Some((author.trim(), content.trim()))
}

pub fn owo_content(summary: &str) -> Option<&str> {
    summary.strip_prefix(OWO_AUTHOR)?.strip_prefix(" , ")
}

fn gem_type(token: &str) -> Option<u8> {
    let inner = token.strip_prefix(':')?.strip_suffix(':')?;
    let (tier, kind) = inner.split_at_checked(1)?;
    if !tier.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let kind: u8 = kind.strip_prefix("gem")?.parse().ok()?;
    GEM_TYPES.iter().any(|(t, _)| *t == kind).then_some(kind)
}

fn superscript_number(token: &str) -> Option<u32> {
    let mut value: u32 = 0;
    for c in token.chars() {
        let digit = "⁰¹²³⁴⁵⁶⁷⁸⁹".chars().position(|d| d == c)? as u32;
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    (!token.is_empty()).then_some(value)
}

fn inventory_gems(content: &str) -> Vec<(u16, u32)> {
    let tokens: Vec<&str> = content.split_whitespace().collect();
    tokens
        .windows(3)
        .filter_map(|w| {
            if w[0].len() != 3 || !w[0].bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let id: u16 = w[0].parse().ok()?;
            let kind = gem_type(w[1])?;
            let count = superscript_number(w[2])?;
            let in_range = GEM_TYPES.iter().any(|(t, ids)| *t == kind && ids.contains(&id));
            in_range.then_some((id, count))
        })
        .collect()
}

/// OwO splits a long inventory into several messages; only the first has the header.
fn find_inventory(messages: &[ChatMessage], player: &str, after: u64) -> Option<BTreeMap<u16, u32>> {
    let header = format!("{player}'s Inventory");
    let mut ordered: Vec<&ChatMessage> = messages.iter().filter(|m| m.id > after).collect();
    ordered.sort_by_key(|m| m.id);
    let start = ordered
        .iter()
        .rposition(|m| owo_content(&m.summary).is_some_and(|c| c.contains(&header)))?;
    let mut stock = BTreeMap::new();
    for (i, message) in ordered[start..].iter().enumerate() {
        let Some(content) = owo_content(&message.summary) else { break };
        if i > 0 && content.contains("'s Inventory") {
            break;
        }
        let gems = inventory_gems(content);
        if i > 0 && gems.is_empty() && !content.split_whitespace().any(|t| superscript_number(t).is_some()) {
            break;
        }
        stock.extend(gems);
    }
    Some(stock)
}

fn hunt_active_gems(content: &str, player: &str) -> Option<BTreeSet<u8>> {
    let (_, after) = content.split_once(&format!("| {player}"))?;
    if !after.starts_with(" spent ") && !after.starts_with(", hunt is empowered") {
        return None;
    }
    let header = after.split('|').next().unwrap_or(after);
    Some(header.split_whitespace().filter_map(gem_type).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUNT_NO_GEMS: &str = "OwO Verified App , 🌱 | Player spent 5 :cowoncy: and caught a common :common: :bug: ! :blank: | :hsnake: :hsquid: :hedgebot: gained 1xp! , 12:00 PM";
    const HUNT_ALL_GEMS: &str = "OwO Verified App , 🌱 | Player, hunt is empowered by :fgem3: [800/1000] :fgem4: [800/1000] :lgem1: [66/100] ! :blank: | You found: :bee: :bug: , 12:00 PM";
    const HUNT_GEM1_GONE: &str = "OwO Verified App , 🌱 | Player, hunt is empowered by :fgem3: [790/1000] :fgem4: [790/1000] ! :blank: | You found: :bee: , 12:01 PM";
    const INVENTORY: &str = "OwO Verified App , ====== Player's Inventory ====== 001 :cring: ⁰³⁷ 049 :flootbox: ⁰⁰² 050 :box: ⁰⁰⁵ 051 :cgem1: ³⁴¹ 052 :ugem1: ²⁵⁴ 053 :rgem1: ¹⁵⁴ 054 :egem1: ¹⁵¹ 055 :mgem1: ⁰³⁷ 065 :cgem3: ²⁸⁵ 066 :ugem3: ²⁷¹ 067 :rgem3: ¹⁸⁸ 068 :egem3: ¹⁵⁰ 069 :mgem3: ⁰⁴¹ 070 :lgem3: ⁰⁰¹ 072 :cgem4: ³¹⁷ 073 :ugem4: ²²⁴ 074 :rgem4: ¹⁵² 075 :egem4: ¹³⁸ ";
    const INVENTORY_PAGE2: &str = "OwO Verified App , 117 :sythe: ⁰⁵¹ 118 :crune: ⁰⁰³ 2-- 🖼 ⁰⁰⁷ , 12:02 PM";

    fn msg(id: u64, summary: &str) -> ChatMessage {
        ChatMessage { id, summary: summary.to_owned() }
    }

    const HIGHEST: [u16; 3] = [57, 71, 78];

    fn keeper_knowing_player() -> GemKeeper {
        keeper_preferring(HIGHEST)
    }

    fn keeper_preferring(preferred: [u16; 3]) -> GemKeeper {
        let mut keeper = GemKeeper::new(100, preferred);
        keeper.sent(HUNT_COMMAND, 100);
        keeper.observe(&[msg(101, "Player Server Tag: TAG , owoh , 12:00 PM")]);
        assert_eq!(keeper.player.as_deref(), Some("Player"));
        keeper
    }

    #[test]
    fn player_is_not_learned_while_another_user_echoes_the_same_command() {
        let mut keeper = GemKeeper::new(100, HIGHEST);
        keeper.sent(HUNT_COMMAND, 101);
        keeper.observe(&[msg(101, "Player , owoh , 12:00 PM"), msg(102, "Other , owoh , 12:00 PM")]);
        assert_eq!(keeper.player, None);
        keeper.sent(HUNT_COMMAND, 200);
        keeper.observe(&[msg(102, "Other , owoh , 12:00 PM"), msg(200, "Player , owoh , 12:01 PM")]);
        assert_eq!(keeper.player.as_deref(), Some("Player"));
    }

    #[test]
    fn hunt_reply_lists_active_gem_types() {
        let active = |s| hunt_active_gems(owo_content(s).unwrap(), "Player");
        assert_eq!(active(HUNT_ALL_GEMS), Some(BTreeSet::from([1, 3, 4])));
        assert_eq!(active(HUNT_GEM1_GONE), Some(BTreeSet::from([3, 4])));
        assert_eq!(active(HUNT_NO_GEMS), Some(BTreeSet::new()));
        assert_eq!(hunt_active_gems(owo_content(HUNT_NO_GEMS).unwrap(), "Other"), None);
        assert_eq!(hunt_active_gems(owo_content(HUNT_NO_GEMS).unwrap(), "Play"), None);
    }

    #[test]
    fn inventory_reads_gem_ids_and_superscript_counts_only() {
        let gems = inventory_gems(owo_content(INVENTORY).unwrap());
        assert_eq!(gems.len(), 15);
        assert!(gems.contains(&(51, 341)));
        assert!(gems.contains(&(70, 1)));
        assert!(gems.contains(&(75, 138)));
        assert!(!gems.iter().any(|(id, _)| *id == 1 || *id == 49 || *id == 50));
    }

    #[test]
    fn inventory_follows_continuation_page_but_not_other_inventories() {
        let messages = [
            msg(10, INVENTORY),
            msg(11, "OwO Verified App , 056 :lgem1: ⁰⁰⁴ 078 :fgem4: ⁰⁰² , 12:02 PM"),
            msg(12, INVENTORY_PAGE2),
            msg(13, "OwO Verified App , ====== Other's Inventory ====== 057 :fgem1: ⁰⁹⁹ "),
        ];
        let stock = find_inventory(&messages, "Player", 5).unwrap();
        assert_eq!(stock.get(&56), Some(&4));
        assert_eq!(stock.get(&78), Some(&2));
        assert_eq!(stock.get(&57), None);
        assert_eq!(find_inventory(&messages, "Player", 10), None);
    }

    #[test]
    fn missing_gems_read_inventory_once_then_use_highest_tier() {
        let mut keeper = keeper_knowing_player();
        assert_eq!(keeper.observe(&[msg(200, HUNT_NO_GEMS)]), vec![Action::Inventory]);
        assert_eq!(keeper.observe(&[msg(200, HUNT_NO_GEMS)]), vec![], "same hunt twice");
        let actions = keeper.observe(&[msg(200, HUNT_NO_GEMS), msg(201, INVENTORY)]);
        assert_eq!(actions, vec![Action::Use(vec![55, 70, 75])]);
        assert_eq!(actions[0].command(), "owo use 55 70 75");

        keeper.observe(&[msg(300, HUNT_ALL_GEMS)]);
        assert_eq!(keeper.observe(&[msg(400, HUNT_GEM1_GONE)]), vec![Action::Use(vec![55])]);
    }

    #[test]
    fn last_gem_of_a_tier_falls_back_to_lower_tier() {
        let mut keeper = keeper_knowing_player();
        keeper.observe(&[msg(200, HUNT_NO_GEMS)]);
        keeper.observe(&[msg(201, INVENTORY)]);
        keeper.observe(&[msg(300, HUNT_ALL_GEMS)]);
        let gem3_gone = HUNT_ALL_GEMS.replace(":fgem3: [800/1000] ", "");
        assert_eq!(keeper.observe(&[msg(400, &gem3_gone)]), vec![Action::Use(vec![69])]);
    }

    #[test]
    fn use_that_did_not_activate_marks_id_empty_and_tries_next() {
        let mut keeper = keeper_knowing_player();
        keeper.observe(&[msg(200, HUNT_NO_GEMS)]);
        keeper.observe(&[msg(201, INVENTORY)]);
        keeper.sent("owo use 55 70 75", 250);
        keeper.sent(HUNT_COMMAND, 260);
        let actions = keeper.observe(&[msg(300, HUNT_ALL_GEMS.replace(":lgem1: [66/100] ", "").as_str())]);
        assert_eq!(actions, vec![Action::Use(vec![54])]);
    }

    #[test]
    fn use_is_judged_only_by_a_hunt_sent_after_it() {
        let gem1_gone = HUNT_ALL_GEMS.replace(":lgem1: [66/100] ", "");
        let mut keeper = keeper_knowing_player();
        keeper.observe(&[msg(200, HUNT_NO_GEMS)]);
        assert_eq!(keeper.observe(&[msg(201, INVENTORY)]), vec![Action::Use(vec![55, 70, 75])]);
        assert_eq!(keeper.observe(&[msg(300, &gem1_gone)]), vec![], "use still queued");
        keeper.sent("owo use 55 70 75", 350);
        assert_eq!(keeper.observe(&[msg(400, &gem1_gone)]), vec![], "no hunt sent since the use");
        assert_eq!(keeper.stock.as_ref().and_then(|s| s.get(&55)), Some(&36));
        keeper.sent(HUNT_COMMAND, 450);
        assert_eq!(keeper.observe(&[msg(500, &gem1_gone)]), vec![Action::Use(vec![54])]);
    }

    #[test]
    fn preferred_gem_falls_back_to_lower_ids_never_higher() {
        let inventory = INVENTORY.replace("054 :egem1: ¹⁵¹ ", "");
        let mut keeper = keeper_preferring([54, 69, 78]);
        keeper.observe(&[msg(200, HUNT_NO_GEMS)]);
        let actions = keeper.observe(&[msg(201, &inventory)]);
        assert_eq!(actions, vec![Action::Use(vec![53, 69, 75])]);

        let inventory = INVENTORY.replace("051 :cgem1: ³⁴¹ ", "");
        let mut keeper = keeper_preferring([51, 65, 72]);
        keeper.observe(&[msg(200, HUNT_NO_GEMS)]);
        let actions = keeper.observe(&[msg(201, &inventory)]);
        assert_eq!(actions, vec![Action::Use(vec![65, 72])]);
    }

    #[test]
    fn hunts_before_start_and_without_known_player_are_ignored() {
        let mut keeper = GemKeeper::new(100, HIGHEST);
        assert_eq!(keeper.observe(&[msg(200, HUNT_NO_GEMS)]), vec![], "player unknown");
        let mut keeper = keeper_knowing_player();
        assert_eq!(keeper.observe(&[msg(50, HUNT_NO_GEMS)]), vec![], "older than start");
    }

    #[test]
    fn unreadable_inventory_is_requested_at_most_three_times() {
        let mut keeper = keeper_knowing_player();
        let requests: usize = (0..6u64)
            .map(|i| keeper.observe(&[msg(200 + i, HUNT_NO_GEMS)]).len())
            .sum();
        assert_eq!(requests, MAX_INVENTORY_REQUESTS as usize);
    }
}
