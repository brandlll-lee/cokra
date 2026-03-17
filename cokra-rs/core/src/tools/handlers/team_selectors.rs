use crate::agent::team_runtime::TeamRuntime;
use crate::tools::context::FunctionCallError;

pub(crate) fn resolve_required_agent_selector(
  team_runtime: &TeamRuntime,
  selector: &str,
  field_name: &str,
) -> Result<String, FunctionCallError> {
  team_runtime
    .resolve_agent_selector_strict(selector)
    .map_err(|err| FunctionCallError::Execution(format!("{field_name}: {err}")))
}

pub(crate) fn resolve_optional_agent_selector(
  team_runtime: &TeamRuntime,
  selector: Option<String>,
  field_name: &str,
) -> Result<Option<String>, FunctionCallError> {
  let Some(selector) = selector.map(|value| value.trim().to_string()) else {
    return Ok(None);
  };
  if selector.is_empty() {
    return Ok(None);
  }
  resolve_required_agent_selector(team_runtime, &selector, field_name).map(Some)
}
