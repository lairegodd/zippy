use std::fs;
use std::path::Path;

use crate::daily::Daily;
use crate::gems::{GEM_TYPES, HUNT_COMMAND};

pub const FILE_NAME: &str = "settings.ini";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Hunt,
    Battle,
    Both,
}

impl Mode {
    pub fn commands(self) -> &'static [&'static str] {
        match self {
            Mode::Hunt => &[HUNT_COMMAND],
            Mode::Battle => &["owob"],
            Mode::Both => &[HUNT_COMMAND, "owob"],
        }
    }

    pub fn hunts(self) -> bool {
        self != Mode::Battle
    }

    fn key(self) -> &'static str {
        match self {
            Mode::Hunt => "hunt",
            Mode::Battle => "battle",
            Mode::Both => "both",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub mode: Mode,
    pub gems: [u16; 3],
    pub owo: bool,
    pub pray: bool,
    pub daily: [bool; Daily::ALL.len()],
    pub stats: bool,
    pub reminders: bool,
    pub update_check: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Both,
            gems: GEM_TYPES.map(|(_, ids)| *ids.end()),
            owo: true,
            pray: true,
            daily: [true; Daily::ALL.len()],
            stats: true,
            reminders: true,
            update_check: true,
        }
    }
}

fn on_off(flag: bool) -> &'static str {
    if flag { "on" } else { "off" }
}

