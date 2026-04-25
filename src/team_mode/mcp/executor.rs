use serde_json::Value;

use crate::error::Result;
use crate::team_mode::mcp::schemas::ToolDescriptor;
use crate::team_mode::mcp::tools::{TeamModeToolset, ToolExecution};

pub trait TeamModeToolExecutor: Send {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>>;
    fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<ToolExecution>;
}

impl TeamModeToolExecutor for TeamModeToolset {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(TeamModeToolset::list_tools(self))
    }

    fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<ToolExecution> {
        TeamModeToolset::call_tool(self, name, arguments)
    }
}
