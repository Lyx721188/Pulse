//! Two-language string table (English and Simplified Chinese), read at
//! display time so text follows the language setting rather than freezing at
//! whichever language was current when the reading was taken.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
}

static LANGUAGE: AtomicU8 = AtomicU8::new(0);

pub fn set_language(language: Language) {
    LANGUAGE.store(match language {
        Language::English => 0,
        Language::Chinese => 1,
    }, Ordering::Relaxed);
}

pub fn current() -> Language {
    if LANGUAGE.load(Ordering::Relaxed) == 1 {
        Language::Chinese
    } else {
        Language::English
    }
}

/// Detect from the Windows UI language, before settings are read.
pub fn detect_from_system() {
    let zh = std::env::var("SYSTEM_LANGUAGE").is_ok_and(|v| v.to_lowercase().starts_with("zh"))
        || std::env::var("LANG").is_ok_and(|v| v.to_lowercase().starts_with("zh"));
    set_language(if zh { Language::Chinese } else { Language::English });
}

struct Entry {
    key: &'static str,
    en: &'static str,
    zh: &'static str,
}

const TABLE: &[Entry] = &[
    Entry { key: "Loading…", en: "Loading…", zh: "读取中…" },
    Entry { key: "notConnected", en: "Connect Claude Code in Settings to see usage.", zh: "在设置中连接 Claude Code 以查看用量。" },
    Entry { key: "awaitingResponse", en: "Waiting for the next Claude Code response.", zh: "等待 Claude Code 的下一次响应。" },
    Entry { key: "No limits reported.", en: "No limits reported.", zh: "没有报告任何限额。" },
    Entry { key: "signInRequired", en: "Sign in to Codex to see usage.", zh: "登录 Codex 以查看用量。" },
    Entry { key: "claudeSignInRequired", en: "Sign in to Claude Code to see usage.", zh: "登录 Claude Code 以查看用量。" },
    Entry { key: "claudeLoginExpired", en: "Claude Code's saved login expired. Use Claude Code, or connect the status line.", zh: "Claude Code 保存的登录已过期。使用一次 Claude Code，或连接状态栏。" },
    Entry { key: "Codex isn't installed.", en: "Codex isn't installed.", zh: "尚未安装 Codex。" },
    Entry { key: "codexServerFailed", en: "Couldn't start the Codex helper.", zh: "无法启动 Codex 助手。" },
    Entry { key: "grokSignInRequired", en: "Sign in to Grok to see usage.", zh: "登录 Grok 以查看用量。" },
    Entry { key: "grokLoginExpired", en: "Grok's saved login expired. Use Grok to renew it.", zh: "Grok 保存的登录已过期。使用一次 Grok 以续期。" },
    Entry { key: "signedOut", en: "Sign in to this account again in Settings.", zh: "请在设置中重新登录此账号。" },
    Entry { key: "notSignedIn", en: "Sign in from Settings to see usage.", zh: "请在设置中登录以查看用量。" },
    Entry { key: "Add an API key in Settings.", en: "Add an API key in Settings.", zh: "请在设置中添加 API 密钥。" },
    Entry { key: "That key was refused. Check it in Settings.", en: "That key was refused. Check it in Settings.", zh: "该密钥被拒绝，请在设置中检查。" },
    Entry { key: "The service didn't respond.", en: "The service didn't respond.", zh: "服务没有响应。" },
    Entry { key: "Couldn't read the reply.", en: "Couldn't read the reply.", zh: "无法读取响应。" },
    Entry { key: "Checking too often — easing off.", en: "Checking too often — easing off.", zh: "检查过于频繁，已自动放缓。" },
    Entry { key: "The service returned an error.", en: "The service returned an error.", zh: "服务返回了错误。" },
    Entry { key: "notOnWindows", en: "This provider's route hasn't been ported to Windows yet.", zh: "该服务商的数据路由尚未移植到 Windows。" },
    Entry { key: "zaiNoCodingPlan", en: "That key works. The account has no Coding Plan running on it.", zh: "密钥有效，但该账号没有正在生效的编码套餐。" },
    Entry { key: "5-hour limit", en: "5-hour limit", zh: "5 小时限额" },
    Entry { key: "Weekly limit", en: "Weekly limit", zh: "每周限额" },
    Entry { key: "Spend limit", en: "Spend limit", zh: "消费限额" },
    Entry { key: "Balance", en: "Balance", zh: "余额" },
    Entry { key: "Monthly limit", en: "Monthly limit", zh: "每月限额" },
    Entry { key: "{n}-day limit", en: "{n}-day limit", zh: "{n} 天限额" },
    Entry { key: "{n}-hour limit", en: "{n}-hour limit", zh: "{n} 小时限额" },
    Entry { key: "estimated", en: "estimated", zh: "估算" },
    Entry { key: "since top-up", en: "since top-up", zh: "按充值额" },
    Entry { key: "of your budget", en: "of your budget", zh: "按你的预算" },
    Entry { key: "{n} days", en: "{n} days", zh: "{n} 天" },
    Entry { key: "{n} hours", en: "{n} hours", zh: "{n} 小时" },
    Entry { key: "under 15 minutes", en: "under 15 minutes", zh: "不到 15 分钟" },
    Entry { key: "about an hour", en: "about an hour", zh: "约 1 小时" },
    Entry { key: "about {n} minutes", en: "about {n} minutes", zh: "约 {n} 分钟" },
    Entry { key: "about {n} hours", en: "about {n} hours", zh: "约 {n} 小时" },
    Entry { key: "Credit balance", en: "Credit balance", zh: "余额" },
    Entry { key: "Resets {time}", en: "Resets {time}", zh: "{time} 重置" },
    Entry { key: "As of {time}", en: "As of {time}", zh: "截至 {time}" },
    Entry { key: "Reading may be out of date", en: "Reading may be out of date", zh: "读数可能已过期" },
    Entry { key: "{p} Used", en: "{p} Used", zh: "已用 {p}" },
    Entry { key: "{p} Left", en: "{p} Left", zh: "剩余 {p}" },
    Entry { key: "Runs out in {t}", en: "Runs out in {t}", zh: "预计 {t} 用尽" },
    Entry { key: "Won't last the window", en: "Won't last the window", zh: "撑不到窗口结束" },
    Entry { key: "Expected to last the window", en: "Expected to last the window", zh: "预计能撑到窗口结束" },
    Entry { key: "{n} points", en: "{n} points", zh: "{n} 积分" },
    Entry { key: "No reading", en: "No reading", zh: "暂无读数" },
    Entry { key: "Refreshing…", en: "Refreshing…", zh: "正在刷新…" },
    Entry { key: "{t} left, {w}", en: "{t} left, {w}", zh: "剩 {t}，{w}" },
    Entry { key: "{t} used, {w}", en: "{t} used, {w}", zh: "已用 {t}，{w}" },
    // Settings and shell strings.
    Entry { key: "Settings", en: "Settings", zh: "设置" },
    Entry { key: "General", en: "General", zh: "常规" },
    Entry { key: "Accounts", en: "Accounts", zh: "账号" },
    Entry { key: "Notifications", en: "Notifications", zh: "通知" },
    Entry { key: "About", en: "About", zh: "关于" },
    Entry { key: "Launch at startup", en: "Launch at startup", zh: "开机自动启动" },
    Entry { key: "Auto-collapse when idle", en: "Auto-collapse when idle", zh: "空闲时自动收起" },
    Entry { key: "Show percent labels", en: "Show percent labels", zh: "显示百分比标签" },
    Entry { key: "Show what's left", en: "Show what's left", zh: "显示剩余量" },
    Entry { key: "Show window clock", en: "Show window clock", zh: "显示窗口时间弧" },
    Entry { key: "Show second ring", en: "Show second ring", zh: "显示第二圆环" },
    Entry { key: "Show forecast", en: "Show forecast", zh: "显示用量预测" },
    Entry { key: "Follow the active display", en: "Follow the active display", zh: "跟随活动显示器" },
    Entry { key: "Panel size", en: "Panel size", zh: "面板大小" },
    Entry { key: "Small", en: "Small", zh: "小" },
    Entry { key: "Standard", en: "Standard", zh: "标准" },
    Entry { key: "Large", en: "Large", zh: "大" },
    Entry { key: "Ring spacing", en: "Ring spacing", zh: "圆环间距" },
    Entry { key: "Tight", en: "Tight", zh: "紧凑" },
    Entry { key: "Loose", en: "Loose", zh: "宽松" },
    Entry { key: "Refresh interval", en: "Refresh interval", zh: "刷新间隔" },
    Entry { key: "Automatic", en: "Automatic", zh: "自动" },
    Entry { key: "{n} seconds", en: "{n} seconds", zh: "{n} 秒" },
    Entry { key: "{n} minutes", en: "{n} minutes", zh: "{n} 分钟" },
    Entry { key: "1 minute", en: "1 minute", zh: "1 分钟" },
    Entry { key: "Warn me when a limit passes", en: "Warn me when a limit passes", zh: "当限额超过时提醒我" },
    Entry { key: "when a limit is spent", en: "when a limit is spent", zh: "当限额耗尽时" },
    Entry { key: "when a warned window comes back", en: "when a warned window comes back", zh: "被提醒过的窗口恢复时" },
    Entry { key: "when checks keep failing", en: "when checks keep failing", zh: "当检查连续失败时" },
    Entry { key: "when a balance falls under", en: "when a balance falls under", zh: "当余额低于" },
    Entry { key: "All notifications are off until you turn them on.", en: "All notifications are off until you turn them on.", zh: "所有通知默认关闭，需要你手动开启。" },
    Entry { key: "API key", en: "API key", zh: "API 密钥" },
    Entry { key: "Save", en: "Save", zh: "保存" },
    Entry { key: "Refresh now", en: "Refresh now", zh: "立即刷新" },
    Entry { key: "Copied", en: "Copied", zh: "已复制" },
    Entry { key: "Copy", en: "Copy", zh: "复制" },
    Entry { key: "Retry", en: "Retry", zh: "重试" },
    Entry { key: "Copy JSON report", en: "Copy JSON report", zh: "复制 JSON 报告" },
    Entry { key: "Open settings", en: "Open settings", zh: "打开设置" },
    Entry { key: "Refresh all", en: "Refresh all", zh: "全部刷新" },
    Entry { key: "Show panel", en: "Show panel", zh: "显示面板" },
    Entry { key: "Exit", en: "Exit", zh: "退出" },
    Entry { key: "Language", en: "Language", zh: "语言" },
    Entry { key: "English", en: "English", zh: "English" },
    Entry { key: "简体中文", en: "简体中文", zh: "简体中文" },
    Entry { key: "A limit passed {p}", en: "A limit passed {p}", zh: "某个限额已超过 {p}" },
    Entry { key: "A limit is spent", en: "A limit is spent", zh: "某个限额已耗尽" },
    Entry { key: "A limit is close", en: "A limit is close", zh: "某个限额即将用尽" },
    Entry { key: "{w} passed {p}", en: "{w} passed {p}", zh: "{w} 已超过 {p}" },
    Entry { key: "A warned window came back", en: "A warned window came back", zh: "被提醒过的窗口已恢复" },
    Entry { key: "Checks keep failing", en: "Checks keep failing", zh: "检查连续失败" },
    Entry { key: "Low balance", en: "Low balance", zh: "余额偏低" },
    Entry { key: "Not on Windows yet", en: "Not on Windows yet", zh: "尚未支持 Windows" },
    Entry { key: "Enabled", en: "Enabled", zh: "已启用" },
    Entry { key: "Route", en: "Route", zh: "数据来源" },
    Entry { key: "Usage endpoint", en: "Usage endpoint", zh: "用量接口" },
    Entry { key: "Reads the login this tool already saved on this PC.", en: "Reads the login this tool already saved on this PC.", zh: "读取该工具已在本机保存的登录。" },
    Entry { key: "Pin the limit the ring shows", en: "Pin the limit the ring shows", zh: "固定圆环显示的限额" },
    Entry { key: "Ring colour", en: "Ring colour", zh: "圆环颜色" },
    Entry { key: "By usage", en: "By usage", zh: "按用量" },
    Entry { key: "Version", en: "Version", zh: "版本" },
    Entry { key: "A screen-edge monitor for your AI coding allowances.", en: "A screen-edge monitor for your AI coding allowances.", zh: "停靠在屏幕边缘的 AI 编码额度监视器。" },
    Entry { key: "Diagnostics", en: "Diagnostics", zh: "诊断" },
    Entry { key: "Copy diagnostic report", en: "Copy diagnostic report", zh: "复制诊断报告" },
    Entry { key: "Last checked {time}", en: "Last checked {time}", zh: "上次检查 {time}" },
    Entry { key: "never", en: "never", zh: "从未" },
    Entry { key: "just now", en: "just now", zh: "刚刚" },
    Entry { key: "{n}m ago", en: "{n}m ago", zh: "{n} 分钟前" },
    Entry { key: "{n}h ago", en: "{n}h ago", zh: "{n} 小时前" },
    Entry { key: "{n}d ago", en: "{n}d ago", zh: "{n} 天前" },
];

