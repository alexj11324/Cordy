//! LLM price table and model-alias resolution.
use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

/// Scale of provider-reported costs: xAI reports whole ticks of 1e-10 USD.
/// Declared here rather than imported from the agent runtime (which owns the
/// wire-format parsing) so this crate keeps no dependency on it for a physical
/// unit; the two must stay equal.
pub const COST_USD_TICKS_PER_USD: i64 = 10_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub provider: &'static str,
    pub model: &'static str,
    pub input_per_m: f64,
    pub cache_read_per_m: f64,
    pub cache_write_per_m: f64,
    pub output_per_m: f64,
}

static MODEL_PRICES: LazyLock<HashMap<&'static str, ModelPrice>> = LazyLock::new(|| {
    let rows: &[(&str, ModelPrice)] = &[
        // GPT-5.6 series (Codex `codex` provider). For 5.6+ cache read is the
        // 90%-off cached-input rate and cache write is billed at 1.25x the
        // uncached input rate.
        (
            "openai:gpt-5.6-sol",
            ModelPrice {
                provider: "openai",
                model: "gpt-5.6-sol",
                input_per_m: 5.00,
                cache_read_per_m: 0.50,
                cache_write_per_m: 6.25,
                output_per_m: 30.00,
            },
        ),
        (
            "openai:gpt-5.6-terra",
            ModelPrice {
                provider: "openai",
                model: "gpt-5.6-terra",
                input_per_m: 2.50,
                cache_read_per_m: 0.25,
                cache_write_per_m: 3.125,
                output_per_m: 15.00,
            },
        ),
        (
            "openai:gpt-5.6-luna",
            ModelPrice {
                provider: "openai",
                model: "gpt-5.6-luna",
                input_per_m: 1.00,
                cache_read_per_m: 0.10,
                cache_write_per_m: 1.25,
                output_per_m: 6.00,
            },
        ),
        (
            "openai:gpt-5.5",
            ModelPrice {
                provider: "openai",
                model: "gpt-5.5",
                input_per_m: 5.00,
                cache_read_per_m: 0.50,
                cache_write_per_m: 0.50,
                output_per_m: 30.00,
            },
        ),
        (
            "openai:gpt-5.4",
            ModelPrice {
                provider: "openai",
                model: "gpt-5.4",
                input_per_m: 2.50,
                cache_read_per_m: 0.25,
                cache_write_per_m: 0.25,
                output_per_m: 15.00,
            },
        ),
        (
            "openai:gpt-5.4-mini",
            ModelPrice {
                provider: "openai",
                model: "gpt-5.4-mini",
                input_per_m: 0.75,
                cache_read_per_m: 0.075,
                cache_write_per_m: 0.075,
                output_per_m: 4.50,
            },
        ),
        (
            "openai:gpt-5.3-codex",
            ModelPrice {
                provider: "openai",
                model: "gpt-5.3-codex",
                input_per_m: 1.75,
                cache_read_per_m: 0.175,
                cache_write_per_m: 0.175,
                output_per_m: 14.00,
            },
        ),
        (
            "openai:gpt-5.2-codex",
            ModelPrice {
                provider: "openai",
                model: "gpt-5.2-codex",
                input_per_m: 1.75,
                cache_read_per_m: 0.175,
                cache_write_per_m: 0.175,
                output_per_m: 14.00,
            },
        ),
        // Anthropic Sonnet 5 launch price is $2 / $10 through 2026-08-31; the
        // static table cannot schedule the post-intro change yet.
        (
            "anthropic:claude-sonnet-5",
            ModelPrice {
                provider: "anthropic",
                model: "claude-sonnet-5",
                input_per_m: 2.00,
                cache_read_per_m: 0.20,
                cache_write_per_m: 2.50,
                output_per_m: 10.00,
            },
        ),
        (
            "anthropic:claude-fable-5",
            ModelPrice {
                provider: "anthropic",
                model: "claude-fable-5",
                input_per_m: 10.00,
                cache_read_per_m: 1.00,
                cache_write_per_m: 12.50,
                output_per_m: 50.00,
            },
        ),
        (
            "anthropic:claude-opus-5",
            ModelPrice {
                provider: "anthropic",
                model: "claude-opus-5",
                input_per_m: 5.00,
                cache_read_per_m: 0.50,
                cache_write_per_m: 6.25,
                output_per_m: 25.00,
            },
        ),
        (
            "anthropic:claude-opus-4.8",
            ModelPrice {
                provider: "anthropic",
                model: "claude-opus-4.8",
                input_per_m: 5.00,
                cache_read_per_m: 0.50,
                cache_write_per_m: 6.25,
                output_per_m: 25.00,
            },
        ),
        (
            "anthropic:claude-opus-4.7",
            ModelPrice {
                provider: "anthropic",
                model: "claude-opus-4.7",
                input_per_m: 5.00,
                cache_read_per_m: 0.50,
                cache_write_per_m: 6.25,
                output_per_m: 25.00,
            },
        ),
        (
            "anthropic:claude-opus-4.6",
            ModelPrice {
                provider: "anthropic",
                model: "claude-opus-4.6",
                input_per_m: 5.00,
                cache_read_per_m: 0.50,
                cache_write_per_m: 6.25,
                output_per_m: 25.00,
            },
        ),
        (
            "anthropic:claude-opus-4.5",
            ModelPrice {
                provider: "anthropic",
                model: "claude-opus-4.5",
                input_per_m: 5.00,
                cache_read_per_m: 0.50,
                cache_write_per_m: 6.25,
                output_per_m: 25.00,
            },
        ),
        (
            "anthropic:claude-sonnet-4.6",
            ModelPrice {
                provider: "anthropic",
                model: "claude-sonnet-4.6",
                input_per_m: 3.00,
                cache_read_per_m: 0.30,
                cache_write_per_m: 3.75,
                output_per_m: 15.00,
            },
        ),
        (
            "anthropic:claude-sonnet-4.5",
            ModelPrice {
                provider: "anthropic",
                model: "claude-sonnet-4.5",
                input_per_m: 3.00,
                cache_read_per_m: 0.30,
                cache_write_per_m: 3.75,
                output_per_m: 15.00,
            },
        ),
        (
            "anthropic:claude-haiku-4.5",
            ModelPrice {
                provider: "anthropic",
                model: "claude-haiku-4.5",
                input_per_m: 1.00,
                cache_read_per_m: 0.10,
                cache_write_per_m: 1.25,
                output_per_m: 5.00,
            },
        ),
        (
            "deepseek:v4-pro",
            ModelPrice {
                provider: "deepseek",
                model: "v4-pro",
                input_per_m: 1.74,
                cache_read_per_m: 0.0145,
                cache_write_per_m: 1.74,
                output_per_m: 3.48,
            },
        ),
        (
            "deepseek:v4-flash",
            ModelPrice {
                provider: "deepseek",
                model: "v4-flash",
                input_per_m: 0.56,
                cache_read_per_m: 0.0112,
                cache_write_per_m: 0.56,
                output_per_m: 1.12,
            },
        ),
        (
            "minimax:m2.7",
            ModelPrice {
                provider: "minimax",
                model: "m2.7",
                input_per_m: 0.30,
                cache_read_per_m: 0.06,
                cache_write_per_m: 0.375,
                output_per_m: 1.20,
            },
        ),
        (
            "minimax:m2.7-highspeed",
            ModelPrice {
                provider: "minimax",
                model: "m2.7-highspeed",
                input_per_m: 0.60,
                cache_read_per_m: 0.06,
                cache_write_per_m: 0.375,
                output_per_m: 2.40,
            },
        ),
        (
            "google:gemini-3-flash",
            ModelPrice {
                provider: "google",
                model: "gemini-3-flash",
                input_per_m: 0.50,
                cache_read_per_m: 0.05,
                cache_write_per_m: 0.50,
                output_per_m: 3.00,
            },
        ),
        (
            "google:gemini-3.1-pro",
            ModelPrice {
                provider: "google",
                model: "gemini-3.1-pro",
                input_per_m: 2.00,
                cache_read_per_m: 0.20,
                cache_write_per_m: 2.00,
                output_per_m: 12.00,
            },
        ),
        (
            "google:gemini-2.5-pro",
            ModelPrice {
                provider: "google",
                model: "gemini-2.5-pro",
                input_per_m: 1.25,
                cache_read_per_m: 0.31,
                cache_write_per_m: 1.25,
                output_per_m: 10.00,
            },
        ),
        (
            "google:gemini-2.5-flash",
            ModelPrice {
                provider: "google",
                model: "gemini-2.5-flash",
                input_per_m: 0.30,
                cache_read_per_m: 0.03,
                cache_write_per_m: 0.30,
                output_per_m: 2.50,
            },
        ),
        // xAI Grok. Short-context tier: a request bills 2x once its prompt
        // reaches 200K tokens, but a usage record aggregates every model call
        // in a turn, so pricing the standard tier under-estimates a
        // long-context turn by at most 50% instead of over-estimating a short
        // one by 100%. No separate cache-write rate exists (writes bill as
        // normal input). `grok-composer-*` ships in the Grok Build catalog but
        // is absent from the price sheet, so it stays unmapped rather than
        // inheriting a guessed rate.
        (
            "xai:grok-4.5",
            ModelPrice {
                provider: "xai",
                model: "grok-4.5",
                input_per_m: 2.00,
                cache_read_per_m: 0.30,
                cache_write_per_m: 2.00,
                output_per_m: 6.00,
            },
        ),
        (
            "xai:grok-4.3",
            ModelPrice {
                provider: "xai",
                model: "grok-4.3",
                input_per_m: 1.25,
                cache_read_per_m: 0.20,
                cache_write_per_m: 1.25,
                output_per_m: 2.50,
            },
        ),
        (
            "xai:grok-build-0.1",
            ModelPrice {
                provider: "xai",
                model: "grok-build-0.1",
                input_per_m: 1.00,
                cache_read_per_m: 0.20,
                cache_write_per_m: 1.00,
                output_per_m: 2.00,
            },
        ),
        (
            "xai:grok-4.20-multi-agent-0309",
            ModelPrice {
                provider: "xai",
                model: "grok-4.20-multi-agent-0309",
                input_per_m: 1.25,
                cache_read_per_m: 0.20,
                cache_write_per_m: 1.25,
                output_per_m: 2.50,
            },
        ),
        (
            "xai:grok-4.20-0309-reasoning",
            ModelPrice {
                provider: "xai",
                model: "grok-4.20-0309-reasoning",
                input_per_m: 1.25,
                cache_read_per_m: 0.20,
                cache_write_per_m: 1.25,
                output_per_m: 2.50,
            },
        ),
        (
            "xai:grok-4.20-0309-non-reasoning",
            ModelPrice {
                provider: "xai",
                model: "grok-4.20-0309-non-reasoning",
                input_per_m: 1.25,
                cache_read_per_m: 0.20,
                cache_write_per_m: 1.25,
                output_per_m: 2.50,
            },
        ),
    ];
    rows.iter().copied().collect()
});

