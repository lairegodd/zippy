# Contributing to Zippy
Bug reports, OwO reply samples, and pull requests are all welcome ^^

## Reporting a bug

Open an issue with:

- The Zippy version (shown on the dashboard and at startup).
- Where Discord runs: the desktop app (Stable, PTB or Canary) or which browser.
- What you expected and what happened, with the lines from the Activity log or the console.
- The output of `zippy.exe --check`. It only reads the page and sends nothing, so it is safe to run while Zippy is running.
- For a reply Zippy misreads: the exact text of OwO's reply, or a screenshot of it.

**Remove personal data before you post:** your Discord name, server and channel names, other players' names, and the dashboard link (its `#key=` part controls your Zippy). Replace names with `Player` and `Other`.

## Security problems

If you find a way for another program or website to control Zippy or read the dashboard, do not post the details in a public issue. Open an issue that only asks for a private contact, and the details can be shared from there.

## Building and checking

You need Windows 10 or 11 and Rust 1.88 or newer.

```powershell
cargo clippy --release --all-targets -- -D warnings
cargo test --release
powershell -ExecutionPolicy Bypass -File build-release.ps1
```

All three must pass before a pull request is merged. Use `build-release.ps1` for any exe you share: it keeps your Windows user folder out of the file.

The dashboard files in `web/` are built into `zippy.exe`, so rebuild after changing them.

## Pull requests

- Keep each pull request to one change, and explain why it is needed.
- Add a test for a bug fix where you can. Tests that read OwO replies use the real reply text with names replaced by `Player` and `Other`.
- Test against real Discord before asking for review, and say what you tried. Running Zippy sends real commands in your channel, so use a channel where that is fine.
- Update `README.md` when something a user sees or sets changes.
- Keep comments short and only where the code cannot say it itself.
- Text people read (README, dashboard, log lines) is plain English. Do not use em dashes.

### Dashboard

- Put text on the page with `textContent` or `el()`, never `innerHTML`: messages from Discord end up on the dashboard.
- Keep every check in `src/web.rs` (the key, the Host and Origin checks, the size and time limits). Any new API route needs the key.
- Check both themes, keyboard use, and a phone-sized window.

## License

Zippy is licensed under the [GNU General Public License v3.0](LICENSE). By sending a pull request, you agree that your contribution is released under the same license.
