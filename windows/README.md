# Pulse for Windows

A native Windows application for [Pulse](../README.md) — the screen-edge monitor
for your AI coding allowances — written in Rust against the Win32 / Direct2D
APIs, styled after WinUI. A Mica dock floats beside a screen edge; each
accent-coloured ring is a limit (the providers' own Lobe icons in the
middle), hover for the detail card, and everything refreshes on an adaptive
ladder so your status line and the panel agree.

The reading, caching, alerting and reporting logic lives in `pulse-core` and
follows the central promise that **Pulse does not invent percentages**. Where a provider says how much of an
allowance is left but never how large it is, the denominator is either
inferred (and labelled `estimated`) or the ring is not drawn at all.

## Build

Any machine with a Rust toolchain can type-check; the release build runs on
GitHub Actions ([`.github/workflows/windows.yml`](../.github/workflows/windows.yml))
and uploads `pulse.exe` as an artifact on every push.

```bash
cd windows
cargo build --release        # target/release/pulse.exe
cargo test                   # core and application tests
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

Reading logic, window selection, caching and reporting are implemented for 14
of the 17 providers; the ring, card, berth and settings UI are complete. The
three gaps below depend on mechanisms that are not implemented yet, and are
listed in Settings with the reason rather than shown as broken.

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
| Antigravity | Language server while the editor is open (process table, PEB command line, owned-TCP ports) | ✅ Ported |
| Cursor | Cursor's own login database (read-only SQLite) + usage summary API | ✅ Ported |
| Ollama Cloud | Browser session cookie | ⬜ Not ported |
| Grok Bot (in Cursor) | Cursor's saved login | ⬜ Not ported |
| Volcengine | Signed usage API (HMAC access keys) | ⬜ Not ported |

## Architecture

Two crates:

- **`pulse-core`** — everything without a window: the provider model, the
  13 service implementations, DPAPI-encrypted key storage (the Keychain's
  counterpart here), the reading cache with its reconcile rules, adaptive
  refresh pacing, alerts, localization (English + 简体中文) and the `--json`
  report. Compiles anywhere.
- **`pulse-win`** — the Win32 surface. The dock and the detail flyout are
  two `WS_EX_NOREDIRECTIONBITMAP` windows whose frames are composed by DWM:
  **real Mica** (`DWMWA_SYSTEMBACKDROP_TYPE`), Windows' own corner radius and
  border, dark/light following the system. Rendering is a flip-model
  `CreateSwapChainForComposition` swap chain handed to DWM through a
  DirectComposition visual — the only route where the premultiplied alpha
  **and** the system backdrop both survive — drawn with Direct2D. The
  settings window is WinUI 3 via Microsoft's [Windows
  Reactor](https://github.com/microsoft/windows-rs) (a
  `windows-reactor` component served on the main thread), while the panel,
  tray and notifications are classic Win32 on a worker thread. Provider
  marks are Lobe-derived assets, rasterised once to transparent PNGs and
  tinted at load into the theme's ink. Motion uses damped springs:
  the arrival, the hover halo, the arcs and the card's slide between rings
  are all damped springs — SwiftUI's `.spring(response:…)`,
  `dampingFraction:…)` integrated in fixed steps. Windows only.

Design tokens follow WinUI (`theme.rs`): the ink sits directly on Mica, in
one dark and one light voice matched to the system appearance; ring arcs,
progress bars and the hover glow use the **system accent colour**. The theme
and accent are re-read whenever Windows announces a change, and the icon
cache re-tints with them. Credentials are stored with `CryptProtectData`
under the current user, so they do not survive `roaming` to another machine
— by design.

## Keeping the port honest

The tests in `pulse-core/tests/ported.rs` cover the percent display rule
(nothing used reads 0%, not quite full
never reads 100%, and a countdown gets the same rule at both ends), which
limit the second ring picks (fullest of the rest, inside the reading's own
scope group), the GLM Coding Plan refusals (every one of them an HTTP 200,
answered in Chinese and English), and DeepSeek's balance-only windows (no
length, no reset, ever — balance is not a limit).
