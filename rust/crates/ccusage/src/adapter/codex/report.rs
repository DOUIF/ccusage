use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{
    Align, CodexGroup, CodexModelUsage, CodexServiceTier, CodexTierUsage, Color, PricingMap,
    Result, SimpleTable,
    cli::{AgentReportKind, CodexSpeed, CodexSpeedView, SharedArgs},
    color, format_currency, format_models_multiline, format_number, json_float,
    missing_pricing_model_for_token_total, print_box_title,
    print_missing_pricing_warnings_for_models,
};

#[cfg(test)]
pub(super) fn report_from_groups(
    groups: &BTreeMap<String, CodexGroup>,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Value {
    report_from_groups_with_speed_view(
        groups,
        kind,
        pricing,
        speed,
        CodexServiceTier::Standard,
        CodexSpeedView::All,
    )
}

pub(super) fn report_from_groups_with_speed_view(
    groups: &BTreeMap<String, CodexGroup>,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
    speed_view: CodexSpeedView,
) -> Value {
    let rows = groups
        .iter()
        .filter_map(|(period, group)| {
            group_json(
                period,
                group,
                kind,
                pricing,
                speed,
                auto_fallback,
                speed_view,
            )
        })
        .collect::<Vec<_>>();
    let totals = totals_json(groups.values(), pricing, speed, auto_fallback, speed_view);
    json!({
        rows_key(kind): rows,
        "totals": totals,
    })
}

fn rows_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "daily",
        AgentReportKind::Weekly => "weekly",
        AgentReportKind::Monthly => "monthly",
        AgentReportKind::Session => "sessions",
    }
}

fn period_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "date",
        AgentReportKind::Weekly => "week",
        AgentReportKind::Monthly => "month",
        AgentReportKind::Session => "sessionId",
    }
}

fn group_json(
    period: &str,
    group: &CodexGroup,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
    speed_view: CodexSpeedView,
) -> Option<Value> {
    let selected_tier = match speed_view {
        CodexSpeedView::Standard => Some(CodexServiceTier::Standard),
        CodexSpeedView::Fast => Some(CodexServiceTier::Fast),
        CodexSpeedView::All | CodexSpeedView::Detailed => None,
    };
    let selected_group = selected_tier
        .map(|tier| group_for_tier(group, speed, auto_fallback, tier))
        .unwrap_or_else(|| group.clone());
    if selected_tier.is_some() && selected_group.total_tokens == 0 {
        return None;
    }
    let cost = selected_tier.map_or_else(
        || calculate_group_cost_with_policy(group, pricing, speed, auto_fallback),
        |tier| calculate_group_tier_cost(group, pricing, speed, auto_fallback, tier),
    );
    let input_tokens = non_cached_input_tokens(
        selected_group.input_tokens,
        selected_group.cached_input_tokens,
    );
    let models = selected_group
        .models
        .iter()
        .map(|(model, usage)| {
            let mut value = model_usage_json(usage);
            if matches!(speed_view, CodexSpeedView::Detailed)
                && let Some(original) = group.models.get(model)
            {
                value["speedBreakdown"] =
                    model_speed_breakdown_json(model, original, pricing, speed, auto_fallback);
            }
            (model.clone(), value)
        })
        .collect::<BTreeMap<_, _>>();
    let mut row = json!({
        period_key(kind): period,
        "inputTokens": input_tokens,
        "cacheCreationTokens": 0,
        "cacheReadTokens": selected_group.cached_input_tokens,
        "outputTokens": selected_group.output_tokens,
        "reasoningOutputTokens": selected_group.reasoning_output_tokens,
        "totalTokens": selected_group.total_tokens,
        "costUSD": json_float(cost),
        "models": models,
    });
    if matches!(speed_view, CodexSpeedView::Detailed) {
        row["speedBreakdown"] = group_speed_breakdown_json(group, pricing, speed, auto_fallback);
    }
    if kind == AgentReportKind::Session {
        row["lastActivity"] = json!(group.last_activity);
        let separator = period.rfind('/');
        row["sessionFile"] = json!(separator.map_or(period, |index| &period[index + 1..]));
        row["directory"] = json!(separator.map_or("", |index| &period[..index]));
    }
    Some(row)
}

pub(crate) fn non_cached_input_tokens(input_tokens: u64, cached_input_tokens: u64) -> u64 {
    input_tokens.saturating_sub(cached_input_tokens)
}

