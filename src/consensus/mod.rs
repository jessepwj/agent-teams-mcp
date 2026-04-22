//! Consensus protocol for multi-agent agreement.
//!
//! When multiple agents answer the same question, the consensus module
//! provides strategies to select or combine their responses into a single
//! decision.
//!
//! All resolution functions are **pure** (no I/O) and fully testable.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Strategy for reaching consensus among agent responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsensusStrategy {
    /// Most common answer wins (requires strict majority: > n/2).
    Majority,
    /// Responses weighted by agent trust/confidence scores.
    Weighted,
    /// All non-timed-out responses must agree.
    Unanimous,
    /// Present all responses to a human for final decision.
    HumanInTheLoop,
}

/// A request to reach consensus among a set of agents.
#[derive(Debug, Clone)]
pub struct ConsensusRequest {
    /// The prompt / question sent to all agents.
    pub prompt: String,
    /// Agent names that should participate.
    pub agents: Vec<String>,
    /// Strategy to use for resolution.
    pub strategy: ConsensusStrategy,
    /// Maximum time to wait for all responses.
    pub timeout: Duration,
    /// Per-agent weights (used with [`ConsensusStrategy::Weighted`]).
    pub weights: Option<HashMap<String, f32>>,
}

/// A single agent's response to the consensus prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Agent name.
    pub agent: String,
    /// The agent's answer content.
    pub content: String,
    /// Weight / confidence (default 1.0).
    pub weight: f32,
    /// Whether this agent timed out (response may be empty/partial).
    pub timed_out: bool,
}

/// Result of a consensus resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusResult {
    /// The chosen answer (if consensus was reached).
    pub decision: Option<String>,
    /// All collected responses.
    pub responses: Vec<AgentResponse>,
    /// Which strategy was used.
    pub strategy_used: ConsensusStrategy,
    /// Whether consensus was actually reached.
    pub consensus_reached: bool,
}

// ---------------------------------------------------------------------------
// Resolution functions (pure, no I/O)
// ---------------------------------------------------------------------------

/// Dispatch to the appropriate resolution function.
pub fn resolve(strategy: ConsensusStrategy, responses: &[AgentResponse]) -> ConsensusResult {
    match strategy {
        ConsensusStrategy::Majority => resolve_majority(responses),
        ConsensusStrategy::Weighted => resolve_weighted(responses),
        ConsensusStrategy::Unanimous => resolve_unanimous(responses),
        ConsensusStrategy::HumanInTheLoop => resolve_human_in_the_loop(responses),
    }
}

/// Majority vote: group by exact content match, most votes wins.
///
/// Consensus is reached only if the winner has a strict majority (> n/2)
/// among non-timed-out responses.
pub fn resolve_majority(responses: &[AgentResponse]) -> ConsensusResult {
    let active: Vec<&AgentResponse> = responses.iter().filter(|r| !r.timed_out).collect();

    if active.is_empty() {
        return ConsensusResult {
            decision: None,
            responses: responses.to_vec(),
            strategy_used: ConsensusStrategy::Majority,
            consensus_reached: false,
        };
    }

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for r in &active {
        *counts.entry(r.content.as_str()).or_insert(0) += 1;
    }

    let (winner, max_count) = counts
        .iter()
        .max_by_key(|&(_, &count)| count)
        .map(|(&content, &count)| (content.to_string(), count))
        .unwrap();

    let threshold = active.len() / 2 + 1;
    let consensus_reached = max_count >= threshold;

    ConsensusResult {
        decision: if consensus_reached {
            Some(winner)
        } else {
            None
        },
        responses: responses.to_vec(),
        strategy_used: ConsensusStrategy::Majority,
        consensus_reached,
    }
}

/// Weighted vote: group by content, sum weights, highest total wins.
pub fn resolve_weighted(responses: &[AgentResponse]) -> ConsensusResult {
    let active: Vec<&AgentResponse> = responses.iter().filter(|r| !r.timed_out).collect();

    if active.is_empty() {
        return ConsensusResult {
            decision: None,
            responses: responses.to_vec(),
            strategy_used: ConsensusStrategy::Weighted,
            consensus_reached: false,
        };
    }

    let mut weight_sums: HashMap<&str, f32> = HashMap::new();
    for r in &active {
        *weight_sums.entry(r.content.as_str()).or_insert(0.0) += r.weight;
    }

    let (winner, _) = weight_sums
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(&content, &weight)| (content.to_string(), weight))
        .unwrap();

    ConsensusResult {
        decision: Some(winner),
        responses: responses.to_vec(),
        strategy_used: ConsensusStrategy::Weighted,
        consensus_reached: true,
    }
}

/// Unanimous: all non-timed-out responses must have identical content.
pub fn resolve_unanimous(responses: &[AgentResponse]) -> ConsensusResult {
    let active: Vec<&AgentResponse> = responses.iter().filter(|r| !r.timed_out).collect();

    if active.is_empty() {
        return ConsensusResult {
            decision: None,
            responses: responses.to_vec(),
            strategy_used: ConsensusStrategy::Unanimous,
            consensus_reached: false,
        };
    }

    let first = &active[0].content;
    let all_agree = active.iter().all(|r| r.content == *first);

    ConsensusResult {
        decision: if all_agree { Some(first.clone()) } else { None },
        responses: responses.to_vec(),
        strategy_used: ConsensusStrategy::Unanimous,
        consensus_reached: all_agree,
    }
}