/// The localized string for a key. Unmatched keys return the key itself, the
/// same way a missing translation falls through to English.
pub fn t(key: &str) -> &'static str {
    let zh = current() == Language::Chinese;
    for entry in TABLE {
        if entry.key == key {
            return if zh { entry.zh } else { entry.en };
        }
    }
    // A miss is a bug in this file, but falling through to the key keeps
    // new call sites visible instead of blank. The keys are code literals,
    // so the leak is a finite, tiny set.
    Box::leak(key.to_string().into_boxed_str())
}

/// Interpolates `{n}`-style placeholders. Values are passed as strings so an
/// integer can never silently produce the wrong formatter.
pub fn t_fmt(key: &str, values: &[&str]) -> String {
    let mut out = t(key).to_string();
    for value in values {
        // The table uses "{n}"; later values would be "{0}"-shaped and are
        // matched in order against the first unfilled placeholder.
        if out.contains("{n}") {
            out = out.replacen("{n}", value, 1);
        } else {
            out.push(' ');
            out.push_str(value);
        }
    }
    out
}

pub fn provider_raw(provider: crate::model::Provider) -> &'static str {
    use crate::model::Provider::*;
    match provider {
        ClaudeCode => "claudeCode",
        Codex => "codex",
        Antigravity => "antigravity",
        Cursor => "cursor",
        OpenCodeGo => "openCodeGo",
        KimiCode => "kimiCode",
        OllamaCloud => "ollamaCloud",
        Zai => "zai",
        GlmCoding => "glmCoding",
        Minimax => "minimax",
        MinimaxCn => "minimaxCN",
        Copilot => "copilot",
        Grok => "grok",
        GrokBot => "grokBot",
        Volcengine => "volcengine",
        CommandCode => "commandCode",
        DeepSeek => "deepSeek",
    }
}
