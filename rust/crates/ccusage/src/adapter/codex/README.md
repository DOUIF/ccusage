# Codex Source

Data source:

```text
${CODEX_HOME:-~/.codex}/sessions/
${CODEX_HOME:-~/.codex}/archived_sessions/
```

When both directories contain the same relative JSONL path for one Codex home,
the active `sessions/` copy wins.

Relevant JSONL event:

- `type === "event_msg"`
- `payload.type === "token_count"`
- `payload.info.total_token_usage` is cumulative.
- `payload.info.last_token_usage` is the current turn delta.
- If only cumulative totals exist, subtract prior totals to recover deltas.

Speed metadata:

- `type === "event_msg"` with `payload.type === "thread_settings_applied"` updates the effective speed for following turns.
- `payload.thread_settings.service_tier === "priority"` or `"fast"` is Fast.
- Other recorded tier values are Standard.
- Usage before the first recorded tier remains unknown and uses the configured fallback only for pricing.

Token mapping:

- `input_tokens` - total input tokens.
- `cached_input_tokens` - cached prompt tokens.
- `output_tokens` - completion tokens, including reasoning cost.
- `reasoning_output_tokens` - informational breakdown; already included in output billing.
- `total_tokens` - provided directly or recomputed as input plus output for legacy entries.

Pricing uses model metadata from `turn_context` and speed metadata from `thread_settings_applied`. Early sessions without model metadata fall back to `gpt-5`, mark `isFallbackModel === true`, and expose fallback rows as approximate in aggregate JSON.
