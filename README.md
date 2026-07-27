# capsule

A fast Apple Music desktop client for Windows. Rust core, native `.exe`, no
bundled Chromium.

Built because the official Apple Music app on Windows is slow. The speed here
comes from keeping the network off the UI path: library metadata is mirrored
into local SQLite, artwork is cached on disk, and lists are virtualised.

> **Status:** early. The playback architecture is proven end to end
> (see *How it works*), but this is not yet a usable player.

## Requirements

- Windows 10/11
- An **active Apple Music subscription** — without one MusicKit only returns
  30-second previews
- WebView2 **Evergreen** runtime (preinstalled on Windows 11). A *fixed-version*
  runtime will fail with an expired Widevine licence

## How it works

Two webviews and a Rust core that owns all state:

```
┌─ visible window ────────────┐   ┌─ hidden window ──────────────┐
│  React UI                   │   │  https://music.apple.com     │
│  never touches Apple        │   │  + injected engine hook      │
│  invoke() + event listeners │   │  MusicKit = audio engine     │
└────────────┬────────────────┘   └───────────┬──────────────────┘
             │ IPC                            │ IPC
             └──────────┬─────────────────────┘
                  ┌─────▼──────┐
                  │ Rust core  │ ← single source of truth
                  └────────────┘
```

The hidden window is Apple's own web player, invisible, reduced to an audio
daemon. Playback has to run there because MusicKit needs Widevine and its
developer token is bound to Apple's origin. The UI you see is entirely ours and
never talks to Apple directly.

Because Rust holds the only copy of playback state, the UI, the Windows media
flyout, Discord, and the scrobbler can't drift out of sync with each other.

## Honest caveats

- **Audio is 256kbps AAC. Lossless is not possible.** Measured, not assumed: the
  EME negotiation is `com.widevine.alpha` requesting `mp4a.40.2`, and
  `MusicKit.PlaybackBitrate` exposes only 64 and 256. Apple serves ALAC to
  first-party clients only. This binds every third-party client, not just this
  one. Local-file lossless playback is planned separately.
- **The default token path is not sanctioned by Apple.** By default the app
  reuses the developer token that `music.apple.com` ships in its own page. It
  works, but Apple can change their bundle at any time and break it. If you have
  an Apple Developer Program membership you can supply your own key instead —
  see `.env.example`.
- **Your Apple credentials never touch this app.** Sign-in happens on Apple's
  real login page inside the engine window. There is no login form here and no
  password is ever seen or stored. Harvested tokens live in Windows Credential
  Manager, never on disk in plaintext.

## Development

```bash
bun install
bun run app        # tauri dev
```

Useful while debugging:

```bash
SHOW_ENGINE_WINDOW=true bun run app   # un-hide the playback webview
```

Tests and checks:

```bash
cd src-tauri && cargo test && cargo clippy -- -D warnings
bun run typecheck
```

Copy `.env.example` to `.env` if you want Last.fm or Discord integration. The
app builds and runs fine without it — those features simply disable themselves.

## Licence

MIT. See [LICENSE](LICENSE).

Not affiliated with or endorsed by Apple. Apple Music is a trademark of Apple Inc.
