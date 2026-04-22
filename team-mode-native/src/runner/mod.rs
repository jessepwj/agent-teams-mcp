pub mod control_client;
pub mod input_injector;
pub mod output_log;
pub mod protocol;
pub mod pty_bridge;

pub use control_client::{RunnerControlClient, send_ndjson_frame};
pub use input_injector::{InjectionStrategy, format_injected_input};
pub use protocol::{
    ChildExitFrame, HostToRunnerFrame, InjectInputFrame, InputInjectedFrame, RunnerFrame,
    RunnerHeartbeatFrame, RunnerHelloFrame, RunnerOutputFrame,
};
pub use pty_bridge::{
    PtyBridge, PtyCommandSpec, PtyEvent, command_spec_from_parts, spawn_direct_bridge,
    spawn_pty_bridge,
};
