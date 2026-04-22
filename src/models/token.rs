//! Token usage and cost types for agent session analysis.
//!
//! These types are used across multiple features (checkpoint, TUI, session
//! discovery) and are therefore NOT feature-gated.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Token usage statistics from an agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    /// Number of input tokens consumed.
    pub input_tokens: u64,
    /// Number of output tokens generated.
    pub output_tokens: u64,
    /// Tokens read from cache (if applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Tokens written to cache (if applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

impl TokenUsage {
    /// Total tokens (input + output).
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Merge another TokenUsage into this one (additive).
    pub fn merge(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        match (self.cache_read_tokens, other.cache_read_tokens) {
            (Some(a), Some(b)) => self.cache_read_tokens = Some(a + b),
            (None, Some(b)) => self.cache_read_tokens = Some(b),
            _ => {}
        }
        match (self.cache_write_tokens, other.cache_write_tokens) {
            (Some(a), Some(b)) => self.cache_write_tokens = Some(a + b),
            (None, Some(b)) => self.cache_write_tokens = Some(b),
            _ => {}
        }
    }
}

/// A single tool call record from an agent session (extended tier data).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRecord {
    /// Tool name (e.g. "Read", "Write", "Bash", "Edit").
    pub tool_name: String,
    /// Truncated input summary (max 200 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_summary: Option<String>,
    /// When the tool was called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

/// Aggregated cost summary across one or more sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostSummary {
    /// Total token usage across all sessions.
    pub total_usage: TokenUsage,
    /// Number of sessions aggregated.
    pub session_count: usize,
    /// Per-agent breakdown: agent_name → TokenUsage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_agent: Vec<AgentTokenUsage>,
    /// Estimated cost in USD (approximate, based on public pricing).
    pub estimated_cost_usd: f64,
}

/// Token usage attributed to a specific agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTokenUsage {
    /// Agent name.
    pub agent_name: String,
    /// Token usage for this agent.
    pub usage: TokenUsage,
}

/// Maximum length for prompt_summary field.
pub const MAX_PROMPT_SUMMARY_LEN: usize = 2000;

/// Maximum length for tool call input_summary field.
pub const MAX_TOOL_INPUT_SUMMARY_LEN: usize = 200;

/// Truncate a string to a maximum length, appending "..." if truncated.
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find the nearest valid char boundary at or before the target byte index
        let target = max_len.saturating_sub(3);
        let boundary = s[..=target.min(s.len().saturating_sub(1))]
            .char_indices()
            .map(|(i, _)| i)
            .last()
            .unwrap_or(0);
        let truncated = &s[..boundary];
        format!("{truncated}...")
    }
}

// Approximate pricing (per 1M tokens, USD) — Claude Sonnet 4.5 as reference
const INPUT_PRICE_PER_M: f64 = 3.0;
const OUTPUT_PRICE_PER_M: f64 = 15.0;
const CACHE_READ_PRICE_PER_M: f64 = 0.30;
const CACHE_WRITE_PRICE_PER_M: f64 = 3.75;

/// Estimate USD cost from token usage using approximate public pricing.
pub fn estimate_cost(usage: &TokenUsage) -> f64 {
    let input_cost = usage.input_tokens as f64 / 1_000_000.0 * INPUT_PRICE_PER_M;
    let output_cost = usage.output_tokens as f64 / 1_000_000.0 * OUTPUT_PRICE_PER_M;
    let cache_read_cost =
        usage.cache_read_tokens.unwrap_or(0) as f64 / 1_000_000.0 * CACHE_READ_PRICE_PER_M;
    let cache_write_cost =
        usage.cache_write_tokens.unwrap_or(0) as f64 / 1_000_000.0 * CACHE_WRITE_PRICE_PER_M;
    input_cost + output_cost + cache_read_cost + cache_write_cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_string_works() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 8), "hello...");
        assert_eq!(truncate_string("", 5), "");
        assert_eq!(truncate_string("ab", 2), "ab");
    }

    #[test]
    fn token_usage_total() {
        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        assert_eq!(usage.total(), 1500);
    }

    #[test]
    fn token_usage_merge() {
        let mut a = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: Some(200),
            cache_write_tokens: None,
        };
        let b = TokenUsage {
            input_tokens: 2000,
            output_tokens: 300,
            cache_read_tokens: Some(100),
            cache_write_tokens: Some(50),
        };
        a.merge(&b);
        assert_eq!(a.input_tokens, 3000);
        assert_eq!(a.output_tokens, 800);
        assert_eq!(a.cache_read_tokens, Some(300));
        assert_eq!(a.cache_write_tokens, Some(50));
    }

    #[test]
    fn estimate_cost_basic() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: None,
            cache_write_tokens: None,
        };
        let cost = estimate_cost(&usage);
        // $3 input + $15 output = $18
        assert!((cost - 18.0).abs() < 0.01);
    }
}
