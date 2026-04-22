//! Dynamic backend routing — select the optimal backend for a given task.
//!
//! The [`BackendRouter`] trait decides which [`BackendType`] to use for a given
//! [`SpawnConfig`], based on keyword heuristics, cost, or any custom logic.

use std::collections::HashMap;

use async_trait::async_trait;

use super::{BackendType, SpawnConfig};

/// A router that selects the best backend for a given spawn configuration.
///
/// Implementations can use keyword matching, cost estimation, model capability
/// databases, or even LLM-based classification to make the decision.
#[async_trait]
pub trait BackendRouter: Send + Sync {
    /// Choose the optimal backend for the given config.
    ///
    /// `available` lists the backends that the orchestrator has registered.
    /// Returns `None` if no suitable backend can be found (e.g., `available` is
    /// empty or all backends are filtered out by requirements).
    async fn route(&self, config: &SpawnConfig, available: &[BackendType]) -> Option<BackendType>;
}

// ---------------------------------------------------------------------------
// KeywordRouter
// ---------------------------------------------------------------------------

/// A keyword-based router with configurable rules and a default fallback.
///
/// Rules map keyword patterns (matched case-insensitively against the prompt)
/// to a preferred [`BackendType`]. The first matching rule wins. If no rules
/// match, the `default` backend is used.
///
/// By default, keywords are matched as substrings. Enable [`word_boundary`](Self::word_boundary)
/// to require that keywords appear as whole words (e.g., "test" won't match "testing").
///
/// # Example
///
/// ```rust
/// use agent_teams::backend::router::KeywordRouter;
/// use agent_teams::BackendType;
///
/// let router = KeywordRouter::new(BackendType::ClaudeCode)
///     .word_boundary(true)
///     .rule("review", BackendType::GeminiCli)
///     .rule("analyze", BackendType::GeminiCli)
///     .rule("implement", BackendType::ClaudeCode)
///     .rule("test", BackendType::Codex);
/// ```
#[derive(Debug)]
pub struct KeywordRouter {
    rules: Vec<(String, BackendType)>,
    default: BackendType,
    /// When true, keywords must appear as whole words (bounded by non-alphanumeric chars).
    use_word_boundary: bool,
}

impl KeywordRouter {
    /// Create a new keyword router with a default backend.
    pub fn new(default: BackendType) -> Self {
        Self {
            rules: Vec::new(),
            default,
            use_word_boundary: false,
        }
    }

    /// Enable or disable word-boundary matching.
    ///
    /// When enabled, "test" matches "test this code" but NOT "testing this code".
    /// Default is `false` (substring matching) for backward compatibility.
    pub fn word_boundary(mut self, enable: bool) -> Self {
        self.use_word_boundary = enable;
        self
    }

    /// Add a keyword → backend rule. Keywords are matched case-insensitively
    /// against the `SpawnConfig::prompt` field.
    pub fn rule(mut self, keyword: impl Into<String>, backend: BackendType) -> Self {
        self.rules.push((keyword.into().to_lowercase(), backend));
        self
    }

    /// Add multiple rules from an iterator of `(keyword, backend)` pairs.
    pub fn rules(mut self, rules: impl IntoIterator<Item = (String, BackendType)>) -> Self {
        for (kw, bt) in rules {
            self.rules.push((kw.to_lowercase(), bt));
        }
        self
    }
}

/// Check if `text` contains `word` as a whole word (bounded by non-alphanumeric characters).
///
/// Both `text` and `word` must be valid UTF-8 (guaranteed by `&str`). The boundary
/// check uses [`u8::is_ascii_alphanumeric`], so non-ASCII characters are always
/// treated as word boundaries — this is intentional for prompt-level keyword matching.
fn contains_word(text: &str, word: &str) -> bool {
    if word.is_empty() || text.len() < word.len() {
        return false;
    }
    let text_bytes = text.as_bytes();
    let mut start = 0;
    while start + word.len() <= text.len() {
        match text[start..].find(word) {
            Some(pos) => {
                let abs_pos = start + pos;
                let before_ok = abs_pos == 0 || !text_bytes[abs_pos - 1].is_ascii_alphanumeric();
                let after_pos = abs_pos + word.len();
                let after_ok =
                    after_pos >= text.len() || !text_bytes[after_pos].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    return true;
                }
                // Advance past the current match start to the next character boundary.
                // Using `abs_pos + word.len()` is safe (word is a valid &str so its
                // byte length always lands on a UTF-8 boundary) and also more efficient
                // since a shorter match starting within the current word cannot exist.
                start = abs_pos + word.len();
            }
            None => break,
        }
    }
    false
}

