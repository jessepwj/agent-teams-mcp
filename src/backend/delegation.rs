//! CLI delegation support for Claude Code agents.
//!
//! Allows Claude agents to delegate work to external CLI tools (Codex, Gemini, etc.)
//! via Bash tool calls. The delegation system injects instructions into the agent's
//! prompt that teach it how and when to invoke each CLI tool.
//!
//! # Architecture
//!
//! Delegations are configured on [`SpawnConfig`](super::SpawnConfig) via the builder's
//! [`.delegate()`](super::SpawnConfigBuilder::delegate) method. When the orchestrator
//! spawns the agent, it calls [`format_delegation_prompt`] to generate instructions
//! and prepends them to the agent's system prompt *before* passing it to the backend.
//!
//! This means the agent is still a Claude agent — it just knows how to shell out to
//! other tools when appropriate.
//!
//! # Example
//!
//! ```rust
//! use agent_teams::backend::delegation::CliDelegation;
//! use agent_teams::SpawnConfig;
//!
//! let config = SpawnConfig::builder("reviewer", "You review Rust code for correctness.")
//!     .delegate(CliDelegation::gemini("gemini-2.5-pro")
//!         .with_description("Use for architectural analysis and design review"))
//!     .delegate(CliDelegation::codex()
//!         .with_description("Use for generating test cases"))
//!     .build();
//! ```

/// Which CLI tool to delegate to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliTool {
    /// OpenAI Codex CLI (`codex`).
    Codex,
    /// Google Gemini CLI (`gemini`).
    Gemini,
    /// A custom CLI tool with a user-provided name and binary path.
    Custom {
        /// Human-readable name shown in the prompt heading.
        name: String,
        /// Binary name or path used in the Bash command.
        binary: String,
    },
}

impl std::fmt::Display for CliTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliTool::Codex => write!(f, "Codex"),
            CliTool::Gemini => write!(f, "Gemini"),
            CliTool::Custom { name, .. } => write!(f, "{name}"),
        }
    }
}

/// Configuration for a single CLI delegation.
///
/// Each delegation describes one external tool that the Claude agent can invoke
/// via Bash. The [`to_prompt_instructions`](Self::to_prompt_instructions) method
/// generates the prompt fragment that teaches the agent how to use the tool.
#[derive(Debug, Clone)]
pub struct CliDelegation {
    /// Which CLI tool to delegate to.
    pub tool: CliTool,
    /// Optional model override (e.g., `"gemini-2.5-pro"`).
    pub model: Option<String>,
    /// When to use this tool (injected into the prompt).
    pub description: String,
    /// Custom Bash usage template. If `None`, a sensible default is generated.
    pub usage_hint: Option<String>,
}

impl CliDelegation {
    /// Pre-configured Codex delegation with sensible defaults.
    ///
    /// ```rust
    /// use agent_teams::backend::delegation::CliDelegation;
    ///
    /// let d = CliDelegation::codex();
    /// assert_eq!(d.tool, agent_teams::backend::delegation::CliTool::Codex);
    /// ```
    pub fn codex() -> Self {
        Self {
            tool: CliTool::Codex,
            model: None,
            description:
                "When you need AI-powered code generation or implementation, use Codex via Bash."
                    .into(),
            usage_hint: None,
        }
    }

    /// Pre-configured Gemini delegation with model selection.
    ///
    /// ```rust
    /// use agent_teams::backend::delegation::CliDelegation;
    ///
    /// let d = CliDelegation::gemini("gemini-2.5-pro");
    /// assert_eq!(d.model.as_deref(), Some("gemini-2.5-pro"));
    /// ```
    pub fn gemini(model: &str) -> Self {
        Self {
            tool: CliTool::Gemini,
            model: Some(model.to_string()),
            description: "When you need AI-powered analysis or review, use Gemini via Bash.".into(),
            usage_hint: None,
        }
    }

    /// Custom CLI tool delegation.
    ///
    /// ```rust
    /// use agent_teams::backend::delegation::CliDelegation;
    ///
    /// let d = CliDelegation::custom("Aider", "aider");
    /// assert_eq!(d.tool.to_string(), "Aider");
    /// ```
    pub fn custom(name: &str, binary: &str) -> Self {
        Self {
            tool: CliTool::Custom {
                name: name.to_string(),
                binary: binary.to_string(),
            },
            model: None,
            description: format!("When appropriate, use {name} via Bash."),
            usage_hint: None,
        }
    }

    /// Override when to use this tool.
    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = desc.to_string();
        self
    }

    /// Override the default Bash usage template.
    pub fn with_usage_hint(mut self, hint: &str) -> Self {
        self.usage_hint = Some(hint.to_string());
        self
    }

    /// Set a model override (mainly useful for Gemini or custom tools).
    pub fn with_model(mut self, model: &str) -> Self {
        self.model = Some(model.to_string());
        self
    }

    /// Generate the prompt instructions for this single delegation.
    pub fn to_prompt_instructions(&self) -> String {
        let tool_name = &self.tool;

        let model_suffix = self
            .model
            .as_ref()
            .map(|m| format!(" ({m})"))
            .unwrap_or_default();

        let usage = self
            .usage_hint
            .clone()
            .unwrap_or_else(|| self.default_usage_hint());

        format!(
            "## CLI Delegation: {tool_name}{model_suffix}\n\
             {description}\n\
             ```bash\n\
             {usage}\n\
             ```",
            description = self.description,
        )
    }

    /// Generate the default usage hint based on the tool type.
    fn default_usage_hint(&self) -> String {
        match &self.tool {
            CliTool::Codex => "codex -q \"YOUR_PROMPT\" --approval-mode full-auto".to_string(),
            CliTool::Gemini => {
                let model_flag = self
                    .model
                    .as_ref()
                    .map(|m| format!("-m {m} "))
                    .unwrap_or_default();
                format!("gemini {model_flag}-y <<'PROMPT'\nYour prompt here\nPROMPT")
            }
            CliTool::Custom { binary, .. } => {
                format!("{binary} \"YOUR_PROMPT\"")
            }
        }
    }
}

