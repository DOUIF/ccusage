mod aggregate;
mod loader;
mod parser;
mod paths;
mod report;
mod speed;
mod types;

use crate::{PricingMap, Result, cli::AgentCommandArgs, log_level, print_json_or_jq, wants_json};

pub(crate) use aggregate::{aggregate_events, filter_events_by_date, load_groups};
pub(crate) use loader::load_codex_events;
#[cfg(test)]
pub(crate) use loader::load_codex_events_from_directory;
pub(crate) use report::{
    calculate_codex_model_cost_with_policy, calculate_group_cost_with_policy,
    codex_model_missing_pricing, non_cached_input_tokens,
};
pub(crate) use speed::resolve_codex_auto_fallback;

use report::{print_table_from_groups_with_speed_view, report_from_groups_with_speed_view};

#[cfg(test)]
use report::{calculate_codex_model_cost, report_from_groups};

#[cfg(test)]
use crate::{
    CodexTokenUsageEvent,
    cli::{AgentReportKind, CodexSpeed},
};

#[cfg(test)]
use serde_json::Value;

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let groups = load_groups(&shared, args.kind)?;
    let speed = args.codex_speed;
    let auto_fallback = resolve_codex_auto_fallback();
    if wants_json(&shared) {
        let output = report_from_groups_with_speed_view(
            &groups,
            args.kind,
            &pricing,
            speed,
            auto_fallback,
            args.codex_speed_view,
        );
        return print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost);
    }
    print_table_from_groups_with_speed_view(
        &groups,
        args.kind,
        &pricing,
        speed,
        auto_fallback,
        args.codex_speed_view,
        &shared,
    )
}

