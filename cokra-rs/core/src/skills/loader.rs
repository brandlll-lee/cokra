use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

pub const COKRA_DIR: &str = ".cokra";
pub const COKRA_GENERATED_DIR: &str = ".cokra.generated";
pub const SKILLS_DIR: &str = "skills";
pub const SKILL_FILENAME: &str = "SKILL.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillScope {
  Project,
  User,
  System,
}

#[derive(Debug, Clone)]
pub struct CokraRoot {
  pub config_dir: PathBuf,
  pub scope: SkillScope,
}

#[derive(Debug, Clone)]
pub struct SkillDocument {
  pub name: String,
  pub description: String,
  pub location: PathBuf,
  pub scope: SkillScope,
  pub content: String,
}

#[derive(Debug, Default, Clone)]
pub struct SkillCatalog {
  pub skills: Vec<SkillDocument>,
  pub warnings: Vec<String>,
}

pub async fn discover_skills(cwd: &Path) -> SkillCatalog {
  let mut by_name: HashMap<String, SkillDocument> = HashMap::new();
  let mut warnings = Vec::new();

  for root in ordered_cokra_roots(cwd) {
    let skill_dir = root.config_dir.join(SKILLS_DIR);
    let paths = collect_named_files(&skill_dir, SKILL_FILENAME).await;
    for path in paths {
      match parse_skill_file(&path, root.scope).await {
        Ok(skill) => {
          by_name.insert(skill.name.clone(), skill);
        }
        Err(err) => warnings.push(format!("failed to load skill {}: {err}", path.display())),
      }
    }
  }

  let mut skills = by_name.into_values().collect::<Vec<_>>();
  skills.sort_by(|left, right| left.name.cmp(&right.name));
  SkillCatalog { skills, warnings }
}

pub async fn load_skill(cwd: &Path, name: &str) -> Option<SkillDocument> {
  discover_skills(cwd)
    .await
    .skills
    .into_iter()
    .find(|skill| skill.name == name)
}

pub async fn build_skill_tool_description(cwd: &Path) -> String {
  let catalog = discover_skills(cwd).await;
  if catalog.skills.is_empty() {
    return "Load a specialized skill that provides domain-specific instructions and workflows. No skills are currently available.".to_string();
  }

  let examples = catalog
    .skills
    .iter()
    .take(3)
    .map(|skill| format!("'{}'", skill.name))
    .collect::<Vec<_>>()
    .join(", ");
  let skill_xml = catalog
    .skills
    .iter()
    .flat_map(|skill| {
      [
        "  <skill>".to_string(),
        format!("    <name>{}</name>", skill.name),
        format!("    <description>{}</description>", skill.description),
        format!("    <location>{}</location>", skill.location.display()),
        "  </skill>".to_string(),
      ]
    })
    .collect::<Vec<_>>()
    .join("\n");

  format!(
    "Load a specialized skill that provides domain-specific instructions and workflows.\n\n\
     When you recognize that a task matches one of the available skills listed below, use this tool to load the full skill instructions.\n\n\
     The skill will inject detailed instructions, workflows, and access to bundled resources into the conversation context.\n\n\
     Tool output includes a `<skill_content name=\"...\">` block with the loaded content.\n\n\
     <available_skills>\n\
     {skill_xml}\n\
     </available_skills>\n\n\
     Available skill names (for example, {examples}, ...)"
  )
}

pub async fn list_skill_bundle_files(skill_md_path: &Path, limit: usize) -> Vec<PathBuf> {
  let Some(dir) = skill_md_path.parent() else {
    return Vec::new();
  };
  let mut files = collect_files(dir).await;
  files.retain(|path| path.file_name().and_then(|name| name.to_str()) != Some(SKILL_FILENAME));
  files.sort();
  files.truncate(limit);
  files
}

pub(crate) fn ordered_cokra_roots(cwd: &Path) -> Vec<CokraRoot> {
  let mut roots = Vec::new();

  if let Some(home) = dirs::home_dir() {
    roots.push(CokraRoot {
      config_dir: home.join(COKRA_DIR),
      scope: SkillScope::User,
    });
  }

  let mut project_roots = cwd
    .ancestors()
    .flat_map(project_root_candidates)
    .collect::<Vec<_>>();
  project_roots.reverse();
  roots.extend(project_roots);

  roots
}

fn project_root_candidates(path: &Path) -> Vec<CokraRoot> {
  let mut roots = Vec::new();
  let config_dir = path.join(COKRA_DIR);
  if config_dir.is_dir() {
    roots.push(CokraRoot {
      config_dir,
      scope: SkillScope::Project,
    });
  }

  let generated_dir = path.join(COKRA_GENERATED_DIR);
  if generated_dir.is_dir() {
    roots.push(CokraRoot {
      config_dir: generated_dir,
      scope: SkillScope::Project,
    });
  }

  roots
}