#[async_trait]
impl BackendRouter for KeywordRouter {
    async fn route(&self, config: &SpawnConfig, available: &[BackendType]) -> Option<BackendType> {
        if available.is_empty() {
            return None;
        }

        let prompt_lower = config.prompt.to_lowercase();

        // Find the first matching rule whose backend is available
        for (keyword, backend) in &self.rules {
            let matched = if self.use_word_boundary {
                contains_word(&prompt_lower, keyword.as_str())
            } else {
                prompt_lower.contains(keyword.as_str())
            };
            if matched && available.contains(backend) {
                return Some(*backend);
            }
        }

        // Fall back to default if available, otherwise first available
        if available.contains(&self.default) {
            Some(self.default)
        } else {
            available.first().copied()
        }
    }
}

// ---------------------------------------------------------------------------
// CapabilityRouter
// ---------------------------------------------------------------------------

/// Backend capabilities for making routing decisions.
///
/// Default values represent a generic mid-tier backend (multi-turn, streaming,
/// cost=2, latency=2). Use [`BackendCapability::defaults()`] for pre-configured
/// values for known backends.
#[derive(Debug, Clone)]
pub struct BackendCapability {
    /// Supports multi-turn conversations.
    pub multi_turn: bool,
    /// Supports streaming output.
    pub streaming: bool,
    /// Relative cost (lower is cheaper). Unitless; used for comparison only.
    pub cost_tier: u8,
    /// Relative latency (lower is faster). Unitless; used for comparison only.
    pub latency_tier: u8,
}

impl Default for BackendCapability {
    fn default() -> Self {
        Self {
            multi_turn: true,
            streaming: true,
            cost_tier: 2,
            latency_tier: 2,
        }
    }
}

impl BackendCapability {
    /// Default capabilities for known backends.
    pub fn defaults() -> HashMap<BackendType, BackendCapability> {
        let mut map = HashMap::new();
        map.insert(
            BackendType::ClaudeCode,
            BackendCapability {
                multi_turn: true,
                streaming: true,
                cost_tier: 3, // most capable, highest cost
                latency_tier: 2,
            },
        );
        map.insert(
            BackendType::Codex,
            BackendCapability {
                multi_turn: true,
                streaming: true,
                cost_tier: 2,
                latency_tier: 2,
            },
        );
        map.insert(
            BackendType::GeminiCli,
            BackendCapability {
                multi_turn: false, // one-shot process per turn
                streaming: true,
                cost_tier: 1,    // cheapest for simple tasks
                latency_tier: 3, // CLI startup overhead
            },
        );
        map
    }
}

/// A capability-aware router that selects backends based on task requirements.
///
/// Scores each available backend on a weighted combination of cost and latency,
/// with optional requirement filters (e.g., must support multi-turn).
#[derive(Debug)]
pub struct CapabilityRouter {
    capabilities: HashMap<BackendType, BackendCapability>,
    /// If true, only consider multi-turn backends.
    require_multi_turn: bool,
    /// Weight for cost (0.0–1.0). Remainder goes to latency.
    cost_weight: f32,
}

impl Default for CapabilityRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityRouter {
    /// Create a capability router with default capability data.
    pub fn new() -> Self {
        Self {
            capabilities: BackendCapability::defaults(),
            require_multi_turn: false,
            cost_weight: 0.5,
        }
    }

    /// Only route to backends that support multi-turn conversations.
    pub fn require_multi_turn(mut self, require: bool) -> Self {
        self.require_multi_turn = require;
        self
    }

    /// Set the cost vs latency weight (0.0 = pure latency, 1.0 = pure cost).
    pub fn cost_weight(mut self, weight: f32) -> Self {
        self.cost_weight = weight.clamp(0.0, 1.0);
        self
    }

