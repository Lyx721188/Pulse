# Third-Party Notices

Pulse bundles provider marks derived from [Lobe Icons](https://github.com/lobehub/lobe-icons):

- `windows/pulse-win/assets/icons/*.png`

Lobe Icons is distributed under the MIT License:

```text
MIT License

Copyright (c) 2023 LobeHub

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The Windows app ships copies of the Lobe-derived provider marks, rasterised for its own renderer
(`windows/pulse-win/assets/icons`). It is written in Rust against the
Microsoft Windows SDK via the [`windows`](https://github.com/microsoft/windows-rs)
crate, and links against further crates from crates.io — overwhelmingly
dual-licensed MIT OR Apache-2.0. The pinned list with versions is
`windows/Cargo.lock`, and each crate's repository carries its licence text;
the SQLite engine compiled into `rusqlite` (the `bundled` feature) is in the
public domain.

The Claude, OpenAI, Antigravity, Cursor, OpenCode, Kimi, Ollama, Z.ai, 智谱
and 清言, MiniMax, GitHub, Grok and xAI, and Volcengine names and marks remain
the property of their respective owners. Their inclusion identifies compatible
services and does not imply endorsement.
