/// Minimal placeholder for prompt argument parsing.
///
/// cokra currently keeps slash prompt arguments in raw text form. This module
/// exists so bottom-pane modules can share the same location as codex for the
/// future full parser.
pub(crate) fn split_prompt_args(input: &str) -> Vec<String> {
  input.split_whitespace().map(ToString::to_string).collect()
}
