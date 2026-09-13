# Contributing

Pulse 是一个 Windows 原生 Rust 应用。源码位于 `windows/`，工作区使用 Cargo 管理，
不再包含 macOS/Swift 实现。

## 开发流程

```bash
cd windows
cargo fmt --all -- --check
cargo clippy --workspace
cargo test --workspace
cargo build --release
```

Provider 的接口、鉴权和本地数据来源说明位于
[`Docs/providers`](Docs/providers/README.md)。修改服务商读取逻辑时，请同步更新对应
说明和 `windows/pulse-core/tests/` 中的覆盖。

提交前请确认：

- Rust 格式检查、Clippy 和测试通过；
- `pulse.exe --json` 的输出仍符合 [`Docs/json-output.md`](Docs/json-output.md)；
- 不提交凭据、账户信息、构建产物或 `target/` 目录；
- README 和用户可见行为与代码保持一致。

## 代码原则

- 不编造服务商没有报告的用量百分比；
- 本地凭据必须使用 Windows DPAPI 或现有安全存储接口；
- 网络失败、缓存过期和未配置账号要分别表达，不能互相伪装；
- 不引入 Pulse 后端、账号体系或遥测。