static MODEL_ALIAS_RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let rules: &[(&str, &str)] = &[
        // Anchored exact-match: the effort is carried in a separate field, so
        // the model id is the bare slug. Anchoring to `$` keeps unknown
        // variants (`gpt-5.6-luna-pro`, `gpt-5.6-luna/x`) out of these rows.
        // The `.` is a LITERAL dot — the real Codex slug is always dotted, and
        // the frontend resolver does NOT dash-normalize, so a dashed
        // `gpt-5-6-luna` must surface as unmapped on both sides rather than
        // silently borrowing a tier here.
        (r"(^|/|:)gpt-5\.6-sol$", "openai:gpt-5.6-sol"),
        (r"(^|/|:)gpt-5\.6-terra$", "openai:gpt-5.6-terra"),
        (r"(^|/|:)gpt-5\.6-luna$", "openai:gpt-5.6-luna"),
        (r"(^|/|:)gpt-5[.-]5$|^gpt-5-5$", "openai:gpt-5.5"),
        (r"(^|/|:)gpt-5[.-]4($|-2026-03-05|-xhigh)", "openai:gpt-5.4"),
        (
            r"(^|/|:)gpt-5[.-]4-mini($|[^a-z0-9])",
            "openai:gpt-5.4-mini",
        ),
        (r"(^|/|:)gpt-5[.-]3-codex$", "openai:gpt-5.3-codex"),
        (r"(^|/|:)gpt-5[.-]2-codex$", "openai:gpt-5.2-codex"),
        (
            r"claude-sonnet-5|claude-5-sonnet",
            "anthropic:claude-sonnet-5",
        ),
        (r"claude-fable-5", "anthropic:claude-fable-5"),
        (r"claude-opus-5", "anthropic:claude-opus-5"),
        (r"claude-opus-4[-.]8", "anthropic:claude-opus-4.8"),
        (r"claude-opus-4[-.]7", "anthropic:claude-opus-4.7"),
        (r"claude-opus-4[-.]6", "anthropic:claude-opus-4.6"),
        (r"claude-opus-4[-.]5", "anthropic:claude-opus-4.5"),
        (
            r"claude-sonnet-4[-.]6|claude-4[-.]6-sonnet",
            "anthropic:claude-sonnet-4.6",
        ),
        (
            r"claude-sonnet-4[-.]5|claude-4[-.]5-sonnet",
            "anthropic:claude-sonnet-4.5",
        ),
        (r"claude-haiku-4[-.]5", "anthropic:claude-haiku-4.5"),
        (r"deepseek-v4-pro", "deepseek:v4-pro"),
        (
            r"deepseek-v4-flash|^deepseek-chat$|^deepseek-reasoner$",
            "deepseek:v4-flash",
        ),
        (
            r"minimax-m2[.]7.*highspeed|highspeed.*minimax-m2[.]7",
            "minimax:m2.7-highspeed",
        ),
        (r"minimax-m2[.]7", "minimax:m2.7"),
        (r"gemini-3-flash", "google:gemini-3-flash"),
        (r"gemini-3[.]1-pro", "google:gemini-3.1-pro"),
        (r"gemini-2[.]5-pro", "google:gemini-2.5-pro"),
        (r"gemini-2[.]5-flash", "google:gemini-2.5-flash"),
        // Anchored exact-match, dotted spelling only — same rule as the
        // gpt-5.6 rows above.
        (r"(^|/|:)grok-4\.5$", "xai:grok-4.5"),
        (r"(^|/|:)grok-4\.3$", "xai:grok-4.3"),
        (r"(^|/|:)grok-build-0\.1$", "xai:grok-build-0.1"),
        (
            r"(^|/|:)grok-4\.20-multi-agent-0309$",
            "xai:grok-4.20-multi-agent-0309",
        ),
        (
            r"(^|/|:)grok-4\.20-0309-reasoning$",
            "xai:grok-4.20-0309-reasoning",
        ),
        (
            r"(^|/|:)grok-4\.20-0309-non-reasoning$",
            "xai:grok-4.20-0309-non-reasoning",
        ),
    ];
    rules
        .iter()
        .map(|(pattern, key)| (Regex::new(pattern).unwrap(), *key))
        .collect()
});

