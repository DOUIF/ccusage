# Codex Per-Turn Speed Design

## Goal

Make Codex cost reports price mixed Standard and Fast turns accurately from session JSONL, while preserving legacy-log behavior and allowing users to view combined, filtered, or detailed per-speed usage.

## CLI Behavior

Keep the existing pricing-policy flag:

```text
--speed auto|standard|fast
```

- `auto` uses recorded per-turn speed when available and falls back to the current Codex `config.toml` speed only for events whose speed is unknown.
- `standard` forces all usage to Standard pricing.
- `fast` forces all usage to Fast pricing.

Add a presentation flag:

```text
--speed-view all|standard|fast|detailed
```

- `all` is the default and preserves the current combined table and JSON schema.
- `standard` includes only usage whose effective pricing tier is Standard.
- `fast` includes only usage whose effective pricing tier is Fast.
- `detailed` emits separate Standard and Fast table subrows and JSON breakdowns.

The speed view operates on the effective tier after applying `--speed`. This keeps displayed breakdowns consistent with `costUSD`.

## Source Parsing

The Codex parser recognizes session entries with:

```json
{
  "type": "event_msg",
  "payload": {
    "type": "thread_settings_applied",
    "thread_settings": {
      "service_tier": "priority"
    }
  }
}
```

The parser maintains current session speed state:

- `priority` or legacy `fast` means Fast.
- `null`, `standard`, or another non-Fast string means Standard.
- A session with no applicable setting event remains Unknown.
- Malformed or non-object `thread_settings` is ignored and leaves the previous state unchanged.
- Repeated setting events update state but never emit usage.
- Every subsequent `token_count` event inherits the latest state.

`CodexTokenUsageEvent` carries `Option<CodexServiceTier>`. Headless and older logs remain Unknown unless their format later exposes an explicit tier.

## Aggregation And Pricing

Aggregation retains Standard, Fast, and Unknown token buckets for every model. Each bucket records input, cached input, output, reasoning output, total tokens, and the existing long-context subsets.

At report time:

1. Resolve the Unknown bucket with the `auto` config fallback, or map all buckets to the explicit `--speed` override.
2. Merge buckets that resolve to the same effective tier.
3. Apply regular pricing to Standard usage and the model-specific LiteLLM Fast multiplier to Fast usage, falling back to 2x when the multiplier is absent.
4. Preserve long-context pricing independently inside both effective tiers.

This avoids pricing mixed-speed aggregated tokens with one global multiplier and correctly handles the Fast/Standard and short/long-context cross-product.

## Output

For `--speed-view all`, existing rows, totals, models, and table layout remain unchanged.

For `standard` and `fast`, rows, model usage, token totals, and costs contain only the selected effective tier. Periods with no selected usage are omitted.

For `detailed`, table output uses one Standard or Fast subrow per non-empty tier for each period, plus tiered totals. It retains the existing columns: model, input, output, reasoning, cache read, total tokens, and cost.

Detailed JSON adds `speedBreakdown` to every report row, every model entry, and totals:

```json
{
  "speedBreakdown": {
    "standard": {
      "inputTokens": 0,
      "cacheCreationTokens": 0,
      "cacheReadTokens": 0,
      "outputTokens": 0,
      "reasoningOutputTokens": 0,
      "totalTokens": 0,
      "costUSD": 0
    },
    "fast": {
      "inputTokens": 0,
      "cacheCreationTokens": 0,
      "cacheReadTokens": 0,
      "outputTokens": 0,
      "reasoningOutputTokens": 0,
      "totalTokens": 0,
      "costUSD": 0
    }
  }
}
```

Unknown is never exposed as a third output tier because it is resolved before filtering and rendering.

## Compatibility And Failure Handling

- Existing commands without `--speed-view` retain their output shape.
- Explicit `--speed fast|standard` retains its force-all pricing behavior.
- Legacy and headless logs retain their existing token totals and use config-based fallback in `auto` mode.
- Invalid setting events do not discard usage or reset model state.
- JSON whitespace and field order remain tolerant through the existing parser path.

## Testing

Use fixture-backed Rust tests and strict Red-Green-Refactor cycles for:

1. Fast and Standard settings applying to following token events.
2. Fast to Standard to Fast switching within one session.
3. Old logs remaining Unknown.
4. `auto` applying config fallback only to Unknown usage.
5. Explicit `--speed` overriding all recorded tiers.
6. `standard` and `fast` views filtering rows, models, totals, and costs.
7. Detailed JSON breakdowns at row, model, and totals levels.
8. Detailed table snapshots with tier subrows.
9. Correct Fast multiplier and long-context pricing interaction.
10. Existing Codex parser, loader, aggregate, report, and repository test suites.

## Documentation

Update the Codex adapter README and `docs/guide/codex/index.md` to explain per-turn detection, legacy fallback, and the distinction between `--speed` pricing policy and `--speed-view` presentation.