fn model_usage_json(usage: &CodexModelUsage) -> Value {
    json!({
        "inputTokens": non_cached_input_tokens(usage.input_tokens, usage.cached_input_tokens),
        "cacheCreationTokens": 0,
        "cacheReadTokens": usage.cached_input_tokens,
        "outputTokens": usage.output_tokens,
        "reasoningOutputTokens": usage.reasoning_output_tokens,
        "totalTokens": usage.total_tokens,
        "isFallback": usage.is_fallback,
    })
}

fn totals_json<'a>(
    groups: impl Iterator<Item = &'a CodexGroup>,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
    speed_view: CodexSpeedView,
) -> Value {
    let mut input = 0;
    let mut cached = 0;
    let mut output = 0;
    let mut reasoning = 0;
    let mut total = 0;
    let mut cost = 0.0;
    let groups = groups.collect::<Vec<_>>();
    let selected_tier = match speed_view {
        CodexSpeedView::Standard => Some(CodexServiceTier::Standard),
        CodexSpeedView::Fast => Some(CodexServiceTier::Fast),
        CodexSpeedView::All | CodexSpeedView::Detailed => None,
    };
    for group in &groups {
        let selected = selected_tier
            .map(|tier| group_for_tier(group, speed, auto_fallback, tier))
            .unwrap_or_else(|| (*group).clone());
        input += non_cached_input_tokens(selected.input_tokens, selected.cached_input_tokens);
        cached += selected.cached_input_tokens;
        output += selected.output_tokens;
        reasoning += selected.reasoning_output_tokens;
        total += selected.total_tokens;
        cost += selected_tier.map_or_else(
            || calculate_group_cost_with_policy(group, pricing, speed, auto_fallback),
            |tier| calculate_group_tier_cost(group, pricing, speed, auto_fallback, tier),
        );
    }
    let mut totals = json!({
        "inputTokens": input,
        "cacheCreationTokens": 0,
        "cacheReadTokens": cached,
        "outputTokens": output,
        "reasoningOutputTokens": reasoning,
        "totalTokens": total,
        "costUSD": json_float(cost),
    });
    if matches!(speed_view, CodexSpeedView::Detailed) {
        totals["speedBreakdown"] =
            totals_speed_breakdown_json(&groups, pricing, speed, auto_fallback);
    }
    totals
}

fn effective_model_tiers(
    usage: &CodexModelUsage,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
) -> (CodexModelUsage, CodexModelUsage) {
    match speed {
        CodexSpeed::Standard => (usage.clone(), CodexModelUsage::default()),
        CodexSpeed::Fast => (CodexModelUsage::default(), usage.clone()),
        CodexSpeed::Auto => {
            let mut standard = model_usage_from_tier(&usage.speed.standard);
            let mut fast = model_usage_from_tier(&usage.speed.fast);
            let unknown = model_usage_from_tier(&usage.speed.unknown);
            match auto_fallback {
                CodexServiceTier::Standard => merge_model_usage(&mut standard, &unknown),
                CodexServiceTier::Fast => merge_model_usage(&mut fast, &unknown),
            }
            standard.is_fallback = usage.is_fallback;
            fast.is_fallback = usage.is_fallback;
            (standard, fast)
        }
    }
}

fn merge_model_usage(target: &mut CodexModelUsage, source: &CodexModelUsage) {
    target.input_tokens += source.input_tokens;
    target.cached_input_tokens += source.cached_input_tokens;
    target.output_tokens += source.output_tokens;
    target.reasoning_output_tokens += source.reasoning_output_tokens;
    target.total_tokens += source.total_tokens;
    target.long_context_input_tokens += source.long_context_input_tokens;
    target.long_context_cached_input_tokens += source.long_context_cached_input_tokens;
    target.long_context_output_tokens += source.long_context_output_tokens;
    target.is_fallback |= source.is_fallback;
}

fn group_for_tier(
    group: &CodexGroup,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
    tier: CodexServiceTier,
) -> CodexGroup {
    let mut selected = CodexGroup {
        last_activity: group.last_activity.clone(),
        ..CodexGroup::default()
    };
    for (model, usage) in &group.models {
        let (standard, fast) = effective_model_tiers(usage, speed, auto_fallback);
        let usage = match tier {
            CodexServiceTier::Standard => standard,
            CodexServiceTier::Fast => fast,
        };
        if usage.total_tokens == 0 {
            continue;
        }
        selected.input_tokens += usage.input_tokens;
        selected.cached_input_tokens += usage.cached_input_tokens;
        selected.output_tokens += usage.output_tokens;
        selected.reasoning_output_tokens += usage.reasoning_output_tokens;
        selected.total_tokens += usage.total_tokens;
        selected.models.insert(model.clone(), usage);
    }
    selected
}

