# Zippy

Zippy is a Windows console program that sends OwO bot commands (`owoh`, `owob`) for you in a Discord channel open in your web browser. It reads the chat before every command and stops when OwO asks for a captcha or announces a ban.

> **Read this first.** This bot is for educational purposes. Automating a user account breaks Discord's Terms of Service (ToS) and OwO's rules against macros. Your Discord account or your OwO profile can be banned. You use Zippy at your own risk.

## What it does

- Sends `owoh`, `owob`, or both every 18 seconds (OwO's 15 second cooldown plus 3 seconds for lag), with 2.5 seconds between the two commands.
- Works with Discord in a browser: Chrome, Edge, Firefox, Brave, Opera, Opera GX, Vivaldi, Chromium, Floorp, and Zen. It does not work with the Discord desktop app (for now, will fix this soon).
- Locks onto the channel that is open when it starts. Switching tab or channel pauses it.
- Scans the chat before typing anything. It pauses and alerts you when:
  - a captcha or "are you a real human" message shows up in the channel,
  - a new DM from OwO arrives (usually a captcha),
  - the Discord window closes.
- Waits without typing while the chat cannot be read or is scrolled up, so it never types over a captcha it cannot see.
- Stops completely when it sees an OwO ban message, until you check it and press F8.
- Alerts with a Windows notification that keeps ringing, a flashing taskbar button, and three beeps.
- Keeps hunt gems active: when a gem runs out, Zippy reads your inventory with `owo inv` and sends `owo use` with the gem IDs you chose in the settings.

To type, Zippy brings the browser window to the front, types the command, and then gives focus back to the window you were using. If you switch windows while it is typing, Zippy stops and pauses, so no keystrokes end up in another app. Clear any leftover text in the Discord message box, then press F8.

## Requirements

- Windows 10 or 11.
- Discord open in one of the browsers listed above, logged in, with the channel visible.

## Usage

1. Open the Discord channel where you want to farm, in your browser.
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
| `--check` | Finds the Discord tab, prints what Zippy can read (message box, newest message, captcha, ban, unread OwO DMs), and exits without sending anything. |
| `--test-notification` | Sends a test alert so you can confirm Windows notifications are not blocked. |
| `--update` | Installs the newest release without asking, then exits. |

### Updates

At startup Zippy asks GitHub whether a newer release exists. If one does, it asks before downloading. Press Enter (or type `y`) to install it: Zippy downloads the new `zippy.exe`, checks its SHA-256 checksum against the one GitHub lists for the release, replaces itself, and restarts. Your `settings.ini` stays as it is.

If the download fails or the checksum does not match, nothing is replaced. Zippy needs write access to its own folder to update, so keep it somewhere like `Documents` rather than `Program Files`. To turn the check off, set `update_check = off`.

### When the chat cannot be read

Zippy reads the page through Windows accessibility (UI Automation). If it reports that chat messages are not readable, keep the Discord tab visible, or start the browser with:

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

# Check GitHub for a newer Zippy at startup: on or off.
update_check = on
```

| Key | Allowed values | Default |
| --- | --- | --- |
| `mode` | `hunt`, `battle`, `both` | `both` |
| `gem1` | 51 to 57 | 57 |
| `gem3` | 65 to 71 | 71 |
| `gem4` | 72 to 78 | 78 |
| `update_check` | `on`, `off` | `on` |

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

## What Zippy leaves on your PC

- `settings.ini` next to the exe.
- The registry key `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\Zippy`, which Windows needs to show Zippy's notifications. Delete it if you stop using Zippy.
- Right after an update, `zippy.old.exe` may sit next to `zippy.exe` until the next start removes it.

Besides the commands it types into Discord, Zippy only contacts GitHub (`api.github.com` and the release download) to check for and download updates. Turn that off with `update_check = off`.

## Credits

- [Paiiss/owofarm](https://github.com/Paiiss/owofarm) for give me an inspiration to make this project.
- Some features from [Kiy0w0/owomizu](https://github.com/Kiy0w0/owomizu) may be added to Zippy later.
