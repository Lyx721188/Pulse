<p align="center">
  <img src="AppIcon/pulse-icon-1024.png" width="112" alt="Pulse">
</p>

<h1 align="center">Pulse</h1>

<p align="center">
  <b>优雅无扰的屏幕边缘 AI 编码额度监视器 —— macOS 原版 + Windows 原生版。</b><br>
  实时掌握 Claude Code、Codex、Cursor、GitHub Copilot、Antigravity、Grok 等多平台的限额与剩余用量。
</p>

<p align="center">
  <a href="https://github.com/Lyx721188/Pulse/actions/workflows/windows.yml"><img src="https://github.com/Lyx721188/Pulse/actions/workflows/windows.yml/badge.svg" alt="Windows 构建"></a>
  <img src="https://img.shields.io/badge/Windows-10%201809%2B%20%2F%2011-0078D4?logo=windows&logoColor=white" alt="Windows 10/11">
  <img src="https://img.shields.io/badge/macOS-14.0%2B%20Sonoma-333333?logo=apple" alt="macOS 14+">
  <img src="https://img.shields.io/badge/Rust-%E7%A8%B3%E5%AE%9A%E7%89%88-DEA584?logo=rust&logoColor=white" alt="Rust">
  <a href="LICENSE"><img src="https://img.shields.io/badge/许可-Apache%202.0-blue" alt="开源许可"></a>
</p>

Pulse 是一个停靠在屏幕边缘的小巧悬浮监视器。它展示各服务自己上报的剩余额度——走的是该产品自己的客户端通道，而不是 Pulse 的服务器——无 Pulse 账号、无遥测。Pulse 不会自行编造用量百分比：服务商只报剩余、不报总量时，分母要么推算（并标注**估算**），要么干脆不画圆环。