fn tier_usage_json(usage: &CodexModelUsage, cost: f64) -> Value {
    json!({
        "inputTokens": non_cached_input_tokens(usage.input_tokens, usage.cached_input_tokens),
        "cacheCreationTokens": 0,
        "cacheReadTokens": usage.cached_input_tokens,
        "outputTokens": usage.output_tokens,
        "reasoningOutputTokens": usage.reasoning_output_tokens,
        "totalTokens": usage.total_tokens,
        "costUSD": json_float(cost),
    })
}

fn model_speed_breakdown_json(
    model: &str,
    usage: &CodexModelUsage,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
) -> Value {
    let (standard, fast) = effective_model_tiers(usage, speed, auto_fallback);
    json!({
        "standard": tier_usage_json(
            &standard,
            calculate_codex_model_cost(model, &standard, pricing, CodexSpeed::Standard),
        ),
        "fast": tier_usage_json(
            &fast,
            calculate_codex_model_cost(model, &fast, pricing, CodexSpeed::Fast),
        ),
    })
}

fn aggregate_group_tier_usage(
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
    tier: CodexServiceTier,
) -> (CodexModelUsage, f64) {
    let mut total = CodexModelUsage::default();
    let mut cost = 0.0;
    for (model, usage) in &group.models {
        let (standard, fast) = effective_model_tiers(usage, speed, auto_fallback);
        let usage = match tier {
            CodexServiceTier::Standard => standard,
            CodexServiceTier::Fast => fast,
        };
        cost += calculate_codex_model_cost(
            model,
            &usage,
            pricing,
            match tier {
                CodexServiceTier::Standard => CodexSpeed::Standard,
                CodexServiceTier::Fast => CodexSpeed::Fast,
            },
        );
        merge_model_usage(&mut total, &usage);
    }
    (total, cost)
}

fn group_speed_breakdown_json(
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
) -> Value {
    let (standard, standard_cost) = aggregate_group_tier_usage(
        group,
        pricing,
        speed,
        auto_fallback,
        CodexServiceTier::Standard,
    );
    let (fast, fast_cost) =
        aggregate_group_tier_usage(group, pricing, speed, auto_fallback, CodexServiceTier::Fast);
    json!({
        "standard": tier_usage_json(&standard, standard_cost),
        "fast": tier_usage_json(&fast, fast_cost),
    })
}

fn totals_speed_breakdown_json(
    groups: &[&CodexGroup],
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
) -> Value {
    let mut standard = CodexModelUsage::default();
    let mut fast = CodexModelUsage::default();
    let mut standard_cost = 0.0;
    let mut fast_cost = 0.0;
    for group in groups {
        let (group_standard, group_standard_cost) = aggregate_group_tier_usage(
            group,
            pricing,
            speed,
            auto_fallback,
            CodexServiceTier::Standard,
        );
        let (group_fast, group_fast_cost) = aggregate_group_tier_usage(
            group,
            pricing,
            speed,
            auto_fallback,
            CodexServiceTier::Fast,
        );
        merge_model_usage(&mut standard, &group_standard);
        merge_model_usage(&mut fast, &group_fast);
        standard_cost += group_standard_cost;
        fast_cost += group_fast_cost;
    }
    json!({
        "standard": tier_usage_json(&standard, standard_cost),
        "fast": tier_usage_json(&fast, fast_cost),
    })
}

