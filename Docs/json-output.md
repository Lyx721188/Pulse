# `pulse.exe --json`

这是状态栏、终端提示符和脚本使用的 JSON 契约。命令只读取本地缓存，不发起网络
请求，也不写入凭据或设置。

```powershell
.\pulse.exe --json
```

在源码中，报告模型位于
[`windows/pulse-core/src/report.rs`](../windows/pulse-core/src/report.rs)，命令行入口
位于 [`windows/pulse-win/src/main.rs`](../windows/pulse-win/src/main.rs)。

## 输出结构

```text
generatedAt            ISO 8601 时间
accounts[]
  id                   账号标识
  provider             服务商标识
  name                 服务商名称
  label                用户为账号设置的名称
  plan                 服务商报告的套餐（如果有）
  creditBalance        服务商报告的余额（如果有）
  observedAt           本次读数时间（如果有）
  ageSeconds           generatedAt - observedAt
  source               读数实际来源
  settingsURL          设置页或账号链接（如果有）
  headline{}           当前最值得展示的限额
  windows[]            全部限额窗口
    id
    kind               fiveHour | weekly | spend | monthly | balance | other:<seconds>
    scope
    usedPercent        展示用百分比
    usedFraction       未四舍五入的原始比例
    exhausted          服务商是否明确报告已耗尽
    windowSeconds      窗口长度（如果服务商报告）
    reportsLength      是否确实报告了窗口长度
    estimated          分母是否为推算值
    estimatedFrom      推算来源（如果有）
    resetsAt           重置时间（如果有）
```

`headline` 重复 `windows` 中的一个窗口，方便常见状态栏直接显示。消费者应把未知
字段当作可忽略字段，并允许未来增加新的 `kind` 或 `source`。

`usedPercent` 遵循 UI 的展示规则：只要服务商报告有消耗，就不会显示为 0%；尚未
完全耗尽时也不会被错误显示为 100%。`estimated` 为 `true` 时，服务商只报告了
剩余量或余额，应用没有把推算值伪装成服务商报告的精确比例。

## 示例

```powershell
.\pulse.exe --json | ConvertFrom-Json
```

脚本应根据 `observedAt` 或 `ageSeconds` 判断数据是否过期，不应把没有读数的账号
当作 0% 使用，也不应把 `balance` 当作会自动重置的限额窗口。