/// Generate combined delegation instructions for multiple tools.
///
/// Returns an empty string if the slice is empty. Otherwise, produces a
/// Markdown document with a heading and one section per delegation.
pub fn format_delegation_prompt(delegations: &[CliDelegation]) -> String {
    if delegations.is_empty() {
        return String::new();
    }

    let mut sections = Vec::with_capacity(delegations.len() + 1);
    sections.push(
        "# CLI Tool Delegations\n\n\
         You have access to the following CLI tools via Bash. \
         Use them when appropriate for your tasks."
            .to_string(),
    );

    for d in delegations {
        sections.push(d.to_prompt_instructions());
    }

    sections.join("\n\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_default_prompt() {
        let d = CliDelegation::codex();
        let prompt = d.to_prompt_instructions();

        assert!(prompt.contains("## CLI Delegation: Codex"));
        assert!(prompt.contains("codex -q"));
        assert!(prompt.contains("--approval-mode full-auto"));
        assert!(prompt.contains("code generation"));
    }

    #[test]
    fn gemini_with_model() {
        let d = CliDelegation::gemini("gemini-2.5-pro");
        let prompt = d.to_prompt_instructions();

        assert!(prompt.contains("## CLI Delegation: Gemini (gemini-2.5-pro)"));
        assert!(prompt.contains("-m gemini-2.5-pro"));
        assert!(prompt.contains("gemini"));
        assert!(prompt.contains("PROMPT"));
    }

    #[test]
    fn gemini_custom_description() {
        let d = CliDelegation::gemini("gemini-2.5-flash")
            .with_description("Use for quick architectural reviews only.");
        let prompt = d.to_prompt_instructions();

        assert!(prompt.contains("quick architectural reviews"));
        assert!(!prompt.contains("analysis or review")); // default replaced
    }

    #[test]
    fn custom_tool() {
        let d = CliDelegation::custom("Aider", "aider");
        let prompt = d.to_prompt_instructions();

        assert!(prompt.contains("## CLI Delegation: Aider"));
        assert!(prompt.contains("aider \"YOUR_PROMPT\""));
    }

    #[test]
    fn custom_tool_with_usage_hint() {
        let d = CliDelegation::custom("Aider", "aider")
            .with_usage_hint("aider --model gpt-4.1 --yes-always \"YOUR_PROMPT\"");
        let prompt = d.to_prompt_instructions();

        assert!(prompt.contains("--yes-always"));
        assert!(!prompt.contains("aider \"YOUR_PROMPT\"")); // default replaced
    }

    #[test]
    fn custom_tool_with_model() {
        let d = CliDelegation::custom("MyTool", "mytool").with_model("v2");
        let prompt = d.to_prompt_instructions();

        assert!(prompt.contains("## CLI Delegation: MyTool (v2)"));
    }

    #[test]
    fn format_delegation_prompt_empty() {
        assert_eq!(format_delegation_prompt(&[]), "");
    }

    #[test]
    fn format_delegation_prompt_single() {
        let delegations = vec![CliDelegation::codex()];
        let prompt = format_delegation_prompt(&delegations);

        assert!(prompt.starts_with("# CLI Tool Delegations"));
        assert!(prompt.contains("## CLI Delegation: Codex"));
    }

    #[test]
    fn format_delegation_prompt_multiple() {
        let delegations = vec![
            CliDelegation::codex(),
            CliDelegation::gemini("gemini-2.5-pro"),
            CliDelegation::custom("Aider", "aider"),
        ];
        let prompt = format_delegation_prompt(&delegations);

        assert!(prompt.contains("# CLI Tool Delegations"));
        assert!(prompt.contains("## CLI Delegation: Codex"));
        assert!(prompt.contains("## CLI Delegation: Gemini (gemini-2.5-pro)"));
        assert!(prompt.contains("## CLI Delegation: Aider"));
    }

    #[test]
    fn cli_tool_display() {
        assert_eq!(CliTool::Codex.to_string(), "Codex");
        assert_eq!(CliTool::Gemini.to_string(), "Gemini");
        assert_eq!(
            CliTool::Custom {
                name: "Aider".into(),
                binary: "aider".into()
            }
            .to_string(),
            "Aider"
        );
    }

    #[test]
    fn cli_delegation_clone_and_debug() {
        let d = CliDelegation::codex();
        let d2 = d.clone();
        assert_eq!(d.tool, d2.tool);

        let debug = format!("{d:?}");
        assert!(debug.contains("CliDelegation"));
    }
}