本仓库在原版 [qunqin24/Pulse](https://github.com/qunqin24/Pulse)（macOS，Swift）的基础上，于 [`windows/`](windows/) 目录提供了一个用 Rust 编写的 **Windows 原生移植**：读取、缓存、通知与报告逻辑逐条对齐原版，界面则完全按 Windows 的方式重做。

---

## 两个版本

| | macOS 原版 | Windows 移植 |
| --- | --- | --- |
| 代码 | `Sources/`（Swift 6 + SwiftUI） | [`windows/`](windows/)（Rust + Win32/Direct2D + WinUI 3） |
| 外观 | 纯黑底板；macOS 26+ 可选原生 Liquid Glass | 真 **Mica**、DWM 圆角与边框、深浅色跟随系统、系统强调色 |
| 结构 | 悬浮胶囊 + 悬停详情卡 | 屏幕边缘停靠栏 + 悬停详情卡 + 托盘图标 + WinUI 3 设置窗口 |
| 凭据存储 | Keychain | DPAPI（`CryptProtectData`，仅当前系统用户可解） |

渲染不走网页、不走图层快照：两个 `WS_EX_NOREDIRECTIONBITMAP` 窗口的画面由翻转模型交换链经 DirectComposition 交给 DWM 合成，圆环、光晕和卡片滑动的动画都是阻尼弹簧。Provider 图标沿用原版的 Lobe SVG 图标集，加载时按主题重新着色。

核心体验两个版本一致：圆环随使用率变色（绿 → 琥珀 → 红 → 用尽深红）、悬停卡片列出每条限额与重置倒计时、消耗速率与耗尽预测、副圆环显示当前窗口时间流逝、闲置自动收起成细条、限额越过 75/80/90/95% 时系统通知（每件事只说一次）、以及面向状态栏和脚本的 `pulse --json` 输出（只读缓存、永不发请求）。

---

## 支持的服务商

Pulse 仅呈现各服务上报的数字，绝不靠本地 Token 粗略估算。各产品的读取通道不同（已文档化的客户端接口、编辑器登录态、本地 language server、粘贴的密钥），并不是每一行都有公开的官方配额 API。

| 服务商 | 读取通道与鉴权方式 | Windows 版 |
|---|---|---|
| **Claude Code** | 账号 OAuth 用量接口；自动回退至 Claude 桌面端 Web 会话 | ✅ 已移植 |
| **Codex** | 客户端用量接口；回退至 `codex app-server` | ✅ 已移植 |
| **Antigravity** | 编辑器本地运行的 Language Server | ✅ 已移植（进程表 + PEB 命令行 + 自有 TCP 端口） |
| **Cursor** | Cursor 账号用量摘要接口 | ✅ 已移植（只读本机登录数据库） |
| **Grok** | Grok Build CLI 代理接口；与网页/CLI/API 共享统一周额度池 | ✅ 已移植 |
| **Grok Bot** | Cursor 仪表盘接口（Cursor 套餐内的 xAI 额度） | ⬜ 暂缺 |
| **GitHub Copilot** | GitHub 设备码登录，仅申请 `read:user`；Premium requests API | ✅ 已移植 |
| **OpenCode Go** | 设置中填入 API Key，或读取 OpenCode CLI 登录信息 | ✅ 已移植 |
| **Kimi Code** | 设置中填入 API Key | ✅ 已移植 |
| **z.ai** | 设置中填入 API Key（国际站） | ✅ 已移植 |
| **Zhipu（智谱）** | 设置中填入 API Key，或读取本地 GLM 工具密钥（国内站） | ✅ 已移植 |
| **MiniMax / MiniMax CN** | 设置中填入 API Key（国际站与国内站） | ✅ 已移植 |
| **Ollama Cloud** | 本地读取浏览器会话 Cookie（官方无配额 API） | ⬜ 暂缺 |
| **Volcengine（火山引擎）** | `arkcli` 登录，或粘贴 Access Key 对（HMAC 签名） | ⬜ 暂缺 |
| **Command Code** | 设置中填入 API Key，或读取 `cmd auth login` 登录 | ✅ 已移植 |
| **DeepSeek** | API Key；官方 `GET /user/balance`，仅报预付余额 | ✅ 已移植 |

Windows 版已移植 17 个服务商中 14 个的读取逻辑（MiniMax 国际/国内算两个）。暂缺的三条通道依赖尚未覆盖的机制（浏览器 Cookie、HMAC 签名器）；它们在设置页里如实说明原因，而不是显示为故障。移植状态与架构细节见 [windows/README.md](windows/README.md)。

---

## 下载与安装

### Windows

每次推送都由 GitHub Actions 自动构建并出包（[Windows build](https://github.com/Lyx721188/Pulse/actions/workflows/windows.yml)）；推送 `windows-v*` 标签时自动发布到 [Releases](https://github.com/Lyx721188/Pulse/releases)。

1. 从 Releases（或 Actions 运行页的 `pulse-windows-x64` 制品）下载 zip 并解压；
2. 运行 `pulse.exe`，停靠栏出现在屏幕右缘，悬停任意圆环查看详情；
3. 右键托盘图标打开设置，启用并登录你的账户。

### macOS

前往[原版仓库的 Releases](https://github.com/qunqin24/Pulse/releases/latest) 下载 `Pulse-x.y.z.dmg`，拖入「应用程序」即可。Pulse 未参与 Apple 公证，首次启动若被拦截：打开**系统设置 → 隐私与安全性**，点击「仍要打开」；或在终端执行 `xattr -cr /Applications/Pulse.app`。

---

## 从源码构建

Windows（Rust stable，MSVC 工具链）：

```bash
git clone https://github.com/Lyx721188/Pulse.git
cd Pulse/windows
cargo build --release        # target/release/pulse.exe
cargo test                   # 移植自 macOS 套件的规则测试
```

`pulse.exe` 直接运行即是完整应用；`pulse.exe --json` 打印最近一次读数（套餐、每条限额、重置时间、数据新旧），只读缓存不发请求，可接 tmux、终端提示符或任意脚本。

macOS（Xcode；编译 Liquid Glass 界面需要 macOS 26 SDK）：

```bash
git clone https://github.com/qunqin24/Pulse.git
cd Pulse
swift run Pulse              # 直接编译运行
./Scripts/bundle.sh          # 打包为标准 App Bundle
```

---

## 隐私与安全性

- **无 Pulse 后端**：没有 Pulse 服务器、账号或遥测。应用直接请求你已在使用的服务商，不插入自有代理；系统代理设置仍然生效。
- **凭据来源**：在产品本身如此工作时，复用本地开发工具已有的登录态（`~/.claude`、`~/.codex`、Cursor 本地数据库、Antigravity 的 language server 等）；部分服务需要在设置中填写密钥。
- **本地加密存储**：手动输入的 API Key 和会话凭据经 Keychain（macOS）或 DPAPI（Windows）加密保存，仅限当前系统用户解密，不会随漫游账户漂移到别的机器。
- **代码与对话**：Pulse 绝不读取或上传你的源码、终端上下文、Prompt 或模型生成内容。

---

## 设计来源与致谢

Pulse 的灵感来自 [**Vinz**(@hivinz_)](https://x.com/hivinz_/status/2092996055248126353) 2026 年 8 月在 X 上分享的一个 UI 概念，是独立的实现。macOS 原版与全部数据读取规则来自 [qunqin24/Pulse](https://github.com/qunqin24/Pulse)；Windows 移植站在它的肩膀上。

---

## 开源许可

本项目遵循 [Apache 2.0 开源许可协议](LICENSE)。原版 Pulse 以 Apache 2.0 发布，本仓库的 Windows 移植（`windows/`）同样以 Apache 2.0 发布。附带的第三方图标与依赖遵循其各自许可，详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。各服务商名称与商标归其所有者所有，仅用于标识兼容的服务，不构成背书。