/// Human-in-the-loop: return all responses without making a decision.
pub fn resolve_human_in_the_loop(responses: &[AgentResponse]) -> ConsensusResult {
    ConsensusResult {
        decision: None,
        responses: responses.to_vec(),
        strategy_used: ConsensusStrategy::HumanInTheLoop,
        consensus_reached: false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_response(agent: &str, content: &str) -> AgentResponse {
        AgentResponse {
            agent: agent.to_string(),
            content: content.to_string(),
            weight: 1.0,
            timed_out: false,
        }
    }

    fn make_weighted_response(agent: &str, content: &str, weight: f32) -> AgentResponse {
        AgentResponse {
            agent: agent.to_string(),
            content: content.to_string(),
            weight,
            timed_out: false,
        }
    }

    fn make_timed_out(agent: &str) -> AgentResponse {
        AgentResponse {
            agent: agent.to_string(),
            content: String::new(),
            weight: 1.0,
            timed_out: true,
        }
    }

    #[test]
    fn majority_clear_winner() {
        let responses = vec![
            make_response("a1", "yes"),
            make_response("a2", "yes"),
            make_response("a3", "no"),
        ];
        let result = resolve_majority(&responses);

        assert!(result.consensus_reached);
        assert_eq!(result.decision, Some("yes".to_string()));
        assert_eq!(result.strategy_used, ConsensusStrategy::Majority);
    }

    #[test]
    fn majority_no_consensus() {
        // 2 vs 2 — no strict majority
        let responses = vec![
            make_response("a1", "yes"),
            make_response("a2", "no"),
            make_response("a3", "yes"),
            make_response("a4", "no"),
        ];
        let result = resolve_majority(&responses);

        assert!(!result.consensus_reached);
        assert!(result.decision.is_none());
    }

    #[test]
    fn majority_with_timeouts() {
        // 2 active, 1 timed out — "yes" has 2/2 active = clear majority
        let responses = vec![
            make_response("a1", "yes"),
            make_response("a2", "yes"),
            make_timed_out("a3"),
        ];
        let result = resolve_majority(&responses);

        assert!(result.consensus_reached);
        assert_eq!(result.decision, Some("yes".to_string()));
    }

    #[test]
    fn unanimous_all_agree() {
        let responses = vec![
            make_response("a1", "42"),
            make_response("a2", "42"),
            make_response("a3", "42"),
        ];
        let result = resolve_unanimous(&responses);

        assert!(result.consensus_reached);
        assert_eq!(result.decision, Some("42".to_string()));
    }

    #[test]
    fn unanimous_disagreement() {
        let responses = vec![make_response("a1", "42"), make_response("a2", "43")];
        let result = resolve_unanimous(&responses);

        assert!(!result.consensus_reached);
        assert!(result.decision.is_none());
    }

    #[test]
    fn weighted_higher_weight_wins() {
        let responses = vec![
            make_weighted_response("expert", "A", 5.0),
            make_weighted_response("junior1", "B", 1.0),
            make_weighted_response("junior2", "B", 1.0),
        ];
        let result = resolve_weighted(&responses);

        assert!(result.consensus_reached);
        assert_eq!(result.decision, Some("A".to_string()));
    }

    #[test]
    fn human_in_the_loop_no_decision() {
        let responses = vec![
            make_response("a1", "option A"),
            make_response("a2", "option B"),
        ];
        let result = resolve_human_in_the_loop(&responses);

        assert!(!result.consensus_reached);
        assert!(result.decision.is_none());
        assert_eq!(result.responses.len(), 2);
    }

    #[test]
    fn all_timed_out() {
        let responses = vec![make_timed_out("a1"), make_timed_out("a2")];

        let majority = resolve_majority(&responses);
        assert!(!majority.consensus_reached);
        assert!(majority.decision.is_none());

        let unanimous = resolve_unanimous(&responses);
        assert!(!unanimous.consensus_reached);

        let weighted = resolve_weighted(&responses);
        assert!(!weighted.consensus_reached);
    }

    #[test]
    fn serde_round_trip() {
        let result = ConsensusResult {
            decision: Some("answer".to_string()),
            responses: vec![make_response("a1", "answer")],
            strategy_used: ConsensusStrategy::Majority,
            consensus_reached: true,
        };

        let json = serde_json::to_string_pretty(&result).unwrap();
        let parsed: ConsensusResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.decision, Some("answer".to_string()));
        assert!(parsed.consensus_reached);
        assert_eq!(parsed.strategy_used, ConsensusStrategy::Majority);
        assert_eq!(parsed.responses.len(), 1);
        assert_eq!(parsed.responses[0].agent, "a1");
    }

    #[test]
    fn resolve_dispatch() {
        let responses = vec![
            make_response("a1", "yes"),
            make_response("a2", "yes"),
            make_response("a3", "no"),
        ];

        let r1 = resolve(ConsensusStrategy::Majority, &responses);
        assert_eq!(r1.strategy_used, ConsensusStrategy::Majority);
        assert!(r1.consensus_reached);

        let r2 = resolve(ConsensusStrategy::Unanimous, &responses);
        assert_eq!(r2.strategy_used, ConsensusStrategy::Unanimous);
        assert!(!r2.consensus_reached);

        let r3 = resolve(ConsensusStrategy::HumanInTheLoop, &responses);
        assert_eq!(r3.strategy_used, ConsensusStrategy::HumanInTheLoop);
        assert!(!r3.consensus_reached);
    }
}
