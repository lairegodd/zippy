# Zippy

Zippy is a Windows console program that sends OwO bot commands (`owoh`, `owob`) for you in a Discord channel, in the Discord desktop app or in your web browser. It reads the chat before every command and stops when OwO asks for a captcha or announces a ban.

> **Read this first.** This bot is for educational purposes. Automating a user account breaks Discord's Terms of Service (ToS) and OwO's rules against macros. Your Discord account or your OwO profile can be banned. You use Zippy at your own risk.

## What it does

- Sends these commands, each on its own timer, with at least 2.5 seconds between any two:

  | Command | How often |
  | --- | --- |
  | `owo` | every 11 seconds |
  | `owoh`, `owob` | every 18 seconds (OwO's 15 second cooldown plus 3 seconds for lag) |
  | `owopray` | every 5 minutes |
  | `oworun`, `owopup`, `owopiku`, `owoarmy` | until today's limit is used up |

  When several are due at once, they go in that order; a command that has waited longer goes first, so none of them is starved.
- Reads each daily command's limit from OwO's reply instead of assuming it, and prints what is left (`[DAILY] owoarmy: 13 left today.`). Once a command is used up it is not sent again until OwO's day resets (midnight US Pacific time, following daylight saving). Limits that grow from one day to the next are followed automatically.
- Stops a daily command until the reset, to be safe, when OwO's reply cannot be read, OwO does not answer twice in a row, or the count does not go down after a use. It never sends one more than 40 times a day. If someone else sends the same command at the same moment, Zippy does not trust the reply and asks again later. Zippy does not remember the counts between runs, so each restart sends every daily command once to read them.
- Works with the Discord desktop app (Stable, PTB, and Canary) and with Discord in a browser: Chrome, Edge, Firefox, Brave, Opera, Opera GX, Vivaldi, Chromium, Floorp, and Zen. If several are open, Zippy picks the one in front.
- Locks onto the channel that is open when it starts. Switching tab or channel pauses it, and so does closing the desktop app to the tray.
- Runs one copy at a time, so two Zippy windows never send every command twice.
- Scans the chat before typing anything. It pauses and alerts you when:
  - a captcha or "are you a real human" message shows up in the channel,
  - a new DM from OwO arrives (usually a captcha),
  - the Discord window closes.
- Waits without typing while the chat cannot be read or is scrolled up, so it never types over a captcha it cannot see.
- Stops completely when it sees an OwO ban message, until you check it and press F8.
- Alerts with a Windows notification that keeps ringing, a flashing taskbar button, and three beeps.
- Keeps hunt gems active: when a gem runs out, Zippy reads your inventory with `owo inv` and sends `owo use` with the gem IDs you chose in the settings.

To type, Zippy brings the Discord window to the front (restoring it if it is minimized), types the command, and then gives focus back to the window you were using. It never posts anything but the command:

- If the message box already has text in it (a draft), Zippy pauses instead of typing.
- While you hold Shift, Ctrl, Alt, or the Windows key, it waits.
- Before pressing Enter, it checks that the message box holds exactly the command. If you pressed keys while it was typing, or switched windows, it pauses without sending. Clear the message box, then press F8.

If Discord runs as administrator, Windows blocks Zippy's keystrokes, so run Zippy as administrator too.

## Requirements

- Windows 10 or 11.
- The Discord desktop app, or Discord open in one of the browsers listed above, logged in.

## Usage

1. Open the Discord channel where you want to farm, in the desktop app or your browser.
2. Run `zippy.exe`. On the first run it creates `settings.ini` next to the exe.
3. Leave the channel open and scrolled to the bottom.

| Key | Action |
| --- | --- |
| F8 | Pause or resume. After a ban alert, F8 confirms you checked it and resumes. |
| F10 | Quit |

The keys work from any window.

### Command line options

| Option | What it does |
| --- | --- |
| `--check` | Finds the Discord window, prints what Zippy can read (message box, newest message, captcha, ban, unread OwO DMs), and exits without sending anything. It also works while Zippy is running. |
| `--test-notification` | Sends a test alert so you can confirm Windows notifications are not blocked. |
| `--update` | Installs the newest release without asking, then exits. |

### Updates

At startup Zippy asks GitHub whether a newer release exists. If one does, it asks before downloading. Press Enter (or type `y`) to install it: Zippy downloads the new `zippy.exe`, checks its SHA-256 checksum against the one GitHub lists for the release, replaces itself, and restarts. Your `settings.ini` stays as it is.

If the download fails or the checksum does not match, nothing is replaced. Zippy needs write access to its own folder to update, so keep it somewhere like `Documents` rather than `Program Files`. To turn the check off, set `update_check = off`.

### When the chat cannot be read

Zippy reads Discord through Windows accessibility (UI Automation). In the desktop app this works without any setup. In a browser, if Zippy reports that chat messages are not readable, keep the Discord tab visible, or start the browser with:

```
--force-renderer-accessibility
```

## Settings

`settings.ini` sits next to `zippy.exe`. Restart Zippy after editing it.

```ini
# What to send every cycle: hunt, battle, or both.
mode = both

# Gem to use when one runs out, by inventory ID.
gem1 = 57
gem3 = 71
gem4 = 78

# Plain "owo" every 11 seconds, and "owopray" every 5 minutes: on or off.
owo = on
pray = on

# Daily commands, sent until OwO says today's limit is used up: on or off.
run = on
pup = on
piku = on
army = on

# Check GitHub for a newer Zippy at startup: on or off.
update_check = on
```

| Key | Allowed values | Default |
| --- | --- | --- |
| `mode` | `hunt`, `battle`, `both` | `both` |
| `gem1` | 51 to 57 | 57 |
| `gem3` | 65 to 71 | 71 |
| `gem4` | 72 to 78 | 78 |
| `owo`, `pray` | `on`, `off` | `on` |
| `run`, `pup`, `piku`, `army` | `on`, `off` | `on` |
| `update_check` | `on`, `off` | `on` |

A `settings.ini` from an older Zippy has none of the new keys, so `owo`, `owopray`, and the daily commands stay off after updating, and Zippy prints how to turn them on. Add the lines above with `on` to use them. A fresh `settings.ini` has them on.

If you run out of the gem you picked, Zippy uses the next lower ID you own (for example 54, then 53, 52, 51). It never uses a higher ID than the one you set. Gems are only managed in `hunt` and `both` mode.

## Building from source

You need Rust 1.88 or newer.

```powershell
cargo test
.\build-release.ps1
```

The exe is written to `target\release\zippy.exe`. Use `build-release.ps1` for anything you share: a plain `cargo build --release` embeds your home folder path (and Windows user name) in the exe.

### Publishing a release

The updater looks for the latest GitHub release whose tag is `vX.Y.Z` and newer than the running build, and downloads the asset named `zippy.exe`. So for each release, bump `version` in `Cargo.toml`, build with `build-release.ps1`, tag the release with the same version (for example `v1.1.0`), and attach the exe as `zippy.exe`.

Release notes follow this layout (leave out a section that has nothing in it):

```markdown
## Changelogs

### Added
1. ...

### Fixed
1. ...

### Notes
- ...

### Disclaimer
- ...

**SHA-256 of zippy.exe:** `...`
```

## What Zippy leaves on your PC

- `settings.ini` next to the exe.
- The registry key `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\Zippy`, which Windows needs to show Zippy's notifications. Delete it if you stop using Zippy.
- Right after an update, `zippy.old.exe` may sit next to `zippy.exe` until the next start removes it.

Besides the commands it types into Discord, Zippy only contacts GitHub (`api.github.com` and the release download) to check for and download updates. Turn that off with `update_check = off`.

## Future updates

Planned, not built yet:

1. A local web dashboard for statistics, battle streaks, pray count, settings, and more.
2. Reminders for `owo huntbot`, `owo daily`, captchas, and more, sent through a webhook.
3. Automatic captcha solving.
4. Android support.
5. A helper for daily quests.
6. An account switcher for quests that need two players, such as "Receive an action from a friend".

## Credits

- [Paiiss/owofarm](https://github.com/Paiiss/owofarm) for give me an inspiration to make this project.
- Some features from [Kiy0w0/owomizu](https://github.com/Kiy0w0/owomizu) may be added to Zippy later.