impl Settings {
    pub fn load_or_create(path: &Path) -> Result<(Self, bool), String> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map(|s| (s, false)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let settings = Self::default();
                settings.save(path)?;
                Ok((settings, true))
            }
            Err(e) => Err(format!("could not read {}: {e}", path.display())),
        }
    }

    /// Writes a temporary file and renames it over the old one, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let temporary = path.with_extension("ini.tmp");
        fs::write(&temporary, self.render())
            .and_then(|()| fs::rename(&temporary, path))
            .map_err(|e| {
                let _ = fs::remove_file(&temporary);
                format!("could not save {}: {e}", path.display())
            })
    }

    /// Commands added after a file was written stay off until it names them, so an update never starts new ones.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut settings = Self {
            owo: false,
            pray: false,
            daily: [false; Daily::ALL.len()],
            stats: false,
            ..Self::default()
        };
        for (number, line) in text.lines().enumerate().map(|(i, l)| (i + 1, l.trim())) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("line {number}: expected `key = value`, found `{line}`"));
            };
            let (key, value) = (key.trim().to_lowercase(), value.trim());
            settings.set(&key, value).map_err(|e| format!("line {number}: {e}"))?;
        }
        Ok(settings)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key == "mode" {
            self.mode = match value.to_lowercase().as_str() {
                "hunt" => Mode::Hunt,
                "battle" => Mode::Battle,
                "both" => Mode::Both,
                _ => return Err(format!("mode must be hunt, battle or both, not `{value}`")),
            };
            return Ok(());
        }
        let switch = match key {
            "owo" => Some(&mut self.owo),
            "pray" => Some(&mut self.pray),
            "stats" => Some(&mut self.stats),
            "reminders" => Some(&mut self.reminders),
            "update_check" => Some(&mut self.update_check),
            _ => Daily::ALL.iter().position(|d| d.key() == key).map(|i| &mut self.daily[i]),
        };
        if let Some(flag) = switch {
            *flag = match value.to_lowercase().as_str() {
                "on" => true,
                "off" => false,
                _ => return Err(format!("{key} must be on or off, not `{value}`")),
            };
            return Ok(());
        }
        let Some(slot) = GEM_TYPES.iter().position(|(kind, _)| key == format!("gem{kind}")) else {
            return Err(format!("unknown setting `{key}`"));
        };
        let ids = &GEM_TYPES[slot].1;
        self.gems[slot] = value
            .parse()
            .ok()
            .filter(|id| ids.contains(id))
            .ok_or_else(|| format!("{key} must be {}-{}, not `{value}`", ids.start(), ids.end()))?;
        Ok(())
    }

    /// A copy with one setting changed, checked by the same rules as the file.
    pub fn with(&self, key: &str, value: &str) -> Result<Self, String> {
        let mut changed = self.clone();
        changed.set(&key.trim().to_lowercase(), value.trim())?;
        Ok(changed)
    }

    /// Every setting as (key, value), in file order.
    pub fn values(&self) -> Vec<(String, String)> {
        let mut values = vec![("mode".to_owned(), self.mode.key().to_owned())];
        values.extend(GEM_TYPES.iter().zip(self.gems).map(|((kind, _), id)| (format!("gem{kind}"), id.to_string())));
        values.push(("owo".to_owned(), on_off(self.owo).to_owned()));
        values.push(("pray".to_owned(), on_off(self.pray).to_owned()));
        values.extend(Daily::ALL.iter().zip(self.daily).map(|(d, on)| (d.key().to_owned(), on_off(on).to_owned())));
        values.push(("stats".to_owned(), on_off(self.stats).to_owned()));
        values.push(("reminders".to_owned(), on_off(self.reminders).to_owned()));
        values.push(("update_check".to_owned(), on_off(self.update_check).to_owned()));
        values
    }

    pub fn render(&self) -> String {
        let value = |key: &str| self.values().into_iter().find(|(k, _)| k == key).map(|(_, v)| v).unwrap_or_default();
        format!(
            "\
# Zippy settings. Lines starting with # are ignored.
# Changes take effect the next time Zippy starts sending.

# What to send on a timer: hunt, battle, or both.
mode = {}

# Gem to use when one runs out, by inventory ID.
# If you don't have that gem, the next lower ID is used (54 -> 53 -> 52 -> 51).
# gem1: 51-57, gem3: 65-71, gem4: 72-78
gem1 = {}
gem3 = {}
gem4 = {}

# Plain \"owo\" every 11 seconds, and \"owopray\" every 5 minutes: on or off.
owo = {}
pray = {}

# Daily commands: owodaily once a day, the others until OwO says today's limit is used up: on or off.
daily = {}
run = {}
pup = {}
piku = {}
army = {}

# Read cash, cookies, boss tickets, vote and huntbot on its own every 30 to 60 minutes: on or off.
# The dashboard's refresh button reads them any time. This sends owocash, owo cookie, owoboss t,
# owovote and owohb (owohb collects a finished huntbot).
stats = {}

# Windows notifications when huntbot, daily, cookie or vote is ready: on or off.
reminders = {}

# Check GitHub for a newer Zippy at startup: on or off.
update_check = {}
",
            value("mode"),
            value("gem1"),
            value("gem3"),
            value("gem4"),
            value("owo"),
            value("pray"),
            value("daily"),
            value("run"),
            value("pup"),
            value("piku"),
            value("army"),
            value("stats"),
            value("reminders"),
            value("update_check"),
        )
    }

    pub fn enabled_dailies(&self) -> impl Iterator<Item = Daily> + '_ {
        Daily::ALL.into_iter().zip(self.daily).filter(|(_, on)| *on).map(|(daily, _)| daily)
    }

    pub fn describe(&self) -> String {
        self.values().iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_file_parses_back_to_the_same_settings() {
        assert_eq!(Settings::parse(&Settings::default().render()), Ok(Settings::default()));
        let changed = Settings::default().with("mode", "battle").unwrap().with("pup", "off").unwrap();
        assert_eq!(Settings::parse(&changed.render()), Ok(changed));
        assert_eq!(Settings::default().gems, [57, 71, 78]);
    }

    #[test]
    fn reads_mode_and_gem_choices() {
        let settings = Settings::parse("MODE = Hunt\ngem1=54\n  gem4 = 72  \n# gem3 = 65\n").unwrap();
        assert_eq!((settings.mode, settings.gems), (Mode::Hunt, [54, 71, 72]));
        assert_eq!(Settings::parse("mode = battle").unwrap().mode, Mode::Battle);
        assert!(!Settings::parse("update_check = OFF").unwrap().update_check);
        assert!(Settings::parse("update_check = maybe").unwrap_err().contains("on or off"));
    }

    #[test]
    fn extra_commands_are_on_only_when_the_file_says_so() {
        let settings = Settings::parse("owo = on\nrun = on\npiku = ON\npup = off\n").unwrap();
        assert!(settings.owo && !settings.pray);
        assert_eq!(settings.enabled_dailies().collect::<Vec<_>>(), [Daily::Run, Daily::Piku]);
        let older_file = Settings::parse("mode = both\ngem1 = 57\nupdate_check = on\n").unwrap();
        assert!(!older_file.owo && !older_file.pray && !older_file.stats && older_file.enabled_dailies().next().is_none());
        assert!(Settings::parse("run = yes").unwrap_err().contains("run must be on or off"));
    }

    #[test]
    fn rejects_bad_values_and_unknown_keys() {
        assert!(Settings::parse("gem1 = 65").unwrap_err().contains("gem1 must be 51-57"));
        assert!(Settings::parse("gem3 = 64").is_err());
        assert!(Settings::parse("gem4 = abc").is_err());
        assert!(Settings::parse("mode = farm").is_err());
        assert!(Settings::parse("gem2 = 58").unwrap_err().contains("unknown setting `gem2`"));
        assert!(Settings::parse("mode both").unwrap_err().contains("line 1"));
        assert!(Settings::default().with("mode", "both\nupdate_check = off").is_err());
        assert!(Settings::default().with("../settings", "on").is_err());
    }
}
