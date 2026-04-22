//! # agent-teams
//!
//! This crate is migrating toward a Team Mode MCP runtime.
//! The new `team_mode` and `runtime` modules are the canonical direction;
//! the legacy modules remain exported for now so the transition can be incremental.

#![warn(clippy::all)]

pub mod error;
pub mod models;
pub mod runtime;
pub mod team_mode;
pub mod util;

// Legacy workflow-oriented modules retained during the migration.
pub mod messaging;
pub mod task;
pub mod team;

pub mod consensus;
pub mod memory;

pub mod backend;
pub mod orchestrator;

#[cfg(feature = "dashboard")]
pub mod dashboard;

#[cfg(feature = "checkpoint")]
pub mod checkpoint;

#[cfg(feature = "tui")]
pub mod tui;

pub use backend::delegation::{CliDelegation, CliTool};
pub use backend::{
    AgentBackend, AgentOutput, AgentOutputStream, AgentSession, BackendType, SpawnConfig,
    SpawnConfigBuilder,
};
pub use consensus::{AgentResponse, ConsensusRequest, ConsensusResult, ConsensusStrategy};
pub use error::{Error, Result};
pub use memory::{ConversationMemory, MemoryConfig, MemoryManager, Role, TurnRecord};
pub use models::{AgentTokenUsage, CostSummary, TokenUsage, ToolCallRecord};
pub use models::{
    CreateTaskRequest, InboxMessage, SessionState, TaskFile, TaskFilter, TaskStatus, TaskUpdate,
    TeamConfig,
};
pub use orchestrator::TeamOrchestrator;
pub use runtime::{
    ExecutionSessionState, ManagedMemberHandle, RuntimeOrchestrator, SessionRegistry,
};
pub use team_mode::domain::{
    AudiencePolicy, DeliveryDrop, DeliveryStatus, DropReason, ExecutionMode, ExecutionProfile,
    Inbox, InboxItem, InboxStatus, MemberKind, MemberProfile, Message, MessageKind, MessageReceipt,
    Room, RoomKind, RoomStatus, Team, TeamStatus, Thread, ThreadStatus, VisibilityReason,
    VisibilityResolution, VisibilityRule,
};
pub use team_mode::mcp::{
    InitializeResult, JsonRpcErrorObject, JsonRpcErrorResponse, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, ListResourcesResult, ListToolsResult, ReadResourceResult,
    ResourceDescriptor, ResourcesUpdatedParams, TeamModeMcpRuntime, TeamModeResourceRegistry,
    TeamModeToolset, TextResourceContents, ToolCallResult, ToolDescriptor,
};
pub use team_mode::service::{
    AddMemberRequest, CreateTeamRequest, InboxCount, InboxService, MemberRecord, MemberService,
    MessageService, ReplyToThreadRequest, RoomService, SendMessageRequest, TeamService,
    ThreadService, UpdateMemberRequest,
};
pub use team_mode::storage::{MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore};

/// Convenience re-exports for common usage patterns.
///
/// ```rust
/// use agent_teams::prelude::*;
/// ```
pub mod prelude {
    pub use crate::backend::claude_code::ClaudeCodeBackend;
    pub use crate::backend::codex::CodexBackend;
    pub use crate::backend::gemini::GeminiCliBackend;
    pub use crate::backend::{
        AgentBackend, AgentOutput, AgentOutputStream, AgentSession, BackendType, SpawnConfig,
        SpawnConfigBuilder,
    };
    pub use crate::error::{Error, Result};
    pub use crate::models::token::{
        AgentTokenUsage, CostSummary, TokenUsage, ToolCallRecord, estimate_cost,
    };
    pub use crate::runtime::{
        ExecutionSessionState, ManagedMemberHandle, RuntimeOrchestrator, SessionRegistry,
    };
    pub use crate::team_mode::domain::{
        AudiencePolicy, DeliveryDrop, DeliveryStatus, DropReason, ExecutionMode, ExecutionProfile,
        Inbox, InboxItem, InboxStatus, MemberKind, MemberProfile, Message, MessageKind,
        MessageReceipt, Room, RoomKind, RoomStatus, Team, TeamStatus, Thread, ThreadStatus,
        VisibilityReason, VisibilityResolution, VisibilityRule,
    };
    pub use crate::team_mode::mcp::{
        InitializeResult, JsonRpcErrorObject, JsonRpcErrorResponse, JsonRpcNotification,
        JsonRpcRequest, JsonRpcResponse, ListResourcesResult, ListToolsResult, ReadResourceResult,
        ResourceDescriptor, ResourcesUpdatedParams, TeamModeMcpRuntime, TeamModeResourceRegistry,
        TeamModeToolset, TextResourceContents, ToolCallResult, ToolDescriptor,
    };
    pub use crate::team_mode::service::{
        AddMemberRequest, CreateTeamRequest, InboxCount, InboxService, MemberRecord, MemberService,
        MessageService, ReplyToThreadRequest, RoomService, SendMessageRequest, TeamService,
        ThreadService, UpdateMemberRequest,
    };
    pub use crate::team_mode::storage::{
        MemberStore, MessageStore, ProjectionStore, RoomStore, TeamStore,
    };
}

/// Legacy workflow-oriented re-exports retained during the migration.
pub mod legacy_prelude {
    pub use crate::backend::claude_code::ClaudeCodeBackend;
    pub use crate::backend::codex::CodexBackend;
    pub use crate::backend::delegation::{CliDelegation, CliTool};
    pub use crate::backend::gemini::GeminiCliBackend;
    pub use crate::backend::router::{
        BackendRouter, CapabilityRouter, ChainRouter, KeywordRouter, PromptComplexity, SmartRouter,
    };
    pub use crate::consensus::{
        AgentResponse, ConsensusRequest, ConsensusResult, ConsensusStrategy,
    };
    pub use crate::memory::{ConversationMemory, MemoryConfig, MemoryManager};
    pub use crate::models::{
        CreateTaskRequest, InboxMessage, SessionState, TaskFile, TaskFilter, TaskStatus,
        TaskUpdate, TeamConfig,
    };
    pub use crate::orchestrator::TeamOrchestrator;
    pub use crate::task::DependencyGraph;

    #[cfg(feature = "checkpoint")]
    pub use crate::checkpoint::{
        AutoCheckpointTrigger, CheckpointCollector, CheckpointDiff, CheckpointFilter,
        CheckpointQuery, CheckpointStore,
    };
    #[cfg(feature = "checkpoint")]
    pub use crate::models::checkpoint::{Checkpoint, CheckpointFile, CheckpointSession, FileRole};
}