pub(crate) async fn collect_markdown_files(dir: &Path) -> Vec<PathBuf> {
  let mut files = collect_files(dir).await;
  files.retain(|path| {
    path
      .extension()
      .and_then(|extension| extension.to_str())
      .map(|extension| extension.eq_ignore_ascii_case("md"))
      .unwrap_or(false)
  });
  files.sort();
  files
}

pub(crate) fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
  let raw = raw.trim_start_matches('\u{feff}');
  let stripped = raw.strip_prefix("---")?;
  let stripped = stripped.trim_start_matches('\r').trim_start_matches('\n');

  if let Some(pos) = stripped.find("\n---\n") {
    let frontmatter = &stripped[..pos];
    let body = &stripped[pos + 5..];
    return Some((frontmatter, body));
  }
  if let Some(pos) = stripped.find("\r\n---\r\n") {
    let frontmatter = &stripped[..pos];
    let body = &stripped[pos + 7..];
    return Some((frontmatter, body));
  }
  if stripped.ends_with("\n---") || stripped.ends_with("\r\n---") {
    let pos = stripped.rfind("\n---").unwrap_or(0);
    return Some((&stripped[..pos], ""));
  }

  None
}

pub(crate) fn extract_frontmatter_field(frontmatter: &str, key: &str) -> Option<String> {
  let prefix = format!("{key}:");
  for line in frontmatter.lines() {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix(&prefix) {
      let value = rest.trim().trim_matches('"').trim_matches('\'').to_string();
      if !value.is_empty() {
        return Some(value);
      }
    }
  }
  None
}

async fn parse_skill_file(path: &Path, scope: SkillScope) -> Result<SkillDocument, String> {
  let raw = tokio::fs::read_to_string(path)
    .await
    .map_err(|err| format!("read error: {err}"))?;
  let (frontmatter, body) =
    split_frontmatter(&raw).ok_or_else(|| "missing YAML frontmatter".to_string())?;
  let name = extract_frontmatter_field(frontmatter, "name")
    .ok_or_else(|| "missing `name` field".to_string())?;
  let description =
    extract_frontmatter_field(frontmatter, "description").unwrap_or_else(|| name.clone());

  Ok(SkillDocument {
    name,
    description,
    location: path.to_path_buf(),
    scope,
    content: body.trim().to_string(),
  })
}

async fn collect_named_files(dir: &Path, target_name: &str) -> Vec<PathBuf> {
  let mut files = collect_files(dir).await;
  files.retain(|path| path.file_name().and_then(|name| name.to_str()) == Some(target_name));
  files.sort();
  files
}

