use std::path::Path;

use crate::gems::{GEM_TYPES, HUNT_COMMAND};

pub const FILE_NAME: &str = "settings.ini";

const DEFAULT_FILE: &str = "\
# Zippy settings. Lines starting with # are ignored.
# Restart Zippy after editing this file.

# What to send every cycle: hunt, battle, or both.
mode = both

# Gem to use when one runs out, by inventory ID.
# If you don't have that gem, the next lower ID is used (54 -> 53 -> 52 -> 51).
# gem1: 51-57, gem3: 65-71, gem4: 72-78
gem1 = 57
gem3 = 71
gem4 = 78

# Check GitHub for a newer Zippy at startup: on or off.
update_check = on
";

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
}

#[derive(Debug, PartialEq, Eq)]
pub struct Settings {
    pub mode: Mode,
    pub gems: [u16; 3],
    pub update_check: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { mode: Mode::Both, gems: GEM_TYPES.map(|(_, ids)| *ids.end()), update_check: true }
    }
}

impl Settings {
    pub fn load_or_create(path: &Path) -> Result<(Self, bool), String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map(|s| (s, false)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::write(path, DEFAULT_FILE)
                    .map_err(|e| format!("could not create {}: {e}", path.display()))?;
                Ok((Self::default(), true))
            }
            Err(e) => Err(format!("could not read {}: {e}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut settings = Self::default();
        for (number, line) in text.lines().enumerate().map(|(i, l)| (i + 1, l.trim())) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("line {number}: expected `key = value`, found `{line}`"));
            };
            let (key, value) = (key.trim().to_lowercase(), value.trim());
            if key == "mode" {
                settings.mode = match value.to_lowercase().as_str() {
                    "hunt" => Mode::Hunt,
                    "battle" => Mode::Battle,
                    "both" => Mode::Both,
                    _ => return Err(format!("line {number}: mode must be hunt, battle or both, not `{value}`")),
                };
                continue;
            }
            if key == "update_check" {
                settings.update_check = match value.to_lowercase().as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err(format!("line {number}: update_check must be on or off, not `{value}`")),
                };
                continue;
            }
            let Some(slot) = GEM_TYPES.iter().position(|(kind, _)| key == format!("gem{kind}")) else {
                return Err(format!("line {number}: unknown setting `{key}`"));
            };
            let ids = &GEM_TYPES[slot].1;
            settings.gems[slot] = value
                .parse()
                .ok()
                .filter(|id| ids.contains(id))
                .ok_or_else(|| format!("line {number}: {key} must be {}-{}, not `{value}`", ids.start(), ids.end()))?;
        }
        Ok(settings)
    }

    pub fn describe(&self) -> String {
        let gems: Vec<String> =
            GEM_TYPES.iter().zip(self.gems).map(|((kind, _), id)| format!("gem{kind}={id}")).collect();
        let update_check = if self.update_check { "on" } else { "off" };
        format!("mode={:?}, {}, update_check={update_check}", self.mode, gems.join(", ")).to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_parses_to_defaults() {
        assert_eq!(Settings::parse(DEFAULT_FILE), Ok(Settings::default()));
        assert_eq!(Settings::default().gems, [57, 71, 78]);
    }

    #[test]
    fn reads_mode_and_gem_choices() {
        let settings = Settings::parse("MODE = Hunt\ngem1=54\n  gem4 = 72  \n# gem3 = 65\n").unwrap();
        assert_eq!(settings, Settings { mode: Mode::Hunt, gems: [54, 71, 72], update_check: true });
        assert_eq!(Settings::parse("mode = battle").unwrap().mode, Mode::Battle);
        assert!(!Settings::parse("update_check = OFF").unwrap().update_check);
        assert!(Settings::parse("update_check = maybe").unwrap_err().contains("on or off"));
    }

    #[test]
    fn rejects_gem_ids_of_another_type_and_unknown_keys() {
        assert!(Settings::parse("gem1 = 65").unwrap_err().contains("gem1 must be 51-57"));
        assert!(Settings::parse("gem3 = 64").is_err());
        assert!(Settings::parse("gem4 = abc").is_err());
        assert!(Settings::parse("mode = farm").is_err());
        assert!(Settings::parse("gem2 = 58").unwrap_err().contains("unknown setting `gem2`"));
        assert!(Settings::parse("mode both").unwrap_err().contains("line 1"));
    }
}
