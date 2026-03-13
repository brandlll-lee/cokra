use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;

use crate::skills::injection::render_skill_tool_output;
use crate::skills::loader::build_skill_tool_description;
use crate::skills::loader::discover_skills;
use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SkillHandler;

#[derive(Debug, Deserialize)]
struct SkillArgs {
  name: String,
}

#[async_trait]
impl ToolHandler for SkillHandler {
  fn kind(&self) -> ToolKind {
    ToolKind::Function
  }

  async fn handle_async(
    &self,
    invocation: ToolInvocation,
  ) -> Result<ToolOutput, FunctionCallError> {
    let id = invocation.id.clone();
    let args: SkillArgs = invocation.parse_arguments()?;

    let catalog = discover_skills(&invocation.cwd).await;
    let available = catalog
      .skills
      .iter()
      .map(|skill| skill.name.clone())
      .collect::<Vec<_>>();
    let Some(skill) = catalog
      .skills
      .iter()
      .find(|skill| skill.name == args.name)
      .cloned()
    else {
      let available = if available.is_empty() {
        "none".to_string()
      } else {
        available.join(", ")
      };
      return Err(FunctionCallError::RespondToModel(format!(
        "Skill \"{}\" not found. Available skills: {available}",
        args.name
      )));
    };

    let output = render_skill_tool_output(&skill).await;
    Ok(ToolOutput::success(output).with_id(id))
  }
}

pub async fn build_skill_description(cwd: &Path) -> String {
  build_skill_tool_description(cwd).await
}
