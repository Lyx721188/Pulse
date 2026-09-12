//! Tests ported from the macOS test suite (`Tests/PulseTests`), covering the
//! rules this port has to keep exactly: the percent display rule, which
//! limits the two rings pick, the GLM Coding Plan refusals and quota reply,
//! and DeepSeek's balance-only denominators.

use pulse_core::model::{AccountKey, Kind, Provider, ProviderUsage, UsageWindow};
use pulse_core::providers::zai;
use pulse_core::providers::deepseek::{self, Purse};

fn window(used: f64) -> UsageWindow {
    window_with(used, "w", Kind::FiveHour, None)
}

fn window_with(used: f64, id: &str, kind: Kind, scope: Option<&str>) -> UsageWindow {
    UsageWindow::new(
        id,
        kind,
        scope.map(|s| s.to_string()),
        used,
        if matches!(kind, Kind::FiveHour) { 5 * 3_600 } else { 7 * 86_400 },
        None,
    )
}

// MARK: - Reported figures (UsageWindowTests)

#[test]
fn nothing_used_reads_zero_but_anything_used_reads_at_least_one_percent() {
    assert_eq!(window(0.0).percent_text(false), "0%");
    // Cursor reports 0.03% and its own page says 1%.
    assert_eq!(window(0.0003).percent_text(false), "1%");
    assert_eq!(window(0.004).percent_text(false), "1%");
}

#[test]
fn not_quite_full_never_reads_one_hundred_percent() {
    assert_eq!(window(0.996).percent_text(false), "99%");
    assert_eq!(window(1.0).percent_text(false), "100%");
    // A spend limit can sail past its own ceiling.
    assert_eq!(window(1.4).percent_text(false), "100%");
}

#[test]
fn counting_down_gets_the_same_rule_at_both_ends() {
    // 99.6% spent still has something left, so it must not read 0% left.
    assert_eq!(window(0.996).percent_text(true), "1%");
    // 0.4% spent is not everything left either.
    assert_eq!(window(0.004).percent_text(true), "99%");
    assert_eq!(window(0.0).percent_text(true), "100%");
    assert_eq!(window(1.0).percent_text(true), "0%");
}

#[test]
fn the_window_clock_needs_a_length_the_provider_actually_stated() {
    let now = pulse_core::timeutil::now_ms();
    // Halfway: the reset lands 2.5 hours out of a five-hour window.
    let halfway = UsageWindow::new("w", Kind::FiveHour, None, 0.5, 5 * 3_600, Some(now + 9_000 * 1_000));
    let elapsed = halfway.elapsed_fraction(now).expect("halfway elapsed");
    assert!((elapsed - 0.5).abs() < 0.01);

    // A sort key is also a positive number. Dividing by one draws an arc
    // nobody reported.
    let mut sort_key_only = UsageWindow::new("w", Kind::FiveHour, None, 0.5, 3_600, Some(now + 3_600 * 1_000));
    sort_key_only.reports_length = false;
    assert_eq!(sort_key_only.elapsed_fraction(now), None);

    // No reset time, nothing to measure against.
    assert_eq!(window(0.5).elapsed_fraction(now), None);
}

// MARK: - The second ring's limit (SecondWindowTests)

fn usage(windows: Vec<UsageWindow>) -> ProviderUsage {
    ProviderUsage::live_now(AccountKey::primary(Provider::ClaudeCode), windows)
}

#[test]
fn one_limit_draws_no_second_ring() {
    assert!(usage(vec![window_with(0.4, "only", Kind::Weekly, None)])
        .second_window(None)
        .is_none());
    // An empty second ring would read as a limit at zero, or as a fault.
    assert!(usage(vec![]).second_window(None).is_none());
}

#[test]
fn second_ring_is_the_fullest_of_the_rest_never_the_one_already_on_the_ring() {
    let u = usage(vec![
        window_with(0.82, "5h", Kind::FiveHour, None),
        window_with(0.34, "weekly", Kind::Weekly, None),
        window_with(0.61, "monthly", Kind::Monthly, None),
    ]);
    assert_eq!(u.headline_window(None).unwrap().id, "5h");
    assert_eq!(u.second_window(None).unwrap().id, "monthly");
}

