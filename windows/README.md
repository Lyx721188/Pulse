# Pulse for Windows

A native Windows port of [Pulse](../README.md) — the screen-edge monitor for
your AI coding allowances — written in Rust against the Win32 / Direct2D
APIs, styled after WinUI's dark theme. One window sits at a screen edge; each
ring is a limit, hover for the detail card, and everything refreshes on an
adaptive ladder so your status line and the panel agree.

This is a **port, not a reimplementation of the data rules**: the reading,
caching, alerting and reporting logic follows the Swift original in
[`Sources/Pulse`](../Sources/Pulse) — including its central promise that
**Pulse does not invent percentages**. Where a provider says how much of an
allowance is left but never how large it is, the denominator is either
inferred (and labelled `estimated`) or the ring is not drawn at all.

## Build

Any machine with a Rust toolchain can type-check; the release build runs on
GitHub Actions ([`.github/workflows/windows.yml`](../.github/workflows/windows.yml))
and uploads `pulse.exe` as an artifact on every push.

```bash
cd windows
cargo build --release        # target/release/pulse.exe
cargo test                   # 41 tests ported from the macOS suite
```

## Run

```bash
pulse.exe            # the panel, the tray icon, the settings window
pulse.exe --json     # print the cached rail for status lines and scripts
```

### `--json`

The status-line contract from
[`Docs/json-output.md`](../Docs/json-output.md) is kept verbatim: it prints
what the running app last banked and how old it is, **never fetches**, and
never writes. Window names are not localized in it; `kind` is a flat token
(`fiveHour`, `weekly`, `spend`, `monthly`, `balance`, `other:<seconds>`) and
`usedPercent` carries the display rule, so a script agrees with the ring.

```bash
pulse.exe --json | jq -r '.accounts[] | "\(.name) \(.headline.usedPercent // "–")%"'
```

## What is ported

Reading logic, window selection, caching and reporting are ported for 12 of
the 17 providers; the ring, card, berth and settings UI are complete. The
five gaps below are routes the macOS app reaches through macOS-only
machinery, listed in Settings with the reason rather than shown as broken.

| Provider | Route | Status |
| --- | --- | --- |
| Claude Code | Usage endpoint + OAuth token | ✅ Ported |
| Codex | Usage endpoint + ChatGPT account | ✅ Ported |
| Copilot | Premium requests API | ✅ Ported |
| Grok | Credits API + `x-xai-token-auth` | ✅ Ported |
| OpenCode | Zen API + stored key | ✅ Ported |
| Kimi (Command) | Kimi API + stored key | ✅ Ported |
| z.ai | GLM Coding Plan quota | ✅ Ported |
| Zhipu (GLM) | GLM Coding Plan quota, mainland host | ✅ Ported |
| MiniMax | Coding Plan API, overseas + mainland | ✅ Ported |
| Command Code | Subscriptions + credits + plan table | ✅ Ported |
| DeepSeek | Balance + basis (since top-up / your budget) | ✅ Ported |
| Antigravity | Language server while the editor is open | ⬜ Not ported |
| Cursor | Cursor's own login database | ⬜ Not ported |
| Ollama Cloud | Browser session cookie | ⬜ Not ported |
| Grok Bot (in Cursor) | Cursor's saved login | ⬜ Not ported |
| Volcengine | Signed usage API (HMAC access keys) | ⬜ Not ported |

## Architecture

Two crates:

- **`pulse-core`** — everything without a window: the provider model, the
  11 service implementations, DPAPI-encrypted key storage (the Keychain's
  counterpart here), the reading cache with its reconcile rules, adaptive
  refresh pacing, alerts, localization (English + 简体中文) and the `--json`
  report. Compiles anywhere.
- **`pulse-win`** — the Win32 surface: a layered window drawn with Direct2D
  over a GDI memory DC (per-pixel alpha for the berth and the hover card),
  the Fluent-styled settings window with a Mica backdrop and custom caption
  buttons, the tray icon, notifications, single-instance enforcement and the
  GitHub device sign-in flow. Windows only.

Design tokens follow WinUI's dark theme (`theme.rs`): the panel keeps the
macOS app's obsidian surface; text faces, corner radii, control fills and
hover states use the Fluent palette. Credentials are stored with
`CryptProtectData` under the current user, so they do not survive
`roaming` to another machine — by design.

## Keeping the port honest

The tests in `pulse-core/tests/ported.rs` are the macOS suite's rules,
restated: the percent display rule (nothing used reads 0%, not quite full
never reads 100%, and a countdown gets the same rule at both ends), which
limit the second ring picks (fullest of the rest, inside the reading's own
scope group), the GLM Coding Plan refusals (every one of them an HTTP 200,
answered in Chinese and English), and DeepSeek's balance-only windows (no
length, no reset, ever — balance is not a limit).
