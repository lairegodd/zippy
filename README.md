# Zippy

Zippy is a Windows console program that sends OwO bot commands (`owoh`, `owob`) for you in a Discord channel, in the Discord desktop app or in your web browser. It reads the chat before every command and stops when OwO asks for a captcha or announces a ban.

> **Read this first!** This bot is for educational purposes. Automating a user account breaks Discord's Terms of Service (ToS) and OwO's rules against macros. Your Discord account or your OwO profile can be banned. You use Zippy at your own risk.

## How's This Work, I Wonder?

- Sends these commands, each on its own timer, with at least 2.5 seconds between any two:

  | Command | How often |
  | --- | --- |
  | `owo` | every 11 seconds |
  | `owoh`, `owob` | every 18 seconds (OwO's 15 second cooldown plus 3 seconds for lag) |
  | `owopray` | every 5 minutes |
  | `oworun`, `owopup`, `owopiku` | every minute, until today's limit is used up |
  | `owoarmy` | every 10 seconds, until today's limit is used up |
  | `owodaily` | once a day, after OwO's daily reset |
  | `owocash`, `owo cookie`, `owoboss t`, `owovote`, `owohb` | with `stats = on`, every 30 to 60 minutes (a random wait each time); any time from the dashboard's refresh buttons |
  | `owohb (nh)` (your full duration) | with `huntbot = on`, each time huntbot is back; you type OwO's password |

  When several are due at once, Zippy sends them in priority order:

  1. Gem upkeep (`owo inv`, then `owo use`) and huntbot's commands; then any command you asked for from the dashboard's right-click menu; then `owoh` and `owob`. After `owo inv`, nothing but `owoh` and `owob` is sent until its reply is read and the `owo use` has gone out (at most 20 seconds), so another command never lands in between. While OwO waits for the huntbot password, nothing else is sent.
  2. `owo` and `owopray`, and the reads you ask for with a refresh button.
  3. The daily commands and the automatic reads.

  Within a level the command that has waited longest goes first, and anything left waiting over two minutes moves to the front, so none of them is starved. A command from the right-click menu is queued once however often you ask, so it can delay a hunt by a few seconds but never hold hunting back.
- Reads each daily command's limit from OwO's reply instead of assuming it, and prints what is left (`[DAILY] owoarmy: 13 left today.`). Once a command is used up it is not sent again until OwO's day resets (midnight US Pacific time, following daylight saving). Limits that grow from one day to the next are followed automatically.
- Stops a daily command until the reset, to be safe, when OwO's reply cannot be read, OwO does not answer twice in a row, or the count does not go down after a use. It never sends one more than 40 times a day. If someone else sends the same command at the same moment, Zippy does not trust the reply and asks again later.
- Saves the daily counts and limits in `zippy-dailies.json` next to the exe, so stopping or quitting does not lose them: a restart on the same OwO day carries on from the saved counts, and a command already used up is not sent again before the reset.
- Works with the Discord desktop app (Stable, PTB, and Canary) and with Discord in a browser: Chrome, Edge, Firefox, Brave, Opera, Opera GX, Vivaldi, Chromium, Floorp, and Zen. If several are open, Zippy picks the one in front.
- Locks onto the channel that is open when it starts. Switching tab or channel pauses it, and so does closing the desktop app to the tray.
- Runs one copy at a time, so two Zippy windows never send every command twice.
- Scans the chat before typing anything. It pauses and alerts you when:
  - a captcha or "are you a real human" message shows up in the channel (messages from other people saying "captcha" are ignored),
  - a new DM from OwO arrives (usually a captcha),
  - the Discord window closes.
- Waits without typing while the chat cannot be read or is scrolled up, so it never types over a captcha it cannot see.
- Only trusts replies from the real OwO bot for stats, gems, and daily limits: the message header must show the name `OwO` with Discord's **Verified App** tag, which no person can give their own account. Discord does not show user IDs to Zippy, so it cannot check OwO's ID (`408785106942164992`) directly. A message whose author Zippy cannot tell (the first one on screen, when its header scrolled away) is never trusted for stats, but still counts for captcha and ban alerts, to stay safe. `zippy.exe --check` shows how many messages on screen came from the real OwO.
- Stops completely when it sees an OwO ban message, until you check it and press F8.
- Alerts with a Windows notification that keeps ringing, a flashing taskbar button, and three beeps.
- Keeps hunt gems active: when a gem runs out, Zippy reads your inventory with `owo inv` and sends `owo use` with the gem IDs you chose in the settings. OwO shows each gem's uses left after the hunt (`[38/75]`), so a gem at `[0/75]` counts as used up at once and Zippy replaces it right after that hunt, not one hunt later.
- Starts huntbot again each time it comes back (see [Huntbot](#huntbot)).

### Huntbot

With `huntbot = on`, Zippy sends huntbot out again for its full duration whenever it comes back:

1. When huntbot's countdown runs out (or Zippy does not know it yet), Zippy sends `owohb`. That collects a finished hunt, and a second `owohb` shows the profile. Zippy reads the Duration trait from it (`Duration - 7.6H`, or from the level: level 0 is 0.5h and every level adds 0.1h, up to 24h at level 235) and what a full hunt costs (`Current Max Autohunt: ... for 5,776 cowoncy`).
2. When the profile names a cost, Zippy sends `owocash` first. If you have less than a full hunt costs, it does not ask OwO for a password you would solve for nothing: it waits 30 minutes and checks again.
3. Zippy sends `owohb 7.6h` (your duration). OwO answers with a password shown as an image.
4. **You type the password.** Zippy rings (Windows notification, flashing taskbar button, three beeps) and the dashboard's Huntbot panel shows a box for it. In the Zippy window you can also type it and press Enter. Zippy then sends `owohb 7.6h <password>`.
5. OwO answers `BEEP BOOP ... YOU SPENT ... I WILL BE BACK IN 1H 36M`, and Zippy waits for that countdown.

While OwO waits for the password, Zippy sends nothing else, so the password image stays at the bottom of the chat. OwO resets the password after 5 minutes; if none was typed by then, Zippy carries on farming and asks again 30 minutes later. If OwO says the password is wrong, type it again. If OwO says you don't have enough cowoncy, Zippy sends a notification and tries again in 30 minutes.

Zippy does not read the password image itself. The password is OwO's check that a person is there, so you always type it yourself.

The **Send owohb now** button (or typing `hb` in the Zippy window) starts a new try at once, for example after you get more cowoncy.

To type, Zippy brings the Discord window to the front (restoring it if it is minimized), types the command, and then gives focus back to the window you were using. It never posts anything but the command:

- If the message box already has text in it (a draft), Zippy pauses instead of typing.
- While you hold Shift, Ctrl, Alt, or the Windows key, it waits.
- Before pressing Enter, it checks that the message box holds exactly the command. If you pressed keys while it was typing, or switched windows, it pauses without sending. Clear the message box, then press F8.

If Discord runs as administrator, Windows blocks Zippy's keystrokes, so run Zippy as administrator too.

## How to use?

1. Open the Discord channel where you want to farm, in the desktop app or your browser.
2. Run `zippy.exe`. On the first run it creates `settings.ini` next to the exe. It asks how to run:

   ```
     1) Open the dashboard in your browser (default)
     2) Run here in this window
     3) Settings
   ```

   Press Enter for the dashboard, type `2` to run in the console window as before, or `3` to change settings from the console.
3. Leave the channel open and scrolled to the bottom.

| Key | Action |
| --- | --- |
| F8 | Start, pause or resume. After a ban alert, F8 confirms you checked it and resumes. |
| F10 | Quit Zippy (and its dashboard) |

The keys work from any window, in both the dashboard and console modes.

### Dashboard

The dashboard opens in your default browser at `http://127.0.0.1:<port>`. Keep the Zippy window open while you use it; closing that window stops both Zippy and the dashboard.

- **Start, Pause, Stop.** Start looks for your Discord channel and begins sending. Pause holds sending (Start then reads Resume). Stop ends the run; Start begins a new one with the current settings.
- **Reminders.** When huntbot is back, `owo daily` is ready, you can give a cookie again, or you can vote, Zippy shows it on the dashboard and, with `reminders = on`, sends a Windows notification. It only tells you when a countdown runs out, not about something that was already ready. The refresh button (hover it for its name, **Check reminders again**) sends only this panel's reads: `owohb`, `owovote` and `owo cookie`, once each. `owohb` collects your huntbot if it has finished. `owo daily` is not read by it, since sending `owodaily` would claim the daily; its time shows after `owodaily` is sent.
- **Huntbot.** What automatic huntbot is doing, your full duration and what a full hunt costs. When OwO asks for the password, a box for it shows here and the browser tab's title changes to "Password needed". **Send owohb now** starts a new try.
- **Statistics.** Cowoncy, cookies, luck points (from `owopray`), animal essence (from `owohb`), battle streak (from `owob`, with this run's wins, losses and draws), daily streak and unread mail (from `owodaily`), and boss tickets, each with the time it was read. Unread mail is counted from the line OwO adds to `owodaily` while mail is unread; a daily without that line sets it to none. The refresh button (**Read stats again a.k.a refresh**) sends only this panel's reads: `owocash`, `owo cookie` and `owoboss t`, once each. `owo cookie` answers for both panels: when both ask, it is sent once, and a reply that carries numbers for the other panel updates them too, so nothing OwO says is thrown away. With `stats = on`, Zippy reads both panels on its own every 30 to 60 minutes. Reads queued when you press Stop are dropped. Level and XP are not shown: OwO sends them as an image Zippy can't read.
- **Daily commands.** How many uses of each daily command are left today, out of today's limit. The bar shrinks with each use.
- **Right-click menu.** Right-click a row in Huntbot, Reminders, Statistics or Daily commands (or focus it with Tab and press Shift+F10, or the menu key) and choose **Send … next** to send that row's command ahead of the routine ones. The rows send `owohb` (Huntbot, Animal essence), `owodaily` (owo daily, Daily streak), `owo cookie`, `owovote`, `owocash`, `owopray` (Luck points), `owob` (Battle streak), `owoboss t`, and each daily command. The command still follows its own rules: `owopray` and `owob` wait for their cooldown, a daily command that is used up, turned off, or still waiting for OwO's reply is not sent, and `owohb` is refused while OwO waits for the huntbot password. Activity says when it goes out, or why it can't. Only these commands can be asked for; the dashboard never sends text of its own to Discord.
- **Activity.** The same messages the console shows.
- **Settings.** Every setting from `settings.ini`, saved as soon as you change it. Changes apply at the next Start.
- Light and dark themes, switched with the sun and moon button at the top; the new theme spreads out from the button. The dashboard follows Windows until you pick one.

Level and XP are not shown: OwO sends them as an image, which Zippy cannot read.

Every value Zippy reads is also appended to `zippy-stats.jsonl` next to the exe (one JSON object per line: time, stat, value), so the dashboard shows the last known values after a restart. The file starts over (the old one is kept as `zippy-stats.old.jsonl`) once it passes 1 MB.

### Command line options

| Option | What it does |
| --- | --- |
| `--dashboard` | Skips the menu and opens the dashboard. |
| `--console` | Skips the menu and runs in the console window, starting right away. |
| `--check` | Finds the Discord window, prints what Zippy can read (message box, newest message, captcha, ban, unread OwO DMs), and exits without sending anything. It also works while Zippy is running. |
| `--test-notification` | Sends a test alert so you can confirm Windows notifications are not blocked. |
| `--update` | Installs the newest release without asking, then exits. |

## Settings

`settings.ini` sits next to `zippy.exe`. Change it from the dashboard's Settings, from option 3 in the startup menu, or in a text editor. Changes apply at the next Start.

 Note: A `settings.ini` from an older Zippy has none of the newer keys, so `owo`, `owopray`, the daily commands, `stats` and `huntbot` stay off after updating, because each of them sends commands. Turn them on in Settings. A fresh `settings.ini` has them on.

If you run out of the gem you picked, Zippy uses the next lower ID you own (for example 54, then 53, 52, 51). It never uses a higher ID than the one you set. Gems are only managed in `hunt` and `both` mode.

### Updates

At startup Zippy asks GitHub whether a newer release exists. If one does, it asks before downloading. Press Enter (or type `y`) to install it: Zippy downloads the new `zippy.exe`, checks its SHA-256 checksum against the one GitHub lists for the release, replaces itself, and restarts. Your `settings.ini` stays as it is.

If the download fails or the checksum does not match, nothing is replaced. Zippy needs write access to its own folder to update, so keep it somewhere like `Documents` rather than `Program Files`. To turn the check off, set `update_check = off`.

### [IMPORTANT] Windows SmartScreen Blocks the program to run

The first time you open a `zippy.exe` downloaded with your browser, Windows may show "Windows protected your PC". Zippy is not code-signed, and SmartScreen warns about any new file that few people have downloaded yet. Check that the file came from this repository's Releases page (its SHA256 is listed in the release notes), then click **More info** and **Run anyway**. Windows remembers that choice for this file.

You can also right-click `zippy.exe`, choose **Properties**, tick **Unblock**, and press **OK** before the first run.

Updates that Zippy installs itself do not show the prompt again. Windows only checks files marked as downloaded by a browser, and Zippy's updater checks the SHA256 instead.

A code-signing certificate would not remove the prompt right away either. Since 2024, signed files also start without reputation and only stop warning after many clean downloads. The one way to skip it entirely is the Microsoft Store.

> This is a safe program, Your data, such as Discord Account, Your Hardware Specifications and such, would NEVER get sent anywhere. you can check the code for yourself. This is open-source, after all.

## Building from source

You need Rust 1.88 or newer.

```powershell
cargo test
.\build-release.ps1
```

The exe is written to `target\release\zippy.exe`. Use `build-release.ps1` for anything you share: a plain `cargo build --release` embeds your home folder path (and Windows user name) in the exe.


## What Zippy leaves on your PC

- `settings.ini`, `zippy-stats.jsonl` and `zippy-dailies.json` next to the exe.
- The registry key `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\Zippy`, which Windows needs to show Zippy's notifications. Delete it if you stop using Zippy.
- Right after an update, `zippy.old.exe` may sit next to `zippy.exe` until the next start removes it.

Besides the commands it types into Discord, Zippy only contacts GitHub (`api.github.com` and the release download) to check for and download updates. Turn that off with `update_check = off`. The dashboard is served by Zippy itself on `127.0.0.1`; nothing is sent anywhere else.

## Future updates

1. Reminders for `owo huntbot`, `owo daily`, captchas, and more, sent through a webhook (to Discord or a phone).
2. Inventory on the dashboard, once there is a reliable source for OwO's item icons.
3. Automatic captcha solving.
4. Android support.
5. A helper for daily quests.
6. An account switcher for clearing quests that need two players.

### FAQ
-  "When the chat cannot be read, what should i do?"

Zippy reads Discord through Windows accessibility (UI Automation). Chromium-based apps stop exposing a page nobody has looked at for a while, so if the chat is not readable Zippy brings the Discord window to the front once, briefly, to wake it. In the desktop app nothing else is needed. In a browser, if Zippy still reports that chat messages are not readable, keep the Discord tab visible, or start the browser with:

```
--force-renderer-accessibility
```

## Credits

- [Paiiss/owofarm](https://github.com/Paiiss/owofarm) for give me an inspiration to make this project.
- Some features from [Kiy0w0/owomizu](https://github.com/Kiy0w0/owomizu) may be added to Zippy in the future.
- The localhost dashboard uses the [Nunito](https://github.com/googlefonts/nunito) font, licensed under the SIL Open Font License 1.1 (`web/OFL-Nunito.txt`).

## License

Zippy is free software under the [GNU General Public License v3.0](LICENSE): you may use, change and share it, and anyone who shares a changed version must share its source under the same license. The Nunito font keeps its own license (SIL Open Font License 1.1).

Want to help? See [CONTRIBUTING.md](CONTRIBUTING.md).