#[test]
fn a_pinned_ring_moves_the_second_one_out_of_its_way() {
    let u = usage(vec![
        window_with(0.82, "5h", Kind::FiveHour, None),
        window_with(0.34, "weekly", Kind::Weekly, None),
    ]);
    // Pinning the *emptier* limit to the ring leaves the fuller one for
    // the inner ring.
    assert_eq!(u.headline_window(Some("weekly")).unwrap().id, "weekly");
    assert_eq!(u.second_window(Some("weekly")).unwrap().id, "5h");
}

#[test]
fn a_provider_with_two_pools_pairs_within_one_of_them() {
    // Antigravity reports a five-hour and a weekly for each of two model
    // groups, and they are separate budgets. Pairing the ring's Gemini
    // weekly with a Claude five-hour would put two unrelated pools on one
    // mark, with nothing to tell the reader they had been mixed.
    let u = usage(vec![
        window_with(0.00, "gemini-5h", Kind::FiveHour, Some("Gemini")),
        window_with(0.01, "gemini-weekly", Kind::Weekly, Some("Gemini")),
        window_with(0.00, "3p-5h", Kind::FiveHour, Some("Claude and GPT")),
        window_with(0.00, "3p-weekly", Kind::Weekly, Some("Claude and GPT")),
    ]);

    assert_eq!(u.headline_window(None).unwrap().id, "gemini-weekly");
    let second = u.second_window(None).unwrap();
    assert_eq!(second.scope.as_deref(), Some("Gemini"));
    assert_eq!(second.id, "gemini-5h");
}

#[test]
fn group_beats_fullness() {
    let u = usage(vec![
        window_with(0.90, "a-5h", Kind::FiveHour, Some("A")),
        window_with(0.10, "a-weekly", Kind::Weekly, Some("A")),
        window_with(0.60, "b-weekly", Kind::Weekly, Some("B")),
    ]);

    // B's 60% is fuller than A's 10%, and still the wrong answer: the ring
    // is A's, so the inner ring has to be A's too.
    assert_eq!(u.headline_window(None).unwrap().id, "a-5h");
    assert_eq!(u.second_window(None).unwrap().id, "a-weekly");
}

#[test]
fn a_group_with_nothing_else_falls_back_rather_than_drawing_nothing() {
    let u = usage(vec![
        window_with(0.80, "solo", Kind::FiveHour, Some("Only one here")),
        window_with(0.30, "other", Kind::Weekly, Some("Elsewhere")),
    ]);

    assert_eq!(u.headline_window(None).unwrap().id, "solo");
    // Better a limit from elsewhere than an empty ring.
    assert_eq!(u.second_window(None).unwrap().id, "other");
}

#[test]
fn claude_codes_unscoped_pair_are_each_others_group() {
    let u = usage(vec![
        window_with(0.14, "5h", Kind::FiveHour, None),
        window_with(0.08, "weekly", Kind::Weekly, None),
        window_with(0.00, "weekly-scoped", Kind::Weekly, Some("Fable")),
    ]);

    assert_eq!(u.headline_window(None).unwrap().id, "5h");
    // The model-scoped weekly is left to the card, even though it is a
    // window of the same provider.
    assert_eq!(u.second_window(None).unwrap().id, "weekly");
}

#[test]
fn two_limits_at_the_same_figure_still_resolve_to_two_different_rings() {
    let u = usage(vec![
        window_with(0.5, "a", Kind::Weekly, None),
        window_with(0.5, "b", Kind::Weekly, None),
    ]);
    let headline = u.headline_window(None).unwrap();
    let second = u.second_window(None).unwrap();
    assert_ne!(headline.id, second.id);
}

// MARK: - GLM Coding Plan refusals (ZaiErrorTests)

fn zai_reply(code: Option<i64>, msg: Option<&str>) -> serde_json::Value {
    let mut reply = serde_json::json!({ "success": false });
    if let Some(code) = code {
        reply["code"] = serde_json::json!(code);
    }
    if let Some(msg) = msg {
        reply["msg"] = serde_json::json!(msg);
    }
    reply
}