async fn collect_files(dir: &Path) -> Vec<PathBuf> {
  if !tokio::fs::metadata(dir)
    .await
    .map(|meta| meta.is_dir())
    .unwrap_or(false)
  {
    return Vec::new();
  }

  let mut files = Vec::new();
  let mut stack = vec![dir.to_path_buf()];
  while let Some(current) = stack.pop() {
    let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
      continue;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
      let path = entry.path();
      let Ok(file_type) = entry.file_type().await else {
        continue;
      };
      if file_type.is_dir() {
        stack.push(path);
      } else if file_type.is_file() {
        files.push(path);
      }
    }
  }
  files
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn split_frontmatter_supports_lf() {
    let raw = "---\nname: rust\ndescription: demo\n---\n\nHello";
    let (frontmatter, body) = split_frontmatter(raw).expect("frontmatter");
    assert!(frontmatter.contains("name: rust"));
    assert_eq!(body.trim(), "Hello");
  }

  #[test]
  fn split_frontmatter_supports_crlf() {
    let raw = "---\r\nname: rust\r\ndescription: demo\r\n---\r\n\r\nHello";
    let (frontmatter, body) = split_frontmatter(raw).expect("frontmatter");
    assert!(frontmatter.contains("description: demo"));
    assert_eq!(body.trim(), "Hello");
  }

  #[test]
  fn extract_frontmatter_field_reads_unquoted_and_quoted_values() {
    let frontmatter = "name: rust\ndescription: \"Rust expert\"";
    assert_eq!(
      extract_frontmatter_field(frontmatter, "name").as_deref(),
      Some("rust")
    );
    assert_eq!(
      extract_frontmatter_field(frontmatter, "description").as_deref(),
      Some("Rust expert")
    );
  }

  #[tokio::test]
  async fn discover_skills_prefers_nearest_project_root() {
    let root = tempfile::tempdir().expect("tempdir");
    let child = root.path().join("child");
    tokio::fs::create_dir_all(&child).await.expect("create child");

    let parent_skill_dir = root.path().join(COKRA_DIR).join(SKILLS_DIR).join("demo");
    tokio::fs::create_dir_all(&parent_skill_dir)
      .await
      .expect("create parent skill dir");
    tokio::fs::write(
      parent_skill_dir.join(SKILL_FILENAME),
      "---\nname: demo\ndescription: parent\n---\n\nparent",
    )
    .await
    .expect("write parent skill");

    let child_skill_dir = child.join(COKRA_DIR).join(SKILLS_DIR).join("demo");
    tokio::fs::create_dir_all(&child_skill_dir)
      .await
      .expect("create child skill dir");
    tokio::fs::write(
      child_skill_dir.join(SKILL_FILENAME),
      "---\nname: demo\ndescription: child\n---\n\nchild",
    )
    .await
    .expect("write child skill");

    let catalog = discover_skills(&child).await;
    let skill = catalog
      .skills
      .iter()
      .find(|skill| skill.name == "demo")
      .expect("demo skill");
    assert_eq!(skill.description, "child");
    assert_eq!(skill.content, "child");
  }

  #[tokio::test]
  async fn build_skill_tool_description_renders_available_skills_block() {
    let root = tempfile::tempdir().expect("tempdir");
    let skill_dir = root.path().join(COKRA_DIR).join(SKILLS_DIR).join("demo");
    tokio::fs::create_dir_all(&skill_dir)
      .await
      .expect("create skill dir");
    tokio::fs::write(
      skill_dir.join(SKILL_FILENAME),
      "---\nname: demo\ndescription: Demo skill\n---\n\nUse it",
    )
    .await
    .expect("write skill");

    let description = build_skill_tool_description(root.path()).await;
    assert!(description.contains("<available_skills>"));
    assert!(description.contains("<name>demo</name>"));
    assert!(description.contains("Demo skill"));
  }

  #[tokio::test]
  async fn discover_skills_reads_generated_fallback_root() {
    let root = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(root.path().join(COKRA_DIR), "marker")
      .await
      .expect("write marker file");

    let generated_skill_dir = root
      .path()
      .join(COKRA_GENERATED_DIR)
      .join(SKILLS_DIR)
      .join("demo");
    tokio::fs::create_dir_all(&generated_skill_dir)
      .await
      .expect("create generated skill dir");
    tokio::fs::write(
      generated_skill_dir.join(SKILL_FILENAME),
      "---\nname: demo\ndescription: Generated demo\n---\n\ngenerated",
    )
    .await
    .expect("write generated skill");

    let catalog = discover_skills(root.path()).await;
    let skill = catalog
      .skills
      .iter()
      .find(|skill| skill.name == "demo")
      .expect("demo skill");
    assert_eq!(skill.description, "Generated demo");
    assert_eq!(skill.content, "generated");
    assert!(skill.location.starts_with(root.path().join(COKRA_GENERATED_DIR)));
  }

  #[tokio::test]
  async fn discover_skills_prefers_handwritten_root_over_generated_root() {
    let root = tempfile::tempdir().expect("tempdir");

    let handwritten_skill_dir = root.path().join(COKRA_DIR).join(SKILLS_DIR).join("demo");
    tokio::fs::create_dir_all(&handwritten_skill_dir)
      .await
      .expect("create handwritten skill dir");
    tokio::fs::write(
      handwritten_skill_dir.join(SKILL_FILENAME),
      "---\nname: demo\ndescription: Handwritten demo\n---\n\nhandwritten",
    )
    .await
    .expect("write handwritten skill");

    let generated_skill_dir = root
      .path()
      .join(COKRA_GENERATED_DIR)
      .join(SKILLS_DIR)
      .join("demo");
    tokio::fs::create_dir_all(&generated_skill_dir)
      .await
      .expect("create generated skill dir");
    tokio::fs::write(
      generated_skill_dir.join(SKILL_FILENAME),
      "---\nname: demo\ndescription: Generated demo\n---\n\ngenerated",
    )
    .await
    .expect("write generated skill");

    let catalog = discover_skills(root.path()).await;
    let skill = catalog
      .skills
      .iter()
      .find(|skill| skill.name == "demo")
      .expect("demo skill");
    assert_eq!(skill.description, "Handwritten demo");
    assert_eq!(skill.content, "handwritten");
    assert!(skill.location.starts_with(root.path().join(COKRA_DIR)));
  }
}
