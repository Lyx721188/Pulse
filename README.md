# Pulse for Windows

Pulse 是一个原生 Windows 屏幕边缘 AI 编码额度监视器，用 Rust 编写，运行在
Win32、Direct2D 和 WinUI 3 之上。它直接读取各服务商自己的客户端通道，展示
剩余额度、重置倒计时、消耗速率和耗尽预测；无 Pulse 后端、无 Pulse 账号、无遥测。

[![Windows 构建](https://github.com/Lyx721188/Pulse/actions/workflows/windows.yml/badge.svg)](https://github.com/Lyx721188/Pulse/actions/workflows/windows.yml)
![Windows 10/11](https://img.shields.io/badge/Windows-10%201809%2B%20%2F%2011-0078D4?logo=windows&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust&logoColor=white)
[![许可证](https://img.shields.io/badge/许可证-Apache%202.0-blue)](LICENSE)

## 特性

- 屏幕边缘停靠栏、悬停详情卡、托盘图标和 WinUI 3 设置窗口
- 真 Mica、DWM 圆角与边框、深浅色跟随系统、系统强调色
- 自适应刷新、缓存、通知和 `pulse.exe --json` 状态输出
- API Key 和会话凭据使用 Windows DPAPI 加密，仅当前系统用户可解密
- Provider 图标和本地化资源随 Windows 应用一起发布

## 支持的服务商

当前已移植读取逻辑的服务商包括 Claude Code、Codex、Antigravity、Cursor、Grok、
GitHub Copilot、OpenCode Go、Kimi Code、z.ai、Zhipu、MiniMax、Command Code 和
DeepSeek。Ollama Cloud、Grok Bot、Volcengine 的部分读取通道仍在设置页中明确标注
为未支持，不会伪装成故障或编造用量。

各服务商的读取通道和鉴权方式见 [`Docs/providers`](Docs/providers/README.md)。

## 下载与构建

从 [Releases](https://github.com/Lyx721188/Pulse/releases) 下载 Windows x64 压缩包，
解压后运行 `pulse.exe`。每次推送都会由
[Windows build](https://github.com/Lyx721188/Pulse/actions/workflows/windows.yml)
构建；推送 `windows-v*` 标签时自动发布 Release。

本地构建需要 Rust stable 和 MSVC 工具链：

```bash
git clone https://github.com/Lyx721188/Pulse.git
cd Pulse/windows
cargo fmt --all -- --check
cargo test --workspace
cargo build --release
```

输出位于 `windows/target/release/pulse.exe`。`pulse.exe --json` 只读取缓存，不发起
网络请求；JSON 字段约定见 [`Docs/json-output.md`](Docs/json-output.md)。

## 隐私与安全性

- Pulse 不提供自有服务器、账号或遥测服务。
- 应用只请求你正在使用的服务商接口，或读取本机已登录工具的状态。
- Pulse 不读取或上传源码、终端上下文、Prompt 或模型生成内容。

## 许可

本项目遵循 [Apache 2.0](LICENSE) 开源许可。第三方图标和依赖的许可见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。服务商名称和商标归其所有者所有，
仅用于标识兼容服务，不构成背书。