pub(crate) fn calculate_codex_model_cost(
    model: &str,
    usage: &CodexModelUsage,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> f64 {
    let Some(pricing) = pricing.find(model) else {
        return 0.0;
    };
    let multiplier = if matches!(speed, CodexSpeed::Fast) {
        if pricing.fast_multiplier == 1.0 {
            2.0
        } else {
            pricing.fast_multiplier
        }
    } else {
        1.0
    };
    let cache_read = if pricing.cache_read_explicit {
        pricing.cache_read
    } else {
        pricing.input
    };
    // OpenAI bills every token of a long-context request (input above 272K
    // tokens) at the long-context rates, so the aggregated usage is priced as
    // two independent buckets. Models without long-context rates fall back to
    // the flat rates, which keeps both buckets at the same price.
    let long_input_rate = pricing.input_above_200k.unwrap_or(pricing.input);
    let long_output_rate = pricing.output_above_200k.unwrap_or(pricing.output);
    let long_cache_read = if pricing.cache_read_explicit {
        pricing.cache_read_above_200k.unwrap_or(cache_read)
    } else {
        long_input_rate
    };
    let long_input = usage.long_context_input_tokens.min(usage.input_tokens);
    let long_cached = usage
        .long_context_cached_input_tokens
        .min(usage.cached_input_tokens)
        .min(long_input);
    let long_output = usage.long_context_output_tokens.min(usage.output_tokens);
    let short_non_cached =
        (usage.input_tokens - long_input).saturating_sub(usage.cached_input_tokens - long_cached);
    let long_non_cached = long_input - long_cached;
    (short_non_cached as f64 * pricing.input
        + (usage.cached_input_tokens - long_cached) as f64 * cache_read
        + (usage.output_tokens - long_output) as f64 * pricing.output
        + long_non_cached as f64 * long_input_rate
        + long_cached as f64 * long_cache_read
        + long_output as f64 * long_output_rate)
        * multiplier
}

pub(crate) fn calculate_codex_model_cost_with_policy(
    model: &str,
    usage: &CodexModelUsage,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
) -> f64 {
    if !matches!(speed, CodexSpeed::Auto) {
        return calculate_codex_model_cost(model, usage, pricing, speed);
    }
    let standard = model_usage_from_tier(&usage.speed.standard);
    let fast = model_usage_from_tier(&usage.speed.fast);
    let unknown = model_usage_from_tier(&usage.speed.unknown);
    calculate_codex_model_cost(model, &standard, pricing, CodexSpeed::Standard)
        + calculate_codex_model_cost(model, &fast, pricing, CodexSpeed::Fast)
        + calculate_codex_model_cost(
            model,
            &unknown,
            pricing,
            match auto_fallback {
                CodexServiceTier::Standard => CodexSpeed::Standard,
                CodexServiceTier::Fast => CodexSpeed::Fast,
            },
        )
}

fn model_usage_from_tier(usage: &CodexTierUsage) -> CodexModelUsage {
    CodexModelUsage {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        total_tokens: usage.total_tokens,
        long_context_input_tokens: usage.long_context_input_tokens,
        long_context_cached_input_tokens: usage.long_context_cached_input_tokens,
        long_context_output_tokens: usage.long_context_output_tokens,
        ..CodexModelUsage::default()
    }
}

pub(crate) fn calculate_group_cost_with_policy(
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
) -> f64 {
    group
        .models
        .iter()
        .map(|(model, usage)| {
            calculate_codex_model_cost_with_policy(model, usage, pricing, speed, auto_fallback)
        })
        .sum()
}

fn calculate_group_tier_cost(
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
    tier: CodexServiceTier,
) -> f64 {
    group
        .models
        .iter()
        .map(|(model, usage)| {
            let (standard, fast) = effective_model_tiers(usage, speed, auto_fallback);
            match tier {
                CodexServiceTier::Standard => {
                    calculate_codex_model_cost(model, &standard, pricing, CodexSpeed::Standard)
                }
                CodexServiceTier::Fast => {
                    calculate_codex_model_cost(model, &fast, pricing, CodexSpeed::Fast)
                }
            }
        })
        .sum()
}

pub(crate) fn codex_model_missing_pricing(
    model: &str,
    usage: &CodexModelUsage,
    pricing: &PricingMap,
) -> bool {
    missing_pricing_model_for_token_total(
        Some(model),
        usage
            .total_tokens
            .max(usage.input_tokens.saturating_add(usage.output_tokens)),
        Some(pricing),
    )
    .is_some()
}

pub(crate) fn codex_missing_pricing_models(
    groups: &BTreeMap<String, CodexGroup>,
    pricing: &PricingMap,
) -> Vec<String> {
    let mut models = BTreeSet::new();
    for group in groups.values() {
        for (model, usage) in &group.models {
            if codex_model_missing_pricing(model, usage, pricing) {
                models.insert(model.clone());
            }
        }
    }
    models.into_iter().collect()
}

pub(super) fn print_table_from_groups_with_speed_view(
    groups: &BTreeMap<String, CodexGroup>,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
    auto_fallback: CodexServiceTier,
    speed_view: CodexSpeedView,
    shared: &SharedArgs,
) -> Result<()> {
    if groups.is_empty() {
        eprintln!("No Codex usage data found.");
        return Ok(());
    }
    let first_column = match kind {
        AgentReportKind::Daily => "Date",
        AgentReportKind::Weekly => "Week",
        AgentReportKind::Monthly => "Month",
        AgentReportKind::Session => "Session",
    };
    print_box_title(
        &format!(
            "Codex Token Usage Report - {}",
            match kind {
                AgentReportKind::Daily => "Daily",
                AgentReportKind::Weekly => "Weekly",
                AgentReportKind::Monthly => "Monthly",
                AgentReportKind::Session => "Session",
            }
        ),
        shared,
    );
    let mut headers = vec![
        first_column,
        "Models",
        "Input",
        "Output",
        "Reasoning",
        "Cache Read",
        "Total Tokens",
        "Cost (USD)",
    ];
    let mut aligns = vec![
        Align::Left,
        Align::Left,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
    ];
    if shared.no_cost {
        headers.pop();
        aligns.pop();
    }
    let mut table = SimpleTable::new(headers, aligns, crate::terminal_style(shared))
        .with_terminal_width(crate::terminal_width())
        .with_date_compaction(true);
    let mut total_input = 0;
    let mut total_cached = 0;
    let mut total_output = 0;
    let mut total_reasoning = 0;
    let mut total_tokens = 0;
    let mut total_cost = 0.0;
    let mut display_groups = Vec::new();
    for (label, group) in groups {
        match speed_view {
            CodexSpeedView::All => display_groups.push((
                label.clone(),
                group.clone(),
                calculate_group_cost_with_policy(group, pricing, speed, auto_fallback),
            )),
            CodexSpeedView::Standard | CodexSpeedView::Fast => {
                let tier = if matches!(speed_view, CodexSpeedView::Standard) {
                    CodexServiceTier::Standard
                } else {
                    CodexServiceTier::Fast
                };
                let selected = group_for_tier(group, speed, auto_fallback, tier);
                if selected.total_tokens > 0 {
                    display_groups.push((
                        label.clone(),
                        selected,
                        calculate_group_tier_cost(group, pricing, speed, auto_fallback, tier),
                    ));
                }
            }
            CodexSpeedView::Detailed => {
                for (tier, suffix) in [
                    (CodexServiceTier::Standard, "Standard"),
                    (CodexServiceTier::Fast, "Fast"),
                ] {
                    let selected = group_for_tier(group, speed, auto_fallback, tier);
                    if selected.total_tokens > 0 {
                        display_groups.push((
                            format!("{label} / {suffix}"),
                            selected,
                            calculate_group_tier_cost(group, pricing, speed, auto_fallback, tier),
                        ));
                    }
                }
            }
        }
    }
    if display_groups.is_empty() {
        eprintln!("No Codex usage data found for the selected speed view.");
        return Ok(());
    }
    for (label, group, cost) in &display_groups {
        let input_tokens = non_cached_input_tokens(group.input_tokens, group.cached_input_tokens);
        total_input += input_tokens;
        total_cached += group.cached_input_tokens;
        total_output += group.output_tokens;
        total_reasoning += group.reasoning_output_tokens;
        total_tokens += group.total_tokens;
        total_cost += *cost;
        let models = format_models_multiline(&group.models.keys().cloned().collect::<Vec<_>>());
        let mut row = vec![
            label.clone(),
            models,
            format_number(input_tokens),
            format_number(group.output_tokens),
            format_number(group.reasoning_output_tokens),
            format_number(group.cached_input_tokens),
            format_number(group.total_tokens),
            format_currency(*cost),
        ];
        if shared.no_cost {
            row.pop();
        }
        table.push(row);
    }
    table.separator();
    let mut total_row = vec![
        color(shared, "Total", Color::Yellow),
        String::new(),
        color(shared, format_number(total_input), Color::Yellow),
        color(shared, format_number(total_output), Color::Yellow),
        color(shared, format_number(total_reasoning), Color::Yellow),
        color(shared, format_number(total_cached), Color::Yellow),
        color(shared, format_number(total_tokens), Color::Yellow),
        color(shared, format_currency(total_cost), Color::Yellow),
    ];
    if shared.no_cost {
        total_row.pop();
    }
    table.push(total_row);
    table.print()?;
    let missing_models = codex_missing_pricing_models(groups, pricing);
    print_missing_pricing_warnings_for_models(
        missing_models.iter().map(String::as_str),
        shared.offline,
    );
    Ok(())
}
