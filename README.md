# Frog-Buddy
the frog is a buddy!

A tiny Rust (egui) desktop frog that sits in a corner of your screen and pops up every so often to remind you to drink water, stretch, rest your eyes, etc.

- Click the frog to dismiss a reminder.
- Right-click the tray icon > **Settings** for the settings window (corner, interval, how long tips stay up, European units, show a tip now, quit).
- Lives in the system tray (right-click: Settings / Quit). Only one copy runs at a time.
- Settings live in `%APPDATA%\FrogBuddy\config.txt`.
- The installer adds a Start Menu shortcut and starts Frog Buddy with Windows.

## Build

```
cargo run --release
```

## Releasing

Bump `version` in `Cargo.toml`, commit, then tag and push:

```
git tag v1.0.0
git push origin v1.0.0
```

GitHub Actions builds `frog-buddy.exe` and `frog-buddy.msi` and attaches them to a new GitHub Release.

## Credits

Pixel font: [Jersey 10](https://fonts.google.com/specimen/Jersey+10), SIL Open Font License (see `assets/OFL.txt`).