/// Resolves a raw model alias to its price row. First matching rule wins;
/// `None` when no rule matches or the pointed row is absent.
pub fn price_for_model_alias(model: &str) -> Option<ModelPrice> {
    let model = model.trim().to_lowercase();
    for (re, price_key) in MODEL_ALIAS_RULES.iter() {
        if re.is_match(&model) {
            return MODEL_PRICES.get(price_key).copied();
        }
    }
    None
}

pub fn token_cost_usd(tokens: i64, price_per_m: f64) -> f64 {
    if tokens <= 0 || price_per_m <= 0.0 {
        return 0.0;
    }
    tokens as f64 * price_per_m / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_prefixed_matches() {
        let luna = price_for_model_alias("gpt-5.6-luna").unwrap();
        assert_eq!(luna.provider, "openai");
        assert_eq!(luna.model, "gpt-5.6-luna");
        assert_eq!(luna.output_per_m, 6.00);

        let prefixed = price_for_model_alias("openai/gpt-5.6-luna").unwrap();
        assert_eq!(prefixed.model, "gpt-5.6-luna");

        assert_eq!(
            price_for_model_alias("GPT-5.6-LUNA ").unwrap().model,
            "gpt-5.6-luna"
        );
    }

    #[test]
    fn anchored_rules_reject_variants() {
        assert!(price_for_model_alias("gpt-5.6-luna-pro").is_none());
        assert!(price_for_model_alias("gpt-5.6-luna/x").is_none());
        // Dashed grok spelling must NOT borrow the dotted tier.
        assert!(price_for_model_alias("grok-4-5").is_none());
        assert!(price_for_model_alias("xai/grok-4.5").is_some());
    }

    #[test]
    fn dash_tolerant_rows_still_apply_where_intended() {
        assert_eq!(price_for_model_alias("gpt-5-5").unwrap().model, "gpt-5.5");
        assert_eq!(
            price_for_model_alias("gpt-5.4-xhigh").unwrap().model,
            "gpt-5.4"
        );
        assert_eq!(
            price_for_model_alias("claude-4-6-sonnet").unwrap().model,
            "claude-sonnet-4.6"
        );
        assert_eq!(
            price_for_model_alias("deepseek-chat").unwrap().model,
            "v4-flash"
        );
    }

    #[test]
    fn highspeed_rule_precedes_base_row() {
        assert_eq!(
            price_for_model_alias("minimax-m2.7-highspeed")
                .unwrap()
                .model,
            "m2.7-highspeed"
        );
        assert_eq!(price_for_model_alias("minimax-m2.7").unwrap().model, "m2.7");
    }

    #[test]
    fn unpriced_aliases_return_none() {
        assert!(price_for_model_alias("").is_none());
        assert!(price_for_model_alias("grok-composer-9").is_none());
        assert!(price_for_model_alias("totally-made-up").is_none());
    }

    #[test]
    fn token_cost_math() {
        assert_eq!(token_cost_usd(1_000_000, 2.0), 2.0);
        assert_eq!(token_cost_usd(500_000, 2.0), 1.0);
        assert_eq!(token_cost_usd(0, 2.0), 0.0);
        assert_eq!(token_cost_usd(-5, 2.0), 0.0);
        assert_eq!(token_cost_usd(1000, 0.0), 0.0);
        assert!((token_cost_usd(1_500_000, 0.075) - 0.1125).abs() < 1e-12);
    }
}