#[cfg(test)]
pub(crate) fn report_json(
    events: &[CodexTokenUsageEvent],
    kind: AgentReportKind,
    timezone: Option<&str>,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<Value> {
    let groups = aggregate_events(events, kind, timezone)?;
    Ok(report_from_groups(&groups, kind, pricing, speed))
}

#[cfg(test)]
pub(crate) fn report_json_with_speed_view(
    events: &[CodexTokenUsageEvent],
    kind: AgentReportKind,
    timezone: Option<&str>,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: crate::CodexServiceTier,
    speed_view: crate::cli::CodexSpeedView,
) -> Result<Value> {
    let groups = aggregate_events(events, kind, timezone)?;
    Ok(report_from_groups_with_speed_view(
        &groups,
        kind,
        pricing,
        speed,
        auto_fallback,
        speed_view,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::aggregate::load_groups_from_directory;
    use super::*;
    use crate::cli::SharedArgs;
    use crate::{CodexModelUsage, CodexTokenUsageEvent};
    use ccusage_test_support::fs_fixture;

    #[test]
    fn loads_directory_groups_with_date_filter_without_global_event_vector() {
        let fixture = fs_fixture!({
            "sessions/session.jsonl": [
                r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150}}}}"#,
                r#"{"timestamp":"2026-01-03T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":200,"cached_input_tokens":20,"output_tokens":75,"reasoning_output_tokens":5,"total_tokens":280}}}}"#,
            ]
            .join("\n"),
        });
        let sessions_dir = fixture.path("sessions");
        let shared = SharedArgs {
            since: Some("20260103".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let groups =
            load_groups_from_directory(&sessions_dir, &shared, AgentReportKind::Daily).unwrap();

        assert_eq!(groups.len(), 1);
        let group = groups.get("2026-01-03").unwrap();
        assert_eq!(group.input_tokens, 200);
        assert_eq!(group.cached_input_tokens, 20);
        assert_eq!(group.output_tokens, 75);
        assert_eq!(group.reasoning_output_tokens, 5);
        assert_eq!(group.total_tokens, 280);
    }

    #[test]
    fn dedupes_matching_grouped_codex_usage_events_from_distinct_sessions() {
        let usage_line = r#"{"timestamp":"2026-01-02T00:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"cached_input_tokens":10,"output_tokens":50,"reasoning_output_tokens":0,"total_tokens":150}}}}"#;
        let fixture = fs_fixture!({
            "sessions/session-a.jsonl": usage_line,
            "sessions/session-b.jsonl": usage_line,
        });
        let sessions_dir = fixture.path("sessions");
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let groups =
            load_groups_from_directory(&sessions_dir, &shared, AgentReportKind::Daily).unwrap();

        assert_eq!(groups.len(), 1);
        let group = groups.get("2026-01-02").unwrap();
        assert_eq!(group.input_tokens, 100);
        assert_eq!(group.cached_input_tokens, 10);
        assert_eq!(group.output_tokens, 50);
        assert_eq!(group.total_tokens, 150);
    }

    #[test]
    fn reports_non_cached_codex_input_separately_from_cached_input() {
        let pricing = PricingMap::default();
        let report = report_json(
            &[CodexTokenUsageEvent {
                session_id: "session-1".to_string(),
                timestamp: "2026-01-02T00:00:00.000Z".to_string(),
                model: Some("gpt-5".to_string()),
                service_tier: None,
                input_tokens: 100,
                cached_input_tokens: 90,
                output_tokens: 5,
                reasoning_output_tokens: 0,
                total_tokens: 105,
                is_fallback_model: false,
            }],
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();

        assert_eq!(report["daily"][0]["inputTokens"], 10);
        assert_eq!(report["daily"][0]["cacheCreationTokens"], 0);
        assert_eq!(report["daily"][0]["cacheReadTokens"], 90);
        assert_eq!(report["daily"][0]["totalTokens"], 105);
        assert_eq!(report["totals"]["inputTokens"], 10);
        assert_eq!(report["totals"]["cacheCreationTokens"], 0);
        assert_eq!(report["totals"]["cacheReadTokens"], 90);
        assert_eq!(report["totals"]["totalTokens"], 105);
        assert_eq!(report["daily"][0]["models"]["gpt-5"]["inputTokens"], 10);
        assert_eq!(
            report["daily"][0]["models"]["gpt-5"]["cacheCreationTokens"],
            0
        );
        assert_eq!(report["daily"][0]["models"]["gpt-5"]["cacheReadTokens"], 90);
    }

    #[test]
    fn reports_codex_model_aliases_without_raw_model_names() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([
            ("private-codex-alpha", "gpt-5.5"),
            ("private-codex-beta", "gpt-5.5"),
        ]);
        let pricing = PricingMap::default();
        let report = report_json(
            &[
                CodexTokenUsageEvent {
                    session_id: "session-1".to_string(),
                    timestamp: "2026-01-02T00:00:00.000Z".to_string(),
                    model: Some("private-codex-alpha".to_string()),
                    service_tier: None,
                    input_tokens: 100,
                    cached_input_tokens: 10,
                    output_tokens: 5,
                    reasoning_output_tokens: 0,
                    total_tokens: 105,
                    is_fallback_model: false,
                },
                CodexTokenUsageEvent {
                    session_id: "session-1".to_string(),
                    timestamp: "2026-01-02T00:00:01.000Z".to_string(),
                    model: Some("private-codex-beta".to_string()),
                    service_tier: None,
                    input_tokens: 50,
                    cached_input_tokens: 5,
                    output_tokens: 3,
                    reasoning_output_tokens: 0,
                    total_tokens: 53,
                    is_fallback_model: false,
                },
            ],
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();

        let models = report["daily"][0]["models"].as_object().unwrap();
        assert!(models.contains_key("gpt-5.5"));
        assert!(!models.contains_key("private-codex-alpha"));
        assert!(!models.contains_key("private-codex-beta"));
        assert_eq!(models["gpt-5.5"]["inputTokens"], 135);
        assert_eq!(models["gpt-5.5"]["cacheReadTokens"], 15);
        assert_eq!(models["gpt-5.5"]["outputTokens"], 8);
    }

    #[test]
    fn charges_cached_input_at_input_rate_when_codex_pricing_omits_cache_read_rate() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );
        let usage = CodexModelUsage {
            input_tokens: 100,
            cached_input_tokens: 40,
            output_tokens: 5,
            reasoning_output_tokens: 0,
            total_tokens: 105,
            ..CodexModelUsage::default()
        };

        let cost = calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Standard);

        assert!((cost - 0.00015).abs() < f64::EPSILON);
    }

    #[test]
    fn bills_long_context_codex_requests_at_long_context_rates() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-long": {
                    "input_cost_per_token": 0.000005,
                    "output_cost_per_token": 0.00003,
                    "cache_read_input_token_cost": 0.0000005,
                    "input_cost_per_token_above_200k_tokens": 0.00001,
                    "output_cost_per_token_above_200k_tokens": 0.000045,
                    "cache_read_input_token_cost_above_200k_tokens": 0.000001
                }
            }"#,
        );
        let usage = CodexModelUsage {
            input_tokens: 350_000,
            cached_input_tokens: 50_000,
            output_tokens: 1_000,
            total_tokens: 351_000,
            long_context_input_tokens: 300_000,
            long_context_cached_input_tokens: 40_000,
            long_context_output_tokens: 800,
            ..CodexModelUsage::default()
        };

        let cost = calculate_codex_model_cost("gpt-long", &usage, &pricing, CodexSpeed::Standard);

        // Short bucket: 40K non-cached input, 10K cached, 200 output tokens.
        // Long bucket: 260K non-cached input, 40K cached, 800 output tokens.
        let expected = 40_000.0 * 5e-6
            + 10_000.0 * 0.5e-6
            + 200.0 * 30e-6
            + 260_000.0 * 10e-6
            + 40_000.0 * 1e-6
            + 800.0 * 45e-6;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn long_context_split_without_tier_rates_matches_flat_pricing() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.00001
                }
            }"#,
        );
        let flat = CodexModelUsage {
            input_tokens: 400_000,
            cached_input_tokens: 100_000,
            output_tokens: 2_000,
            total_tokens: 402_000,
            ..CodexModelUsage::default()
        };
        let split = CodexModelUsage {
            long_context_input_tokens: 300_000,
            long_context_cached_input_tokens: 80_000,
            long_context_output_tokens: 1_500,
            ..flat.clone()
        };

        let flat_cost =
            calculate_codex_model_cost("gpt-test", &flat, &pricing, CodexSpeed::Standard);
        let split_cost =
            calculate_codex_model_cost("gpt-test", &split, &pricing, CodexSpeed::Standard);

        assert!((flat_cost - split_cost).abs() < f64::EPSILON);
    }

    #[test]
    fn prices_gpt_5_6_long_context_usage_from_embedded_pricing() {
        let pricing = PricingMap::load_embedded();
        let usage = CodexModelUsage {
            input_tokens: 300_000,
            cached_input_tokens: 100_000,
            output_tokens: 1_000,
            total_tokens: 301_000,
            long_context_input_tokens: 300_000,
            long_context_cached_input_tokens: 100_000,
            long_context_output_tokens: 1_000,
            ..CodexModelUsage::default()
        };

        let cost =
            calculate_codex_model_cost("gpt-5.6-sol", &usage, &pricing, CodexSpeed::Standard);

        // The whole request is billed at long-context rates: 200K non-cached
        // input at $10/M, 100K cached at $1/M, 1K output at $45/M.
        let expected = 200_000.0 * 10e-6 + 100_000.0 * 1e-6 + 1_000.0 * 45e-6;
        assert!((cost - expected).abs() < 1e-9);
    }

    #[test]
    fn applies_speed_option_to_codex_cost() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.3-codex": {
                    "input_cost_per_token": 0.00000175,
                    "output_cost_per_token": 0.000014,
                    "cache_read_input_token_cost": 0.000000175
                }
            }"#,
        );
        let usage = CodexModelUsage {
            input_tokens: 100,
            cached_input_tokens: 40,
            output_tokens: 5,
            reasoning_output_tokens: 0,
            total_tokens: 105,
            ..CodexModelUsage::default()
        };

        let standard =
            calculate_codex_model_cost("gpt-5.3-codex", &usage, &pricing, CodexSpeed::Standard);
        let fast = calculate_codex_model_cost("gpt-5.3-codex", &usage, &pricing, CodexSpeed::Fast);

        assert!((fast - (standard * 2.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn auto_prices_recorded_tiers_and_falls_back_only_for_unknown_usage() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );
        let tier = |input_tokens, output_tokens| crate::CodexTierUsage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens + output_tokens,
            ..crate::CodexTierUsage::default()
        };
        let usage = CodexModelUsage {
            input_tokens: 250,
            output_tokens: 25,
            total_tokens: 275,
            speed: crate::CodexSpeedUsage {
                standard: tier(100, 10),
                fast: tier(100, 10),
                unknown: tier(50, 5),
            },
            ..CodexModelUsage::default()
        };

        let cost = calculate_codex_model_cost_with_policy(
            "gpt-test",
            &usage,
            &pricing,
            CodexSpeed::Auto,
            crate::CodexServiceTier::Standard,
        );

        assert!((cost - 0.0007).abs() < f64::EPSILON);
    }

    #[test]
    fn auto_preserves_fast_pricing_for_recorded_long_context_usage() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "input_cost_per_token_above_200k_tokens": 0.000002,
                    "output_cost_per_token_above_200k_tokens": 0.000020,
                    "fast_mode_multiplier": 2.0
                }
            }"#,
        );
        let tier = crate::CodexTierUsage {
            input_tokens: 300_001,
            output_tokens: 10,
            total_tokens: 300_011,
            long_context_input_tokens: 300_001,
            long_context_output_tokens: 10,
            ..crate::CodexTierUsage::default()
        };
        let usage = CodexModelUsage {
            input_tokens: 300_001,
            output_tokens: 10,
            total_tokens: 300_011,
            long_context_input_tokens: 300_001,
            long_context_output_tokens: 10,
            speed: crate::CodexSpeedUsage {
                fast: tier,
                ..crate::CodexSpeedUsage::default()
            },
            ..CodexModelUsage::default()
        };

        let auto_cost = calculate_codex_model_cost_with_policy(
            "gpt-test",
            &usage,
            &pricing,
            CodexSpeed::Auto,
            crate::CodexServiceTier::Standard,
        );
        let forced_fast =
            calculate_codex_model_cost("gpt-test", &usage, &pricing, CodexSpeed::Fast);

        assert!((auto_cost - forced_fast).abs() < f64::EPSILON);
        assert!((auto_cost - 1.200_404).abs() < 1e-9);
    }

    #[test]
    fn detailed_json_reports_full_standard_and_fast_breakdowns() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );
        let event =
            |timestamp: &str, service_tier, input_tokens, output_tokens| CodexTokenUsageEvent {
                session_id: "session-1".to_string(),
                timestamp: timestamp.to_string(),
                model: Some("gpt-test".to_string()),
                service_tier,
                input_tokens,
                cached_input_tokens: 0,
                output_tokens,
                reasoning_output_tokens: 0,
                total_tokens: input_tokens + output_tokens,
                is_fallback_model: false,
            };
        let events = vec![
            event(
                "2026-07-14T08:00:00.000Z",
                Some(crate::CodexServiceTier::Standard),
                100,
                10,
            ),
            event(
                "2026-07-14T08:01:00.000Z",
                Some(crate::CodexServiceTier::Fast),
                50,
                5,
            ),
        ];

        let report = report_json_with_speed_view(
            &events,
            AgentReportKind::Daily,
            Some("UTC"),
            &pricing,
            CodexSpeed::Auto,
            crate::CodexServiceTier::Standard,
            crate::cli::CodexSpeedView::Detailed,
        )
        .unwrap();

        assert_eq!(
            report["daily"][0]["speedBreakdown"]["standard"]["inputTokens"],
            100
        );
        assert_eq!(
            report["daily"][0]["speedBreakdown"]["standard"]["totalTokens"],
            110
        );
        assert_eq!(
            report["daily"][0]["speedBreakdown"]["fast"]["inputTokens"],
            50
        );
        assert_eq!(
            report["daily"][0]["speedBreakdown"]["fast"]["totalTokens"],
            55
        );
        assert_eq!(
            report["daily"][0]["models"]["gpt-test"]["speedBreakdown"]["fast"]["outputTokens"],
            5
        );
        let standard_cost = report["totals"]["speedBreakdown"]["standard"]["costUSD"]
            .as_f64()
            .unwrap();
        let fast_cost = report["totals"]["speedBreakdown"]["fast"]["costUSD"]
            .as_f64()
            .unwrap();
        assert!((standard_cost - 0.0002).abs() < f64::EPSILON);
        assert!((fast_cost - 0.0002).abs() < f64::EPSILON);
    }

    #[test]
    fn speed_view_filters_turns_by_effective_tier() {
        let event = |timestamp: &str, service_tier, total_tokens| CodexTokenUsageEvent {
            session_id: "session-1".to_string(),
            timestamp: timestamp.to_string(),
            model: Some("gpt-test".to_string()),
            service_tier,
            input_tokens: total_tokens,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            total_tokens,
            is_fallback_model: false,
        };
        let events = vec![
            event(
                "2026-07-14T08:00:00.000Z",
                Some(crate::CodexServiceTier::Standard),
                100,
            ),
            event(
                "2026-07-15T08:00:00.000Z",
                Some(crate::CodexServiceTier::Fast),
                50,
            ),
        ];

        let report = report_json_with_speed_view(
            &events,
            AgentReportKind::Daily,
            Some("UTC"),
            &PricingMap::default(),
            CodexSpeed::Auto,
            crate::CodexServiceTier::Standard,
            crate::cli::CodexSpeedView::Fast,
        )
        .unwrap();

        assert_eq!(report["daily"].as_array().unwrap().len(), 1);
        assert_eq!(report["daily"][0]["date"], "2026-07-15");
        assert_eq!(report["totals"]["totalTokens"], 50);
        assert!(report["daily"][0].get("speedBreakdown").is_none());
    }

    #[test]
    fn explicit_speed_override_applies_before_view_filter() {
        let events = [CodexTokenUsageEvent {
            session_id: "session-1".to_string(),
            timestamp: "2026-07-14T08:00:00.000Z".to_string(),
            model: Some("gpt-test".to_string()),
            service_tier: Some(crate::CodexServiceTier::Standard),
            input_tokens: 100,
            cached_input_tokens: 0,
            output_tokens: 10,
            reasoning_output_tokens: 0,
            total_tokens: 110,
            is_fallback_model: false,
        }];

        let report = report_json_with_speed_view(
            &events,
            AgentReportKind::Daily,
            Some("UTC"),
            &PricingMap::default(),
            CodexSpeed::Fast,
            crate::CodexServiceTier::Standard,
            crate::cli::CodexSpeedView::Fast,
        )
        .unwrap();

        assert_eq!(report["daily"].as_array().unwrap().len(), 1);
        assert_eq!(report["totals"]["totalTokens"], 110);
    }

    #[test]
    fn identifies_codex_models_missing_pricing() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-known": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010
                }
            }"#,
        );
        let mut group = crate::CodexGroup::default();
        group.models.insert(
            "gpt-known".to_string(),
            CodexModelUsage {
                input_tokens: 100,
                output_tokens: 5,
                total_tokens: 105,
                ..CodexModelUsage::default()
            },
        );
        group.models.insert(
            "gpt-unknown".to_string(),
            CodexModelUsage {
                input_tokens: 200,
                output_tokens: 10,
                total_tokens: 210,
                ..CodexModelUsage::default()
            },
        );
        let groups = BTreeMap::from([("2026-01-02".to_string(), group)]);

        assert_eq!(
            report::codex_missing_pricing_models(&groups, &pricing),
            vec!["gpt-unknown".to_string()]
        );
    }

    #[test]
    fn snapshots_codex_reports_for_periods_sessions_costs_and_fallback_models() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-5.3-codex": {
                    "input_cost_per_token": 0.00000175,
                    "output_cost_per_token": 0.000014,
                    "cache_read_input_token_cost": 0.000000175
                },
                "gpt-5-mini": {
                    "input_cost_per_token": 0.00000025,
                    "output_cost_per_token": 0.000002
                }
            }"#,
        );
        let events = vec![
            CodexTokenUsageEvent {
                session_id: "/workspace/api/session-a.jsonl".to_string(),
                timestamp: "2026-01-02T00:00:00.000Z".to_string(),
                model: Some("gpt-5.3-codex".to_string()),
                service_tier: None,
                input_tokens: 140,
                cached_input_tokens: 40,
                output_tokens: 5,
                reasoning_output_tokens: 2,
                total_tokens: 147,
                is_fallback_model: false,
            },
            CodexTokenUsageEvent {
                session_id: "/workspace/api/session-a.jsonl".to_string(),
                timestamp: "2026-01-02T00:05:00.000Z".to_string(),
                model: Some("gpt-5.3-codex".to_string()),
                service_tier: None,
                input_tokens: 70,
                cached_input_tokens: 70,
                output_tokens: 10,
                reasoning_output_tokens: 0,
                total_tokens: 80,
                is_fallback_model: true,
            },
            CodexTokenUsageEvent {
                session_id: "/workspace/web/session-b.jsonl".to_string(),
                timestamp: "2026-01-05T23:59:59.000Z".to_string(),
                model: Some("gpt-5-mini".to_string()),
                service_tier: None,
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 2,
                reasoning_output_tokens: 0,
                total_tokens: 12,
                is_fallback_model: false,
            },
            CodexTokenUsageEvent {
                session_id: "ignored-missing-model".to_string(),
                timestamp: "2026-01-06T00:00:00.000Z".to_string(),
                model: None,
                service_tier: None,
                input_tokens: 999,
                cached_input_tokens: 0,
                output_tokens: 999,
                reasoning_output_tokens: 0,
                total_tokens: 1_998,
                is_fallback_model: false,
            },
        ];

        insta::assert_json_snapshot!(serde_json::json!({
            "daily": report_json(
                &events,
                AgentReportKind::Daily,
                Some("UTC"),
                &pricing,
                CodexSpeed::Standard,
            )
            .unwrap(),
            "weekly": report_json(
                &events,
                AgentReportKind::Weekly,
                Some("UTC"),
                &pricing,
                CodexSpeed::Standard,
            )
            .unwrap(),
            "monthly": report_json(
                &events,
                AgentReportKind::Monthly,
                Some("UTC"),
                &pricing,
                CodexSpeed::Standard,
            )
            .unwrap(),
            "sessionFast": report_json(
                &events,
                AgentReportKind::Session,
                Some("UTC"),
                &pricing,
                CodexSpeed::Fast,
            )
            .unwrap(),
        }));
    }
}