#[test]
fn a_key_of_the_wrong_shape_is_refused_in_chinese() {
    assert_eq!(
        zai::envelope_problem(&zai_reply(Some(401), Some("令牌已过期或验证不正确"))),
        pulse_core::model::Unavailability::ApiKeyRefused
    );
}

#[test]
fn a_well_formed_key_the_host_does_not_know_is_refused() {
    // The case that sent people looking for an outage. A z.ai key is the
    // right shape and the wrong account for BigModel.
    assert_eq!(
        zai::envelope_problem(&zai_reply(Some(1000), Some("身份验证失败。"))),
        pulse_core::model::Unavailability::ApiKeyRefused
    );
}

#[test]
fn no_authorization_header_reaching_the_server_is_refused() {
    assert_eq!(
        zai::envelope_problem(&zai_reply(
            Some(1001),
            Some("Header中未收到Authorization参数，无法进行身份验证。")
        )),
        pulse_core::model::Unavailability::ApiKeyRefused
    );
}

#[test]
fn the_international_host_says_the_same_things_in_english() {
    assert_eq!(
        zai::envelope_problem(&zai_reply(Some(401), Some("token expired or incorrect"))),
        pulse_core::model::Unavailability::ApiKeyRefused
    );
    assert_eq!(
        zai::envelope_problem(&zai_reply(Some(999), Some("invalid api key"))),
        pulse_core::model::Unavailability::ApiKeyRefused
    );
}

#[test]
fn chinese_wording_carries_even_when_the_code_is_unknown() {
    // The code list cannot be complete — it is one vendor's private
    // numbering and is not published in full — so the words have to work
    // on their own.
    for said in ["鉴权失败", "认证信息有误", "未授权的请求", "密钥无效", "无权限访问该接口"] {
        assert_eq!(
            zai::envelope_problem(&zai_reply(Some(12_345), Some(said))),
            pulse_core::model::Unavailability::ApiKeyRefused,
            "missed: {said}"
        );
    }
}

#[test]
fn a_key_that_works_on_an_account_with_no_plan_is_not_a_fault() {
    // All three hosts reply with this, over an HTTP 200, using the vendor's
    // generic 500: the code says nothing and only the sentence does.
    assert_eq!(
        zai::envelope_problem(&zai_reply(Some(500), Some("当前用户不存在coding plan"))),
        pulse_core::model::Unavailability::ZaiNoCodingPlan
    );
    // And it must outrank the code test: 500 alone would say the service
    // broke.
    assert_eq!(
        zai::envelope_problem(&zai_reply(Some(500), Some("user has no coding plan"))),
        pulse_core::model::Unavailability::ZaiNoCodingPlan
    );
}

#[test]
fn rate_limiting_and_real_server_faults_are_still_told_apart() {
    assert_eq!(
        zai::envelope_problem(&zai_reply(Some(429), Some("too many requests"))),
        pulse_core::model::Unavailability::RateLimited
    );
    assert_eq!(
        zai::envelope_problem(&zai_reply(Some(500), Some("内部错误"))),
        pulse_core::model::Unavailability::ServerError
    );
    assert_eq!(
        zai::envelope_problem(&zai_reply(None, None)),
        pulse_core::model::Unavailability::ServerError
    );
}

#[test]
fn the_two_glm_rows_are_named_for_the_shops_not_for_the_product() {
    // Both shops sell the same thing under the same name, so a row called
    // "GLM Coding Plan" is a row half the buyers pick wrongly — issue #13.
    assert_eq!(Provider::Zai.display_name(), "z.ai");
    assert_eq!(Provider::GlmCoding.display_name(), "Zhipu");
    for provider in [Provider::Zai, Provider::GlmCoding] {
        assert!(
            !provider.display_name().contains("GLM Coding Plan"),
            "the ambiguous name is what caused the mix-up"
        );
    }
}

// MARK: - GLM Coding Plan quota (ZaiQuotaTests)
// Fixture: Tests/PulseTests/Fixtures/glm-coding-plan-quota.json, captured
// from open.bigmodel.cn on 2026-09-07 with a live Lite key.

