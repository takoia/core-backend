//! Notional per-token pricing used to value LLM usage (the platform's own
//! spend estimate and the basis for marketplace billing). Configured, not
//! hardcoded: `LLM_PRICING` is a JSON object keyed by model-name prefix.
//!
//! ```json
//! { "claude-opus": {"input_per_m": 15, "output_per_m": 75},
//!   "claude":      {"input_per_m": 3,  "output_per_m": 15},
//!   "gpt-4o-mini": {"input_per_m": 0.15, "output_per_m": 0.6} }
//! ```
//!
//! The longest matching prefix wins (case-insensitive). Unknown models fall
//! back to `LLM_PRICING_DEFAULT` (same shape, a single rule) so usage is never
//! valued at zero by accident; the canned demo provider is always free.

use crate::llm::TokenUsage;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub struct Rate {
    /// USD per million prompt tokens.
    pub input_per_m: f64,
    /// USD per million completion tokens.
    pub output_per_m: f64,
}

impl Rate {
    pub const FREE: Rate = Rate {
        input_per_m: 0.0,
        output_per_m: 0.0,
    };
}

#[derive(Debug, Clone)]
pub struct Pricing {
    /// (lower-cased model prefix, rate), sorted longest prefix first.
    rules: Vec<(String, Rate)>,
    fallback: Rate,
}

impl Default for Pricing {
    /// The historical constant (3 / 15 USD per M) applied to everything.
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            fallback: Rate {
                input_per_m: 3.0,
                output_per_m: 15.0,
            },
        }
    }
}

impl Pricing {
    /// Build from the raw `LLM_PRICING` and `LLM_PRICING_DEFAULT` values.
    /// Absent values keep the defaults; malformed JSON is a configuration error.
    pub fn from_env(rules_json: Option<&str>, default_json: Option<&str>) -> Result<Self> {
        let mut pricing = Pricing::default();
        if let Some(raw) = rules_json.map(str::trim).filter(|s| !s.is_empty()) {
            let map: HashMap<String, Rate> = serde_json::from_str(raw).context(
                "LLM_PRICING must be a JSON object of {prefix: {input_per_m, output_per_m}}",
            )?;
            let mut rules: Vec<(String, Rate)> = map
                .into_iter()
                .map(|(k, v)| (k.to_lowercase(), v))
                .collect();
            rules.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
            pricing.rules = rules;
        }
        if let Some(raw) = default_json.map(str::trim).filter(|s| !s.is_empty()) {
            pricing.fallback = serde_json::from_str(raw)
                .context("LLM_PRICING_DEFAULT must be {input_per_m, output_per_m}")?;
        }
        Ok(pricing)
    }

    /// Rate for a model name: longest matching prefix, else the fallback. The
    /// canned demo provider is free whatever the configuration says.
    pub fn rate_for(&self, model: &str) -> Rate {
        let m = model.to_lowercase();
        if m.starts_with("canned") {
            return Rate::FREE;
        }
        self.rules
            .iter()
            .find(|(prefix, _)| m.starts_with(prefix.as_str()))
            .map(|(_, r)| *r)
            .unwrap_or(self.fallback)
    }

    /// Notional USD cost of one completion.
    pub fn cost_usd(&self, model: &str, usage: TokenUsage) -> f64 {
        let r = self.rate_for(model);
        (usage.prompt_tokens as f64 * r.input_per_m
            + usage.completion_tokens as f64 * r.output_per_m)
            / 1_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(p: u32, c: u32) -> TokenUsage {
        TokenUsage {
            prompt_tokens: p,
            completion_tokens: c,
        }
    }

    #[test]
    fn default_keeps_the_historical_constant() {
        let p = Pricing::default();
        let cost = p.cost_usd("whatever-model", usage(1_000_000, 1_000_000));
        assert!((cost - 18.0).abs() < 1e-9);
    }

    #[test]
    fn longest_prefix_wins_case_insensitively() {
        let p = Pricing::from_env(
            Some(r#"{"claude": {"input_per_m": 3, "output_per_m": 15}, "claude-opus": {"input_per_m": 15, "output_per_m": 75}}"#),
            None,
        )
        .unwrap();
        assert_eq!(p.rate_for("Claude-Opus-4").output_per_m, 75.0);
        assert_eq!(p.rate_for("claude-sonnet-4").output_per_m, 15.0);
        assert_eq!(
            p.rate_for("gpt-4o").output_per_m,
            15.0,
            "unknown falls back"
        );
    }

    #[test]
    fn canned_is_always_free_and_default_is_configurable() {
        let p = Pricing::from_env(None, Some(r#"{"input_per_m": 1, "output_per_m": 2}"#)).unwrap();
        assert_eq!(p.rate_for("canned-demo"), Rate::FREE);
        assert!((p.cost_usd("x", usage(1_000_000, 1_000_000)) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn malformed_configuration_is_an_error_not_a_silent_default() {
        assert!(Pricing::from_env(Some("not json"), None).is_err());
        assert!(Pricing::from_env(None, Some("[]")).is_err());
    }
}
