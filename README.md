# Frog-Buddy
the frog is a buddy!

A tiny Rust (egui) desktop frog that sits in a corner of your screen and pops up every so often to remind you to drink water, stretch, rest your eyes, etc.

- Click the frog to dismiss a reminder.
- Right-click the frog, or the tray icon > Show, for settings (screen corner, reminder interval, quit).
- Lives in the system tray (right-click: Show / Quit). Only one copy runs at a time.
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

Pixel font: [Press Start 2P](https://fonts.google.com/specimen/Press+Start+2P), SIL Open Font License (see `assets/OFL.txt`).
