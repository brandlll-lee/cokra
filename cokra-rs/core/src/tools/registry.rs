// Tool Registry
use std::collections::HashMap;
use std::sync::Arc;

use crate::tools::context::{ToolInvocation, ToolOutput, FunctionCallError};

pub enum ToolKind { Function, Mcp }

pub trait ToolHandler: Send + Sync {
    fn kind(&self) -> ToolKind;
    fn handle(&self, _: ToolInvocation) -> Result<ToolOutput, FunctionCallError>;
}

pub struct ToolRegistry {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    pub fn new() -> Self { Self { handlers: HashMap::new() } }
    pub fn register(&mut self, name: String, handler: Arc<dyn ToolHandler>) {
        self.handlers.insert(name, handler);
    }
    pub fn get(&self, name: &str) -> Option<&Arc<dyn ToolHandler>> {
        self.handlers.get(name)
    }
}