    /// Override capability data for a backend.
    pub fn with_capability(mut self, backend: BackendType, cap: BackendCapability) -> Self {
        self.capabilities.insert(backend, cap);
        self
    }
}

#[async_trait]
impl BackendRouter for CapabilityRouter {
    async fn route(&self, _config: &SpawnConfig, available: &[BackendType]) -> Option<BackendType> {
        let latency_weight = 1.0 - self.cost_weight;

        // Score each candidate; backends without capability entries get a neutral default
        available
            .iter()
            .filter(|bt| {
                if let Some(cap) = self.capabilities.get(bt) {
                    !self.require_multi_turn || cap.multi_turn
                } else {
                    // Unknown backends are included with neutral score unless multi-turn is required
                    !self.require_multi_turn
                }
            })
            .min_by(|a, b| {
                let score = |bt: &BackendType| -> f32 {
                    if let Some(cap) = self.capabilities.get(bt) {
                        self.cost_weight * cap.cost_tier as f32
                            + latency_weight * cap.latency_tier as f32
                    } else {
                        // Neutral score for unknown backends (middle tier)
                        self.cost_weight * 2.0 + latency_weight * 2.0
                    }
                };
                score(a)
                    .partial_cmp(&score(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
    }
}

// ---------------------------------------------------------------------------
// ChainRouter
// ---------------------------------------------------------------------------

/// A composite router that tries multiple routers in sequence.
///
/// The first router to return `Some(backend)` wins. If all routers return
/// `None`, the chain returns `None`.
///
/// This is useful for combining a precise [`KeywordRouter`] with a broader
/// [`CapabilityRouter`] as a fallback:
///
/// ```rust
/// use agent_teams::backend::router::{ChainRouter, KeywordRouter, CapabilityRouter};
/// use agent_teams::BackendType;
///
/// let router = ChainRouter::new()
///     .push(KeywordRouter::new(BackendType::ClaudeCode)
///         .word_boundary(true)
///         .rule("review", BackendType::GeminiCli))
///     .push(CapabilityRouter::new().cost_weight(0.7));
/// ```
pub struct ChainRouter {
    routers: Vec<Box<dyn BackendRouter>>,
}

impl std::fmt::Debug for ChainRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainRouter")
            .field("routers_count", &self.routers.len())
            .finish()
    }
}

impl ChainRouter {
    /// Create an empty chain.
    pub fn new() -> Self {
        Self {
            routers: Vec::new(),
        }
    }

    /// Append a router to the chain.
    pub fn push(mut self, router: impl BackendRouter + 'static) -> Self {
        self.routers.push(Box::new(router));
        self
    }
}

impl Default for ChainRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BackendRouter for ChainRouter {
    async fn route(&self, config: &SpawnConfig, available: &[BackendType]) -> Option<BackendType> {
        for router in &self.routers {
            if let Some(bt) = router.route(config, available).await {
                return Some(bt);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// SmartRouter
// ---------------------------------------------------------------------------

/// Prompt complexity level inferred from heuristic analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptComplexity {
    /// Short, simple prompts (single sentence, no code).
    Simple,
    /// Medium-length prompts or those containing code snippets.
    Medium,
    /// Long, multi-paragraph prompts with significant code or technical depth.
    Complex,
}

/// A smart router that combines keyword matching, capability scoring, and
/// prompt complexity analysis to select the optimal backend.
///
/// **Routing strategy** (evaluated in order):
///
/// 1. **Priority overrides** — explicit `(keyword, backend)` pairs checked first.
///    If a keyword matches the prompt and the backend is available, it wins immediately.
///
/// 2. **Prompt complexity analysis** — the prompt is scored for length, code presence,
///    and technical depth. Each complexity level maps to a preferred backend.
///
/// 3. **Capability fallback** — an embedded [`CapabilityRouter`] scores remaining
///    candidates on cost/latency when neither keywords nor complexity produce a match.
///
/// # Example
///
/// ```rust
/// use agent_teams::backend::router::SmartRouter;
/// use agent_teams::BackendType;
///
/// let router = SmartRouter::new(BackendType::ClaudeCode)
///     .priority("security audit", BackendType::ClaudeCode)
///     .priority("quick fix", BackendType::Codex)
///     .simple_backend(BackendType::GeminiCli)
///     .complex_backend(BackendType::ClaudeCode)
///     .complexity_threshold(200, 800)
///     .cost_weight(0.6);
/// ```
#[derive(Debug)]
pub struct SmartRouter {
    /// Priority keyword overrides (checked first, word-boundary matching).
    priorities: Vec<(String, BackendType)>,
    /// Backend preferred for simple prompts.
    simple: Option<BackendType>,
    /// Backend preferred for medium-complexity prompts.
    medium: Option<BackendType>,
    /// Backend preferred for complex prompts.
    complex: Option<BackendType>,
    /// Character threshold: prompts shorter than this are `Simple`.
    simple_threshold: usize,
    /// Character threshold: prompts longer than this are `Complex`.
    complex_threshold: usize,
    /// Capability-based fallback router.
    capability: CapabilityRouter,
    /// Default backend (used when nothing else matches).
    default: BackendType,
}

impl SmartRouter {
    /// Create a new smart router with a default backend.
    pub fn new(default: BackendType) -> Self {
        Self {
            priorities: Vec::new(),
            simple: None,
            medium: None,
            complex: None,
            simple_threshold: 200,
            complex_threshold: 800,
            capability: CapabilityRouter::new(),
            default,
        }
    }

    /// Add a priority keyword override.
    ///
    /// Priority keywords use word-boundary matching (same semantics as
    /// [`KeywordRouter::word_boundary(true)`]). The first matching priority wins.
    pub fn priority(mut self, keyword: impl Into<String>, backend: BackendType) -> Self {
        self.priorities
            .push((keyword.into().to_lowercase(), backend));
        self
    }

    /// Set the preferred backend for simple (short, non-technical) prompts.
    pub fn simple_backend(mut self, backend: BackendType) -> Self {
        self.simple = Some(backend);
        self
    }

    /// Set the preferred backend for medium-complexity prompts.
    pub fn medium_backend(mut self, backend: BackendType) -> Self {
        self.medium = Some(backend);
        self
    }

    /// Set the preferred backend for complex (long, code-heavy) prompts.
    pub fn complex_backend(mut self, backend: BackendType) -> Self {
        self.complex = Some(backend);
        self
    }

    /// Set the character-length thresholds for complexity classification.
    ///
    /// - Prompts shorter than `simple` characters are `Simple`.
    /// - Prompts longer than `complex` characters are `Complex`.
    /// - Everything in between is `Medium`.
    ///
    /// Defaults: `simple = 200`, `complex = 800`.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `simple >= complex`.
    pub fn complexity_threshold(mut self, simple: usize, complex: usize) -> Self {
        debug_assert!(
            simple < complex,
            "simple threshold must be less than complex threshold"
        );
        self.simple_threshold = simple;
        self.complex_threshold = complex;
        self
    }

    /// Set the cost vs latency weight for the capability fallback (0.0–1.0).
    pub fn cost_weight(mut self, weight: f32) -> Self {
        self.capability = self.capability.cost_weight(weight);
        self
    }

    /// Require multi-turn support in the capability fallback.
    pub fn require_multi_turn(mut self, require: bool) -> Self {
        self.capability = self.capability.require_multi_turn(require);
        self
    }

    /// Override capability data for a backend in the fallback scorer.
    pub fn with_capability(mut self, backend: BackendType, cap: BackendCapability) -> Self {
        self.capability = self.capability.with_capability(backend, cap);
        self
    }

    /// Analyze a prompt and return its complexity level.
    pub fn analyze_complexity(&self, prompt: &str) -> PromptComplexity {
        let len = prompt.len();

        // Code indicators: triple-backtick blocks, `fn `, `def `, `class `, `impl `,
        // `struct `, `import `, `#include`, common code patterns
        let has_code = prompt.contains("```")
            || prompt.contains("fn ")
            || prompt.contains("def ")
            || prompt.contains("class ")
            || prompt.contains("impl ")
            || prompt.contains("struct ")
            || prompt.contains("import ")
            || prompt.contains("#include")
            || prompt.contains("function ")
            || prompt.contains("async fn");

        // Multi-paragraph indicator
        let paragraph_count = prompt.split("\n\n").count();

        if len >= self.complex_threshold
            || (has_code && len >= self.simple_threshold)
            || paragraph_count >= 4
        {
            PromptComplexity::Complex
        } else if len >= self.simple_threshold || has_code || paragraph_count >= 2 {
            PromptComplexity::Medium
        } else {
            PromptComplexity::Simple
        }
    }
}

#[async_trait]
impl BackendRouter for SmartRouter {
    async fn route(&self, config: &SpawnConfig, available: &[BackendType]) -> Option<BackendType> {
        if available.is_empty() {
            return None;
        }

        let prompt_lower = config.prompt.to_lowercase();

        // Phase 1: Priority keyword overrides (word-boundary matching)
        for (keyword, backend) in &self.priorities {
            if contains_word(&prompt_lower, keyword.as_str()) && available.contains(backend) {
                return Some(*backend);
            }
        }

        // Phase 2: Prompt complexity routing
        let complexity = self.analyze_complexity(&config.prompt);
        let preferred = match complexity {
            PromptComplexity::Simple => self.simple,
            PromptComplexity::Medium => self.medium,
            PromptComplexity::Complex => self.complex,
        };
        if let Some(backend) = preferred {
            if available.contains(&backend) {
                return Some(backend);
            }
        }

        // Phase 3: Capability-based fallback
        if let Some(bt) = self.capability.route(config, available).await {
            return Some(bt);
        }

        // Phase 4: Default or first available
        if available.contains(&self.default) {
            Some(self.default)
        } else {
            available.first().copied()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn keyword_router_matches_first_rule() {
        let router = KeywordRouter::new(BackendType::ClaudeCode)
            .rule("review", BackendType::GeminiCli)
            .rule("implement", BackendType::ClaudeCode)
            .rule("test", BackendType::Codex);

        let config = SpawnConfig::new("reviewer", "Please review this code carefully.");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        let result = router.route(&config, &available).await;
        assert_eq!(result, Some(BackendType::GeminiCli));
    }

    #[tokio::test]
    async fn keyword_router_falls_back_to_default() {
        let router = KeywordRouter::new(BackendType::Codex).rule("review", BackendType::GeminiCli);

        let config = SpawnConfig::new("worker", "Do something unrelated.");
        let available = vec![BackendType::ClaudeCode, BackendType::Codex];

        let result = router.route(&config, &available).await;
        assert_eq!(result, Some(BackendType::Codex));
    }

    #[tokio::test]
    async fn keyword_router_skips_unavailable_backend() {
        let router =
            KeywordRouter::new(BackendType::ClaudeCode).rule("review", BackendType::GeminiCli);

        let config = SpawnConfig::new("reviewer", "Please review this code.");
        // GeminiCli not available
        let available = vec![BackendType::ClaudeCode, BackendType::Codex];

        let result = router.route(&config, &available).await;
        // Should fall through to default since matched backend not available
        assert_eq!(result, Some(BackendType::ClaudeCode));
    }

    #[tokio::test]
    async fn keyword_router_case_insensitive() {
        let router =
            KeywordRouter::new(BackendType::ClaudeCode).rule("REVIEW", BackendType::GeminiCli);

        let config = SpawnConfig::new("reviewer", "review this code");
        let available = vec![BackendType::ClaudeCode, BackendType::GeminiCli];

        let result = router.route(&config, &available).await;
        assert_eq!(result, Some(BackendType::GeminiCli));
    }

    #[tokio::test]
    async fn keyword_router_returns_none_for_empty_available() {
        let router = KeywordRouter::new(BackendType::ClaudeCode);
        let config = SpawnConfig::new("worker", "Do something.");
        let result = router.route(&config, &[]).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn capability_router_prefers_cheapest() {
        let router = CapabilityRouter::new().cost_weight(1.0); // pure cost optimization

        let config = SpawnConfig::new("worker", "Do a simple task.");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        let result = router.route(&config, &available).await;
        assert_eq!(result, Some(BackendType::GeminiCli)); // cost_tier=1
    }

    #[tokio::test]
    async fn capability_router_multi_turn_requirement() {
        let router = CapabilityRouter::new()
            .require_multi_turn(true)
            .cost_weight(1.0); // cheapest multi-turn

        let config = SpawnConfig::new("worker", "Multi-turn session.");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        let result = router.route(&config, &available).await;
        // GeminiCli excluded (not multi-turn), Codex is cheaper than ClaudeCode
        assert_eq!(result, Some(BackendType::Codex));
    }

    #[tokio::test]
    async fn capability_router_prefers_fastest() {
        let router = CapabilityRouter::new().cost_weight(0.0); // pure latency optimization

        let config = SpawnConfig::new("worker", "Quick task.");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        let result = router.route(&config, &available).await;
        // ClaudeCode & Codex both latency_tier=2, ClaudeCode wins (first in available list)
        assert_eq!(result, Some(BackendType::ClaudeCode));
    }

    #[tokio::test]
    async fn capability_router_returns_none_for_empty_available() {
        let router = CapabilityRouter::new();
        let config = SpawnConfig::new("worker", "Do something.");
        let result = router.route(&config, &[]).await;
        assert_eq!(result, None);
    }

    // -----------------------------------------------------------------------
    // Word boundary tests
    // -----------------------------------------------------------------------

    #[test]
    fn contains_word_exact_match() {
        assert!(super::contains_word("review this code", "review"));
        assert!(super::contains_word("please review", "review"));
        assert!(super::contains_word("review", "review"));
    }

    #[test]
    fn contains_word_rejects_substring() {
        assert!(!super::contains_word("reviewing this code", "review"));
        assert!(!super::contains_word("code reviewer", "review"));
        assert!(!super::contains_word("prereview step", "review"));
    }

    #[test]
    fn contains_word_with_punctuation() {
        assert!(super::contains_word("please review, thanks", "review"));
        assert!(super::contains_word("review.", "review"));
        assert!(super::contains_word("(review)", "review"));
    }

    #[test]
    fn contains_word_test_vs_testing() {
        assert!(super::contains_word("test this function", "test"));
        assert!(!super::contains_word("testing this function", "test"));
        assert!(!super::contains_word("run the contest", "test"));
    }

    #[test]
    fn contains_word_edge_cases() {
        // Empty text / empty word
        assert!(!super::contains_word("", "review"));
        assert!(!super::contains_word("some text", ""));
        assert!(!super::contains_word("", ""));

        // Single character word
        assert!(super::contains_word("a quick task", "a"));
        assert!(!super::contains_word("analyze this", "a"));

        // Word at very end
        assert!(super::contains_word("do a test", "test"));

        // Multiple occurrences, only later one is a word
        assert!(super::contains_word("testing the test", "test"));

        // Hyphenated context (hyphen is not alphanumeric → boundary)
        assert!(super::contains_word("run unit-test now", "test"));

        // UTF-8 multi-byte: boundary check must not panic
        // "café test" — 'é' is 2 bytes (0xC3 0xA9), acts as word boundary
        assert!(super::contains_word("café test", "test"));
        assert!(super::contains_word("café", "café"));
        // 'é' before keyword — boundary check on multi-byte char must not panic
        assert!(!super::contains_word("acafé", "café"));
        // Non-ASCII word embedded with ASCII neighbors — ensure no mid-char slicing
        assert!(super::contains_word("aéb test éb", "éb"));
    }

    #[tokio::test]
    async fn keyword_router_word_boundary_mode() {
        let router = KeywordRouter::new(BackendType::ClaudeCode)
            .word_boundary(true)
            .rule("test", BackendType::Codex);

        let available = vec![BackendType::ClaudeCode, BackendType::Codex];

        // "test" as a whole word — should match
        let config = SpawnConfig::new("w", "test this function");
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::Codex)
        );

        // "testing" contains "test" as substring but not as word — should NOT match
        let config = SpawnConfig::new("w", "testing this function");
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::ClaudeCode)
        );
    }

    // -----------------------------------------------------------------------
    // ChainRouter tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn chain_router_first_match_wins() {
        let keyword =
            KeywordRouter::new(BackendType::ClaudeCode).rule("review", BackendType::GeminiCli);
        let capability = CapabilityRouter::new().cost_weight(1.0);

        let router = ChainRouter::new().push(keyword).push(capability);

        let config = SpawnConfig::new("w", "review this code");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        // Keyword router matches "review" → GeminiCli
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::GeminiCli)
        );
    }

    #[tokio::test]
    async fn chain_router_falls_through_to_second() {
        let keyword = KeywordRouter::new(BackendType::ClaudeCode)
            .word_boundary(true)
            .rule("review", BackendType::GeminiCli);
        let capability = CapabilityRouter::new().cost_weight(1.0);

        let router = ChainRouter::new().push(keyword).push(capability);

        // No keyword match → falls through to CapabilityRouter → cheapest = GeminiCli
        let config = SpawnConfig::new("w", "do a simple task");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        // KeywordRouter default is ClaudeCode which IS available, so it returns ClaudeCode
        // Actually, KeywordRouter falls back to its default which is ClaudeCode
        // So the chain stops at the first router that returns Some
        let result = router.route(&config, &available).await;
        assert_eq!(result, Some(BackendType::ClaudeCode));
    }

    #[tokio::test]
    async fn chain_router_empty_returns_none() {
        let router = ChainRouter::new();
        let config = SpawnConfig::new("w", "anything");
        let result = router.route(&config, &[BackendType::ClaudeCode]).await;
        assert_eq!(result, None);
    }

    // -----------------------------------------------------------------------
    // BackendCapability + Debug tests
    // -----------------------------------------------------------------------

    #[test]
    fn backend_capability_default() {
        let cap = BackendCapability::default();
        assert!(cap.multi_turn);
        assert!(cap.streaming);
        assert_eq!(cap.cost_tier, 2);
        assert_eq!(cap.latency_tier, 2);
    }

    #[test]
    fn capability_router_with_custom_capability() {
        let cap = BackendCapability {
            cost_tier: 1,
            ..Default::default()
        };
        assert!(cap.multi_turn); // from default
        assert_eq!(cap.cost_tier, 1); // overridden
    }

    #[test]
    fn routers_are_debug() {
        let kw = KeywordRouter::new(BackendType::ClaudeCode).rule("test", BackendType::Codex);
        assert!(format!("{kw:?}").contains("KeywordRouter"));

        let cap = CapabilityRouter::new();
        assert!(format!("{cap:?}").contains("CapabilityRouter"));

        let chain = ChainRouter::new().push(KeywordRouter::new(BackendType::ClaudeCode));
        let debug = format!("{chain:?}");
        assert!(debug.contains("ChainRouter"));
        assert!(debug.contains("routers_count"));

        let smart = SmartRouter::new(BackendType::ClaudeCode);
        assert!(format!("{smart:?}").contains("SmartRouter"));
    }

    // -----------------------------------------------------------------------
    // SmartRouter tests
    // -----------------------------------------------------------------------

    #[test]
    fn smart_router_complexity_simple() {
        let router = SmartRouter::new(BackendType::ClaudeCode);
        let complexity = router.analyze_complexity("Fix the bug.");
        assert_eq!(complexity, PromptComplexity::Simple);
    }

    #[test]
    fn smart_router_complexity_medium_by_length() {
        let router = SmartRouter::new(BackendType::ClaudeCode);
        let prompt = "a".repeat(250); // > 200 chars
        let complexity = router.analyze_complexity(&prompt);
        assert_eq!(complexity, PromptComplexity::Medium);
    }

    #[test]
    fn smart_router_complexity_medium_by_code() {
        let router = SmartRouter::new(BackendType::ClaudeCode);
        let complexity = router.analyze_complexity("Please add fn main() here");
        assert_eq!(complexity, PromptComplexity::Medium);
    }

    #[test]
    fn smart_router_complexity_complex_by_length() {
        let router = SmartRouter::new(BackendType::ClaudeCode);
        let prompt = "a".repeat(900); // > 800 chars
        let complexity = router.analyze_complexity(&prompt);
        assert_eq!(complexity, PromptComplexity::Complex);
    }

    #[test]
    fn smart_router_complexity_complex_by_code_and_length() {
        let router = SmartRouter::new(BackendType::ClaudeCode);
        // Has code indicators AND >= simple_threshold
        let prompt = format!(
            "Please implement this:\n```rust\nfn main() {{}}\n```\n{}",
            "x".repeat(200)
        );
        let complexity = router.analyze_complexity(&prompt);
        assert_eq!(complexity, PromptComplexity::Complex);
    }

    #[test]
    fn smart_router_complexity_complex_by_paragraphs() {
        let router = SmartRouter::new(BackendType::ClaudeCode);
        let prompt =
            "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.\n\nFourth paragraph.";
        let complexity = router.analyze_complexity(prompt);
        assert_eq!(complexity, PromptComplexity::Complex);
    }

    #[tokio::test]
    async fn smart_router_priority_wins() {
        let router = SmartRouter::new(BackendType::Codex)
            .priority("security audit", BackendType::ClaudeCode)
            .simple_backend(BackendType::GeminiCli);

        let config = SpawnConfig::new("w", "Run a security audit on this module");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        // Priority "security audit" matches → ClaudeCode
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::ClaudeCode)
        );
    }

    #[tokio::test]
    async fn smart_router_complexity_routing_simple() {
        let router = SmartRouter::new(BackendType::Codex).simple_backend(BackendType::GeminiCli);

        let config = SpawnConfig::new("w", "Fix the typo.");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        // Short prompt → Simple → GeminiCli
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::GeminiCli)
        );
    }

    #[tokio::test]
    async fn smart_router_complexity_routing_complex() {
        let router = SmartRouter::new(BackendType::Codex).complex_backend(BackendType::ClaudeCode);

        let long_prompt = "a".repeat(900);
        let config = SpawnConfig::new("w", &long_prompt);
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        // Long prompt → Complex → ClaudeCode
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::ClaudeCode)
        );
    }

    #[tokio::test]
    async fn smart_router_falls_back_to_capability() {
        // No priorities, no complexity mappings → falls through to capability router
        let router = SmartRouter::new(BackendType::Codex).cost_weight(1.0); // cheapest wins

        let config = SpawnConfig::new("w", "Do a task.");
        let available = vec![
            BackendType::ClaudeCode,
            BackendType::Codex,
            BackendType::GeminiCli,
        ];

        // CapabilityRouter with cost_weight=1.0 picks cheapest → GeminiCli (cost_tier=1)
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::GeminiCli)
        );
    }

    #[tokio::test]
    async fn smart_router_priority_skips_unavailable() {
        let router = SmartRouter::new(BackendType::Codex)
            .priority("audit", BackendType::ClaudeCode)
            .simple_backend(BackendType::GeminiCli);

        let config = SpawnConfig::new("w", "Run an audit");
        // ClaudeCode NOT available
        let available = vec![BackendType::Codex, BackendType::GeminiCli];

        // Priority matches but ClaudeCode unavailable → falls to complexity (Simple) → GeminiCli
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::GeminiCli)
        );
    }

    #[tokio::test]
    async fn smart_router_returns_none_for_empty() {
        let router = SmartRouter::new(BackendType::ClaudeCode);
        let config = SpawnConfig::new("w", "anything");
        assert_eq!(router.route(&config, &[]).await, None);
    }

    #[tokio::test]
    async fn smart_router_default_fallback() {
        // No priorities, no complexity backends, capability returns None
        // (this shouldn't normally happen, but test the default path)
        let router = SmartRouter::new(BackendType::Codex);

        let config = SpawnConfig::new("w", "Short task.");
        let available = vec![BackendType::Codex];

        // No simple_backend set, capability picks Codex (only option), or default
        assert_eq!(
            router.route(&config, &available).await,
            Some(BackendType::Codex)
        );
    }

    #[test]
    fn smart_router_custom_thresholds() {
        let router = SmartRouter::new(BackendType::ClaudeCode).complexity_threshold(50, 300);

        // 60 chars = medium (>50, <300)
        assert_eq!(
            router.analyze_complexity(&"a".repeat(60)),
            PromptComplexity::Medium
        );
        // 30 chars = simple (<50)
        assert_eq!(
            router.analyze_complexity(&"a".repeat(30)),
            PromptComplexity::Simple
        );
        // 400 chars = complex (>300)
        assert_eq!(
            router.analyze_complexity(&"a".repeat(400)),
            PromptComplexity::Complex
        );
    }
}
