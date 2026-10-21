# Penguin Notes

Markdown notes with a local AI sidebar (Pengu), voice input that runs on your computer, and sync with your iPhone's Calendar and Notes apps.

**New here? Read the [easy step-by-step guide](GUIDE.md).**

## Download

Get it from the **[latest release](https://github.com/Abdullahalmutairi97/Penguin-Notes/releases/latest)**.

- **Windows 10/11:** run **PenguinNotes-Setup.exe**. If Windows says it "protected your PC", click **More info > Run anyway** (the app isn't code-signed yet).
- **Linux** (64-bit, glibc 2.35+: Ubuntu 22.04+, Debian 12+, Fedora, Mint 21+, Arch, ...):

  ```
  curl -fsSL https://github.com/Abdullahalmutairi97/Penguin-Notes/releases/latest/download/install.sh | sh
  ```

  It installs for your user only (no sudo). Remove it with `sh ~/.local/share/penguin-notes-app/install.sh --uninstall`; your notes stay.

## Setting things up

Everything is set up inside the app; nothing else needs installing.

- **Pengu:** *Pengu > Settings*: choose Ollama or LM Studio (runs on your computer), or a cloud model with your API key.
- **Talk to Pengu:** click *Talk*. The first time it downloads the speech model (190 MB, once); after that, speech is turned into text on your computer and never leaves it.
- **iPhone Calendar:** *More > iPhone sync*: your Apple ID email and an [app-specific password](https://account.apple.com/account/manage).
- **iPhone Notes app:** in the same window pick **Gmail**, **Yahoo** or **Fastmail**, enter your email and an app password for it, then on the iPhone turn on *Notes* for that mail account (Settings > Apps > Mail > Mail Accounts). Your notes then appear in the Notes app under that account.
  Any other mail service with IMAP works too ("Other").

## Building from source

```
cargo build --release
```

Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed.
