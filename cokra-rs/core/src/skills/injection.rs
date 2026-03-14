use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use super::loader::CokraRoot;
use super::loader::SkillDocument;
use super::loader::collect_markdown_files;
use super::loader::discover_skills;
use super::loader::extract_frontmatter_field;
use super::loader::list_skill_bundle_files;
use super::loader::ordered_cokra_roots;
use super::loader::split_frontmatter;

const PERSONAS_DIR: &str = "personas";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptAssetKind {
  Skill,
  Persona,
}

#[derive(Debug, Clone)]
pub struct PromptAssetDocument {
  pub kind: PromptAssetKind,
  pub name: String,
  pub description: String,
  pub location: PathBuf,
  pub content: String,
}

#[derive(Debug, Default, Clone)]
pub struct ExplicitPromptInjections {
  pub skills: Vec<SkillDocument>,
  pub personas: Vec<PromptAssetDocument>,
  pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Mention {
  kind: PromptAssetKind,
  name: String,
  linked_target: Option<String>,
}

pub async fn build_explicit_prompt_injections(
  cwd: &Path,
  user_text: &str,
) -> ExplicitPromptInjections {
  let mentions = extract_explicit_mentions(user_text);
  if mentions.is_empty() {
    return ExplicitPromptInjections::default();
  }

  let skill_catalog = discover_skills(cwd).await;
  let personas = discover_prompt_assets(cwd, PromptAssetKind::Persona).await;
  let mut seen_skills = HashSet::new();
  let mut seen_personas = HashSet::new();

  let mut result = ExplicitPromptInjections {
    warnings: skill_catalog.warnings.clone(),
    ..Default::default()
  };

  for mention in mentions {
    match mention.kind {
      PromptAssetKind::Skill => {
        if let Some(skill) = resolve_skill_mention(&mention, &skill_catalog.skills) {
          if seen_skills.insert(skill.name.clone()) {
            result.skills.push(skill);
          }
        } else {
          result.warnings.push(format!(
            "explicit skill mention `{}` could not be resolved",
            mention.name
          ));
        }
      }
      PromptAssetKind::Persona => {
        if let Some(persona) = resolve_asset_mention(&mention, &personas) {
          if seen_personas.insert(persona.name.clone()) {
            result.personas.push(persona);
          }
        } else {
          result.warnings.push(format!(
            "explicit persona mention `{}` could not be resolved",
            mention.name
          ));
        }
      }
    }
  }

  result
}

pub fn render_explicit_prompt_injections(injections: &ExplicitPromptInjections) -> Option<String> {
  if injections.skills.is_empty() && injections.personas.is_empty() {
    return None;
  }

  let mut rendered = String::from(
    "<explicit_injections>\n\
     The user explicitly referenced the following prompt assets. Treat them as active context for this turn.\n\n",
  );

  if !injections.skills.is_empty() {
    rendered.push_str("<skills>\n");
    for skill in &injections.skills {
      rendered.push_str(&render_skill_prompt_block(skill));
      rendered.push('\n');
    }
    rendered.push_str("</skills>\n\n");
  }

  if !injections.personas.is_empty() {
    rendered.push_str("<personas>\n");
    for persona in &injections.personas {
      rendered.push_str(&render_prompt_asset_block(persona, "persona_content"));
      rendered.push('\n');
    }
    rendered.push_str("</personas>\n\n");
  }

  rendered.push_str("</explicit_injections>");
  Some(rendered)
}

pub async fn render_skill_tool_output(skill: &SkillDocument) -> String {
  let bundled_files = list_skill_bundle_files(&skill.location, 10).await;
  let file_xml = bundled_files
    .iter()
    .map(|path| format!("<file>{}</file>", path.display()))
    .collect::<Vec<_>>()
    .join("\n");
  let base_dir = skill
    .location
    .parent()
    .map(|path| path.display().to_string())
    .unwrap_or_default();

  format!(
    "<skill_content name=\"{name}\">\n\
     # Skill: {name}\n\n\
     {content}\n\n\
     Base directory for this skill: {base_dir}\n\
     Relative paths in this skill are resolved from this base directory.\n\
     Note: bundled file list is sampled.\n\n\
     <skill_files>\n\
     {file_xml}\n\
     </skill_files>\n\
     </skill_content>",
    name = skill.name,
    content = skill.content.trim(),
  )
}

fn render_skill_prompt_block(skill: &SkillDocument) -> String {
  format!(
    "<skill_content name=\"{name}\">\n\
     Source: {source}\n\
     Scope: {scope:?}\n\n\
     {content}\n\
     </skill_content>",
    name = skill.name,
    source = skill.location.display(),
    scope = skill.scope,
    content = skill.content.trim(),
  )
}

fn render_prompt_asset_block(asset: &PromptAssetDocument, tag_name: &str) -> String {
  format!(
    "<{tag_name} name=\"{name}\">\n\
     Source: {source}\n\
     Description: {description}\n\n\
     {content}\n\
     </{tag_name}>",
    tag_name = tag_name,
    name = asset.name,
    source = asset.location.display(),
    description = asset.description,
    content = asset.content.trim(),
  )
}

async fn discover_prompt_assets(cwd: &Path, kind: PromptAssetKind) -> Vec<PromptAssetDocument> {
  let mut by_name = std::collections::HashMap::new();
  for root in ordered_cokra_roots(cwd) {
    let directory_name = match kind {
      PromptAssetKind::Skill => continue,
      PromptAssetKind::Persona => PERSONAS_DIR,
    };
    let asset_dir = root.config_dir.join(directory_name);
    let mut paths = collect_markdown_files(&asset_dir).await;
    paths.sort_by(|left, right| {
      prompt_asset_generated_rank(left)
        .cmp(&prompt_asset_generated_rank(right))
        .then_with(|| left.cmp(right))
    });
    for path in paths {
      if let Ok(asset) = parse_prompt_asset(&root, &path, kind).await {
        by_name.insert(asset.name.clone(), asset);
      }
    }
  }
  let mut assets = by_name.into_values().collect::<Vec<_>>();
  assets.sort_by(|left, right| left.name.cmp(&right.name));
  assets
}

fn prompt_asset_generated_rank(path: &Path) -> u8 {
  path
    .components()
    .any(|component| {
      component
        .as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case("generated")
    })
    .then_some(0)
    .unwrap_or(1)
}

async fn parse_prompt_asset(
  _root: &CokraRoot,
  path: &Path,
  kind: PromptAssetKind,
) -> Result<PromptAssetDocument, String> {
  let raw = tokio::fs::read_to_string(path)
    .await
    .map_err(|err| format!("read error: {err}"))?;
  let (name, description, content) = if let Some((frontmatter, body)) = split_frontmatter(&raw) {
    let name = extract_frontmatter_field(frontmatter, "name").unwrap_or_else(|| {
      path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unnamed")
        .to_string()
    });
    let description =
      extract_frontmatter_field(frontmatter, "description").unwrap_or_else(|| name.clone());
    (name, description, body.trim().to_string())
  } else {
    let name = path
      .file_stem()
      .and_then(|stem| stem.to_str())
      .unwrap_or("unnamed")
      .to_string();
    let description = name.clone();
    (name, description, raw.trim().to_string())
  };

  Ok(PromptAssetDocument {
    kind,
    name,
    description,
    location: path.to_path_buf(),
    content,
  })
}

fn extract_explicit_mentions(text: &str) -> Vec<Mention> {
  let mut mentions = Vec::new();
  let mut seen = HashSet::new();

  for mention in extract_linked_mentions(text) {
    let key = (mention.kind, mention.name.clone());
    if seen.insert(key) {
      mentions.push(mention);
    }
  }
  for mention in extract_plain_mentions(text) {
    let key = (mention.kind, mention.name.clone());
    if seen.insert(key) {
      mentions.push(mention);
    }
  }

  mentions
}

fn extract_linked_mentions(text: &str) -> Vec<Mention> {
  let bytes = text.as_bytes();
  let mut mentions = Vec::new();
  let mut index = 0;

  while index < bytes.len() {
    if bytes[index] != b'[' {
      index += 1;
      continue;
    }

    let Some(close_bracket_offset) = text[index + 1..].find("](") else {
      index += 1;
      continue;
    };
    let close_bracket = index + 1 + close_bracket_offset;
    let Some(close_paren_offset) = text[close_bracket + 2..].find(')') else {
      index += 1;
      continue;
    };
    let close_paren = close_bracket + 2 + close_paren_offset;

    let label = text[index + 1..close_bracket].trim();
    let target = text[close_bracket + 2..close_paren].trim();
    if let Some(mention) = mention_from_label(label, Some(target.to_string())) {
      mentions.push(mention);
    }
    index = close_paren + 1;
  }

  mentions
}

fn extract_plain_mentions(text: &str) -> Vec<Mention> {
  let mut mentions = Vec::new();
  let chars = text.char_indices().collect::<Vec<_>>();
  let link_spans = markdown_link_spans(text);

  for (position, (index, ch)) in chars.iter().enumerate() {
    if !matches!(*ch, '$' | '@' | '#') {
      continue;
    }
    if link_spans
      .iter()
      .any(|(start, end)| *index >= *start && *index < *end)
    {
      continue;
    }

    let previous = position
      .checked_sub(1)
      .and_then(|idx| chars.get(idx).copied());
    if previous
      .map(|(_, prev)| is_mention_name_char(prev))
      .unwrap_or(false)
    {
      continue;
    }

    let Some((_, first)) = chars.get(position + 1).copied() else {
      continue;
    };
    if !is_mention_name_char(first) {
      continue;
    }

    let mut end = text.len();
    for (next_index, next_char) in chars.iter().skip(position + 1) {
      if !is_mention_name_char(*next_char) {
        end = *next_index;
        break;
      }
    }

    if let Some(mention) = mention_from_label(&text[*index..end], None) {
      mentions.push(mention);
    }
  }

  mentions
}

fn markdown_link_spans(text: &str) -> Vec<(usize, usize)> {
  let bytes = text.as_bytes();
  let mut spans = Vec::new();
  let mut index = 0;

  while index < bytes.len() {
    if bytes[index] != b'[' {
      index += 1;
      continue;
    }

    let Some(close_bracket_offset) = text[index + 1..].find("](") else {
      index += 1;
      continue;
    };
    let close_bracket = index + 1 + close_bracket_offset;
    let Some(close_paren_offset) = text[close_bracket + 2..].find(')') else {
      index += 1;
      continue;
    };
    let close_paren = close_bracket + 2 + close_paren_offset;
    spans.push((index, close_paren + 1));
    index = close_paren + 1;
  }

  spans
}

fn mention_from_label(label: &str, linked_target: Option<String>) -> Option<Mention> {
  let mut chars = label.chars();
  let marker = chars.next()?;
  let name = chars.as_str().trim();
  if name.is_empty() {
    return None;
  }
  let kind = match marker {
    '$' => PromptAssetKind::Skill,
    '@' => PromptAssetKind::Persona,
    _ => return None,
  };
  Some(Mention {
    kind,
    name: name.to_string(),
    linked_target,
  })
}

fn is_mention_name_char(ch: char) -> bool {
  ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/')
}

fn resolve_skill_mention(mention: &Mention, skills: &[SkillDocument]) -> Option<SkillDocument> {
  if let Some(target) = mention.linked_target.as_deref()
    && let Some(skill) = skills
      .iter()
      .find(|skill| skill_target_matches(target, skill))
  {
    return Some(skill.clone());
  }

  skills
    .iter()
    .find(|skill| skill.name == mention.name)
    .cloned()
}

fn skill_target_matches(target: &str, skill: &SkillDocument) -> bool {
  target
    .strip_prefix("skill://")
    .map(|value| value == skill.name)
    .unwrap_or(false)
    || PathBuf::from(target) == skill.location
}

fn resolve_asset_mention(
  mention: &Mention,
  assets: &[PromptAssetDocument],
) -> Option<PromptAssetDocument> {
  if let Some(target) = mention.linked_target.as_deref()
    && let Some(asset) = assets
      .iter()
      .find(|asset| asset_target_matches(target, asset))
  {
    return Some(asset.clone());
  }

  assets
    .iter()
    .find(|asset| asset.name == mention.name)
    .cloned()
}

fn asset_target_matches(target: &str, asset: &PromptAssetDocument) -> bool {
  let prefix = match asset.kind {
    PromptAssetKind::Skill => "skill://",
    PromptAssetKind::Persona => "persona://",
  };
  target
    .strip_prefix(prefix)
    .map(|value| value == asset.name)
    .unwrap_or(false)
    || PathBuf::from(target) == asset.location
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn extract_explicit_mentions_supports_plain_markers() {
    let mentions = extract_explicit_mentions("Use $rust-expert with @backend");
    assert_eq!(mentions.len(), 2);
    assert_eq!(mentions[0].name, "rust-expert");
    assert_eq!(mentions[1].name, "backend");
  }

  #[test]
  fn extract_explicit_mentions_supports_linked_markers() {
    let mentions =
      extract_explicit_mentions("Use [$rust-expert](/tmp/SKILL.md) and [@ops](persona://ops)");
    assert_eq!(mentions.len(), 2);
    assert_eq!(mentions[0].linked_target.as_deref(), Some("/tmp/SKILL.md"));
    assert_eq!(mentions[1].linked_target.as_deref(), Some("persona://ops"));
  }

  #[test]
  fn extract_explicit_mentions_prefers_linked_variant_over_plain_duplicate() {
    let mentions = extract_explicit_mentions(
      "Use [$rust-expert](skill://rust-expert) and then $rust-expert again",
    );
    assert_eq!(mentions.len(), 1);
    assert_eq!(
      mentions[0].linked_target.as_deref(),
      Some("skill://rust-expert")
    );
  }

  #[test]
  fn render_explicit_prompt_injections_groups_sections() {
    let rendered = render_explicit_prompt_injections(&ExplicitPromptInjections {
      skills: vec![SkillDocument {
        name: "rust-expert".to_string(),
        description: "Rust".to_string(),
        location: PathBuf::from("/tmp/SKILL.md"),
        scope: super::super::loader::SkillScope::Project,
        content: "Follow Rust rules.".to_string(),
      }],
      personas: vec![PromptAssetDocument {
        kind: PromptAssetKind::Persona,
        name: "backend".to_string(),
        description: "Backend maintainer".to_string(),
        location: PathBuf::from("/tmp/backend.md"),
        content: "Focus on service safety.".to_string(),
      }],
      warnings: Vec::new(),
    })
    .expect("rendered");

    assert!(rendered.contains("<skills>"));
    assert!(rendered.contains("<personas>"));
    assert!(rendered.contains("rust-expert"));
    assert!(rendered.contains("backend"));
  }
}