fn fixture_windows() -> Vec<UsageWindow> {
    let limits = serde_json::json!([
        { "type": "CREDIT_LIMIT", "unit": 3, "number": 5, "usage": 2000, "currentValue": 0, "remaining": 2000, "percentage": 0 },
        { "type": "CREDIT_LIMIT", "unit": 6, "number": 1, "usage": 10000, "currentValue": 0, "remaining": 10000, "percentage": 0, "nextResetTime": 1789373585999i64 }
    ]);
    zai::parse_limits(limits.as_array().unwrap(), Provider::GlmCoding)
}

#[test]
fn both_glm_limits_are_read_shortest_first() {
    let windows = fixture_windows();
    assert_eq!(windows.len(), 2);
    assert!(matches!(windows[0].kind, Kind::FiveHour));
    assert!(matches!(windows[1].kind, Kind::Weekly));
}

#[test]
fn glm_unit_and_number_are_a_real_duration_not_a_sort_key() {
    let windows = fixture_windows();
    // unit 3 is hours, unit 6 is weeks — 5 hours and 1 week, both stated
    // by the service, so the window clock and the forecast may divide.
    assert_eq!(windows[0].window_seconds, 5 * 3_600);
    assert_eq!(windows[1].window_seconds, 7 * 86_400);
    assert!(windows.iter().all(|w| w.reports_length));
}

#[test]
fn an_untouched_glm_plan_reads_as_nothing_used_not_as_no_reading() {
    let windows = fixture_windows();
    // `usage` and `remaining` are equal, so the spend is zero — and zero
    // is a reading. A missing figure has to stay None rather than draw a
    // full green ring.
    assert!(windows.iter().all(|w| w.used_fraction == 0.0));
    assert!(windows.iter().all(|w| !w.is_exhausted));
}

#[test]
fn the_glm_reset_stamp_is_milliseconds_and_only_one_limit_has_one() {
    let windows = fixture_windows();
    assert_eq!(windows[0].resets_at, None);
    assert_eq!(windows[1].resets_at, Some(1_789_373_585_999));
}

#[test]
fn glm_ids_are_unique_so_a_pinned_window_stays_resolvable() {
    let windows = fixture_windows();
    let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    // Two limits of the same type differing only in duration — the index
    // is in the id because a collision leaves a pin unresolvable.
    assert_eq!(unique.len(), ids.len());
}

// MARK: - DeepSeek denominators (DeepSeekParsingTests)

#[test]
fn balance_only_produces_no_window_at_all() {
    // `balanceOnly` means there is no denominator to draw, and the rail
    // shows the money in place of a percentage.
    let windows = deepseek::windows_for(
        &Purse { currency: "CNY".into(), total: 1_234.5 },
        "balanceOnly",
        None,
        0.0,
        0,
        None,
    );
    assert!(windows.is_empty());
}

#[test]
fn the_budget_basis_measures_against_your_budget() {
    let windows = deepseek::windows_for(
        &Purse { currency: "CNY".into(), total: 25.0 },
        "budget",
        Some(100.0),
        0.0,
        0,
        None,
    );
    assert_eq!(windows.len(), 1);
    assert!((windows[0].used_fraction - 0.75).abs() < 1e-9);
    // The denominator was inferred, and says where it came from.
    assert_eq!(windows[0].estimate.as_ref().map(|e| e.token()), Some("yourBudget"));
}

#[test]
fn a_budget_of_zero_or_less_is_not_a_denominator() {
    let windows = deepseek::windows_for(
        &Purse { currency: "CNY".into(), total: 25.0 },
        "budget",
        Some(0.0),
        0.0,
        0,
        None,
    );
    assert!(windows.is_empty());
}

#[test]
fn a_deepseek_window_has_no_length_and_no_reset_ever() {
    // Balance is prepaid credit, which is not a limit: it never turns over.
    let windows = deepseek::windows_for(
        &Purse { currency: "CNY".into(), total: 10.0 },
        "sinceTopUp",
        None,
        90.0,
        3_600_000,
        None,
    );
    assert_eq!(windows.len(), 1);
    assert!(!windows[0].reports_length);
    assert_eq!(windows[0].resets_at, None);
    assert!(matches!(windows[0].kind, Kind::Balance));
    assert_eq!(windows[0].estimate.as_ref().map(|e| e.token()), Some("sinceTopUp"));
}
