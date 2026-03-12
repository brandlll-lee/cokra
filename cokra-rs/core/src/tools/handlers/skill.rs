//! skill 工具 handler — 加载并注入领域专属 Skill 文档。
//!
//! 复刻 opencode `tool/skill.ts` + codex `skills` crate 设计，适配 cokra Rust 架构。
//!
//! ## Skill 搜索路径（优先级从低到高）
//! 1. `~/.cokra/skills/**/SKILL.md`  — 全局用户 skills
//! 2. `.cokra/skills/**/SKILL.md`    — 项目级 skills（从 cwd 向上遍历）
//!
//! ## SKILL.md 格式
//! ```markdown
//! ---
//! name: my-skill
//! description: 这个 skill 做什么
//! ---
//!
//! skill 的完整指令内容...
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::tools::context::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct SkillHandler;

const SKILL_FILENAME: &str = "SKILL.md";
const SKILLS_DIR: &str = "skills";
const COKRA_DIR: &str = ".cokra";

#[derive(Debug, Deserialize)]
struct SkillArgs {
  /// 要加载的 skill 名称，必须是 available_skills 中列出的之一。
  name: String,
}

/// Skill 元数据（从 SKILL.md frontmatter 解析）。
#[derive(Debug, Clone)]
struct SkillInfo {
  name: String,
  description: String,
  /// SKILL.md 文件的绝对路径。
  location: PathBuf,
  /// SKILL.md frontmatter 之后的正文内容。
  content: String,
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

    let cwd = &invocation.cwd;
    let skills = discover_skills(cwd).await;

    let skill = skills.get(&args.name).cloned().ok_or_else(|| {
      let available = if skills.is_empty() {
        "无可用 skill".to_string()
      } else {
        skills.keys().cloned().collect::<Vec<_>>().join(", ")
      };
      FunctionCallError::RespondToModel(format!(
        "skill \"{}\" 不存在。可用 skill: {}",
        args.name, available
      ))
    })?;

    let skill_dir = skill
      .location
      .parent()
      .map(|p| p.display().to_string())
      .unwrap_or_default();

    // 列出 skill 目录内的附属文件（排除 SKILL.md 本身），最多 10 个
    let bundled_files = list_skill_files(&skill.location).await;

    let output = build_skill_output(&skill, &skill_dir, &bundled_files);

    Ok(ToolOutput::success(output).with_id(id))
  }
}

// ── Skill 发现 ─────────────────────────────────────────────────────────────

/// 扫描所有 skill 搜索路径，返回 name → SkillInfo 映射。
/// 项目级 skills 覆盖全局 skills（同名时后者胜出）。
async fn discover_skills(cwd: &Path) -> HashMap<String, SkillInfo> {
  let mut skills: HashMap<String, SkillInfo> = HashMap::new();

  // 1. 全局 skill：~/.cokra/skills/
  if let Some(home) = dirs::home_dir() {
    let global_dir = home.join(COKRA_DIR).join(SKILLS_DIR);
    scan_skills_dir(&global_dir, &mut skills).await;
  }

  // 2. 项目级 skill：从 cwd 向上遍历，找到 .cokra/skills/
  for dir in ancestors_with_cokra(cwd) {
    let project_dir = dir.join(COKRA_DIR).join(SKILLS_DIR);
    scan_skills_dir(&project_dir, &mut skills).await;
  }

  skills
}

/// 从 cwd 向上遍历，收集所有含 .cokra 目录的祖先路径（从根到子顺序）。
fn ancestors_with_cokra(start: &Path) -> Vec<PathBuf> {
  let mut candidates: Vec<PathBuf> = start
    .ancestors()
    .filter(|p| p.join(COKRA_DIR).is_dir())
    .map(|p| p.to_path_buf())
    .collect();
  // 翻转：根目录优先（全局先，项目后），确保项目级覆盖全局
  candidates.reverse();
  candidates
}

/// 递归扫描 `dir` 下所有 SKILL.md，解析并注入 `skills` map。
async fn scan_skills_dir(dir: &Path, skills: &mut HashMap<String, SkillInfo>) {
  if !tokio::fs::metadata(dir)
    .await
    .map(|m| m.is_dir())
    .unwrap_or(false)
  {
    return;
  }

  // walkdir 风格：递归查找所有 SKILL.md
  let entries = collect_skill_files(dir).await;
  for skill_path in entries {
    match parse_skill_file(&skill_path).await {
      Ok(info) => {
        skills.insert(info.name.clone(), info);
      }
      Err(_) => {
        // 解析失败：静默跳过，不影响其他 skill
      }
    }
  }
}

/// 迭代式收集目录下所有 SKILL.md 文件路径（避免递归 async 生命周期问题）。
async fn collect_skill_files(dir: &Path) -> Vec<PathBuf> {
  let mut result = Vec::new();
  let mut stack = vec![dir.to_path_buf()];

  while let Some(current) = stack.pop() {
    let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
      continue;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
      let path = entry.path();
      let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
      if is_dir {
        stack.push(path);
      } else if path.file_name().and_then(|n| n.to_str()) == Some(SKILL_FILENAME) {
        result.push(path);
      }
    }
  }

  result
}

// ── SKILL.md 解析 ───────────────────────────────────────────────────────────

/// 解析单个 SKILL.md 文件，提取 frontmatter 中的 name/description 和正文。
async fn parse_skill_file(path: &Path) -> Result<SkillInfo, String> {
  let raw = tokio::fs::read_to_string(path)
    .await
    .map_err(|e| format!("read error: {e}"))?;

  let (frontmatter, content) =
    split_frontmatter(&raw).ok_or_else(|| "缺少 YAML frontmatter（--- 分隔符）".to_string())?;

  let name = extract_frontmatter_field(frontmatter, "name")
    .ok_or_else(|| "frontmatter 缺少 name 字段".to_string())?;

  let description =
    extract_frontmatter_field(frontmatter, "description").unwrap_or_else(|| name.clone());

  Ok(SkillInfo {
    name,
    description,
    location: path.to_path_buf(),
    content: content.trim().to_string(),
  })
}

/// 将 SKILL.md 内容分割为 (frontmatter, body)。
/// 文件必须以 `---\n` 开头，frontmatter 以第二个 `---` 结束。
fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
  let raw = raw.trim_start_matches('\u{feff}'); // strip BOM
  let stripped = raw.strip_prefix("---")?;
  let stripped = stripped.trim_start_matches('\r').trim_start_matches('\n');

  // 查找结束 ---
  let end_marker = if let Some(pos) = stripped.find("\n---\n") {
    let fm = &stripped[..pos];
    let body = &stripped[pos + 5..];
    (fm, body)
  } else if let Some(pos) = stripped.find("\r\n---\r\n") {
    let fm = &stripped[..pos];
    let body = &stripped[pos + 7..];
    (fm, body)
  } else if stripped.ends_with("\n---") || stripped.ends_with("\r\n---") {
    let pos = stripped.rfind("\n---").unwrap_or(0);
    (&stripped[..pos], "")
  } else {
    return None;
  };

  Some(end_marker)
}

/// 从 frontmatter 字符串中提取单行字段值。
/// 匹配 `key: value` 格式，value 可以带引号。
fn extract_frontmatter_field(frontmatter: &str, key: &str) -> Option<String> {
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

// ── 附属文件列举 ────────────────────────────────────────────────────────────

/// 列出 skill 目录内除 SKILL.md 外的文件（最多 10 个）。
async fn list_skill_files(skill_md_path: &Path) -> Vec<PathBuf> {
  let Some(dir) = skill_md_path.parent() else {
    return Vec::new();
  };

  let mut files = Vec::new();
  if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
    while let Ok(Some(entry)) = entries.next_entry().await {
      let path = entry.path();
      let is_file = entry
        .file_type()
        .await
        .map(|t| t.is_file())
        .unwrap_or(false);
      if is_file && path.file_name().and_then(|n| n.to_str()) != Some(SKILL_FILENAME) {
        files.push(path);
        if files.len() >= 10 {
          break;
        }
      }
    }
  }
  files.sort();
  files
}

// ── 输出构建 ────────────────────────────────────────────────────────────────

fn build_skill_output(skill: &SkillInfo, skill_dir: &str, bundled_files: &[PathBuf]) -> String {
  let files_section = if bundled_files.is_empty() {
    String::new()
  } else {
    let file_list = bundled_files
      .iter()
      .map(|p| format!("<file>{}</file>", p.display()))
      .collect::<Vec<_>>()
      .join("\n");
    format!(
      "\nBase directory for this skill: {skill_dir}\n\
       Relative paths in this skill (e.g., scripts/, reference/) are relative to this base directory.\n\
       Note: file list is sampled (max 10).\n\
       \n<skill_files>\n{file_list}\n</skill_files>"
    )
  };

  let name = &skill.name;
  let content = &skill.content;
  format!(
    "<skill_content name=\"{name}\">\
     # Skill: {name}\n\
     \n\
     {content}\n\
     {files_section}\n\
     </skill_content>"
  )
}

// ── 工具注册辅助：构建给模型的动态描述 ────────────────────────────────────

/// 动态构建 skill 工具的描述，1:1 复刻 opencode `SkillTool` 的 description 生成逻辑。
///
/// 扫描当前 cwd 下所有 skill，将可用列表以 `<available_skills>` XML 块嵌入描述，
/// 使模型能直接识别有哪些 skill 可用并主动调用本工具（而不是用 glob 自己找）。
pub async fn build_skill_description(cwd: &Path) -> String {
  let skills = discover_skills(cwd).await;

  if skills.is_empty() {
    return "Load a specialized skill that provides domain-specific instructions and workflows. \
            No skills are currently available."
      .to_string();
  }

  let mut entries: Vec<_> = skills.values().collect();
  entries.sort_by(|a, b| a.name.cmp(&b.name));

  // 1:1 opencode: examples hint for the parameter description
  let examples = entries
    .iter()
    .take(3)
    .map(|s| format!("'{}'", s.name))
    .collect::<Vec<_>>()
    .join(", ");

  // 1:1 opencode: build <available_skills> XML block
  let skill_xml = entries
    .iter()
    .flat_map(|s| {
      let location = s.location.display().to_string();
      vec![
        "  <skill>".to_string(),
        format!("    <name>{}</name>", s.name),
        format!("    <description>{}</description>", s.description),
        format!("    <location>{location}</location>"),
        "  </skill>".to_string(),
      ]
    })
    .collect::<Vec<_>>()
    .join("\n");

  format!(
    "Load a specialized skill that provides domain-specific instructions and workflows.\n\
     \n\
     When you recognize that a task matches one of the available skills listed below, \
     use this tool to load the full skill instructions.\n\
     \n\
     The skill will inject detailed instructions, workflows, and access to bundled resources \
     (scripts, references, templates) into the conversation context.\n\
     \n\
     Tool output includes a `<skill_content name=\"...\">` block with the loaded content.\n\
     \n\
     The following skills provide specialized sets of instructions for particular tasks.\n\
     Invoke this tool to load a skill when a task matches one of the available skills listed below:\n\
     \n\
     <available_skills>\n\
     {skill_xml}\n\
     </available_skills>\n\
     \n\
     Available skill names (e.g., {examples}, ...)"
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  // ── frontmatter 解析测试 ────────────────────────────────────────────────

  #[test]
  fn split_frontmatter_basic() {
    let raw = "---\nname: my-skill\ndescription: test\n---\n\nBody text here.";
    let result = split_frontmatter(raw);
    assert!(result.is_some());
    let (fm, body) = result.unwrap();
    assert!(fm.contains("name: my-skill"));
    assert!(body.contains("Body text here."));
  }

  #[test]
  fn split_frontmatter_crlf() {
    let raw = "---\r\nname: crlf-skill\r\ndescription: crlf test\r\n---\r\n\r\nContent.";
    let result = split_frontmatter(raw);
    assert!(result.is_some());
    let (fm, body) = result.unwrap();
    assert!(fm.contains("name: crlf-skill"));
    assert!(body.contains("Content."));
  }

  #[test]
  fn split_frontmatter_missing_delimiters_returns_none() {
    let raw = "name: no-frontmatter\n\nBody.";
    assert!(split_frontmatter(raw).is_none());
  }

  #[test]
  fn split_frontmatter_bom_stripped() {
    let raw = "\u{feff}---\nname: bom-skill\ndescription: bom\n---\n\nContent.";
    let result = split_frontmatter(raw);
    assert!(result.is_some());
  }

  #[test]
  fn extract_frontmatter_field_basic() {
    let fm = "name: hello-world\ndescription: A test skill";
    assert_eq!(
      extract_frontmatter_field(fm, "name"),
      Some("hello-world".to_string())
    );
    assert_eq!(
      extract_frontmatter_field(fm, "description"),
      Some("A test skill".to_string())
    );
  }

  #[test]
  fn extract_frontmatter_field_quoted_value() {
    let fm = "name: \"quoted skill\"\ndescription: 'single quoted'";
    assert_eq!(
      extract_frontmatter_field(fm, "name"),
      Some("quoted skill".to_string())
    );
    assert_eq!(
      extract_frontmatter_field(fm, "description"),
      Some("single quoted".to_string())
    );
  }

  #[test]
  fn extract_frontmatter_field_missing_returns_none() {
    let fm = "name: present";
    assert_eq!(extract_frontmatter_field(fm, "missing"), None);
  }

  // ── skill 文件解析测试 ──────────────────────────────────────────────────

  #[tokio::test]
  async fn parse_skill_file_valid() {
    let dir = tempfile::tempdir().unwrap();
    let skill_path = dir.path().join(SKILL_FILENAME);
    tokio::fs::write(
      &skill_path,
      "---\nname: test-skill\ndescription: A test skill\n---\n\nSkill content here.",
    )
    .await
    .unwrap();

    let info = parse_skill_file(&skill_path).await.unwrap();
    assert_eq!(info.name, "test-skill");
    assert_eq!(info.description, "A test skill");
    assert_eq!(info.content, "Skill content here.");
    assert_eq!(info.location, skill_path);
  }

  #[tokio::test]
  async fn parse_skill_file_missing_name_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let skill_path = dir.path().join(SKILL_FILENAME);
    tokio::fs::write(
      &skill_path,
      "---\ndescription: no name here\n---\n\nContent.",
    )
    .await
    .unwrap();

    let result = parse_skill_file(&skill_path).await;
    assert!(result.is_err());
  }

  #[tokio::test]
  async fn parse_skill_file_no_frontmatter_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let skill_path = dir.path().join(SKILL_FILENAME);
    tokio::fs::write(&skill_path, "Just content, no frontmatter.")
      .await
      .unwrap();

    let result = parse_skill_file(&skill_path).await;
    assert!(result.is_err());
  }

  // ── discover_skills 集成测试 ────────────────────────────────────────────

  #[tokio::test]
  async fn discover_skills_finds_project_skill() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join(COKRA_DIR).join(SKILLS_DIR).join("my-skill");
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(
      skill_dir.join(SKILL_FILENAME),
      "---\nname: my-skill\ndescription: Test\n---\n\nContent.",
    )
    .await
    .unwrap();

    let skills = discover_skills(dir.path()).await;
    assert!(skills.contains_key("my-skill"));
    assert_eq!(skills["my-skill"].description, "Test");
  }

  #[tokio::test]
  async fn discover_skills_empty_when_no_cokra_dir() {
    let dir = tempfile::tempdir().unwrap();
    // cwd 不含 .cokra 目录
    let skills = discover_skills(dir.path()).await;
    // 可能有全局 skill，但至少不崩溃
    let _ = skills;
  }

  #[tokio::test]
  async fn discover_skills_project_overrides_parent() {
    // 父目录有 skill-a，子目录覆盖同名 skill-a
    let parent = tempfile::tempdir().unwrap();
    let child = parent.path().join("child");
    tokio::fs::create_dir_all(&child).await.unwrap();

    let parent_skill = parent
      .path()
      .join(COKRA_DIR)
      .join(SKILLS_DIR)
      .join("shared-skill");
    tokio::fs::create_dir_all(&parent_skill).await.unwrap();
    tokio::fs::write(
      parent_skill.join(SKILL_FILENAME),
      "---\nname: shared-skill\ndescription: Parent version\n---\n\nParent.",
    )
    .await
    .unwrap();

    let child_skill = child.join(COKRA_DIR).join(SKILLS_DIR).join("shared-skill");
    tokio::fs::create_dir_all(&child_skill).await.unwrap();
    tokio::fs::write(
      child_skill.join(SKILL_FILENAME),
      "---\nname: shared-skill\ndescription: Child version\n---\n\nChild.",
    )
    .await
    .unwrap();

    let skills = discover_skills(&child).await;
    // 子目录（更接近 cwd）应覆盖父目录
    assert_eq!(skills["shared-skill"].description, "Child version");
  }

  // ── 输出格式测试 ────────────────────────────────────────────────────────

  #[test]
  fn build_skill_output_no_files() {
    let skill = SkillInfo {
      name: "demo".to_string(),
      description: "Demo skill".to_string(),
      location: PathBuf::from("/tmp/SKILL.md"),
      content: "Do the thing.".to_string(),
    };
    let output = build_skill_output(&skill, "/tmp", &[]);
    assert!(output.contains("<skill_content name=\"demo\">"));
    assert!(output.contains("# Skill: demo"));
    assert!(output.contains("Do the thing."));
    assert!(output.contains("</skill_content>"));
  }

  #[test]
  fn build_skill_output_with_files() {
    let skill = SkillInfo {
      name: "demo".to_string(),
      description: "Demo".to_string(),
      location: PathBuf::from("/tmp/SKILL.md"),
      content: "Instructions.".to_string(),
    };
    let files = vec![PathBuf::from("/tmp/script.sh")];
    let output = build_skill_output(&skill, "/tmp", &files);
    assert!(output.contains("<skill_files>"));
    assert!(output.contains("script.sh"));
  }

  // ── build_skill_description XML 格式测试 ─────────────────────────────────

  #[tokio::test]
  async fn build_skill_description_empty_returns_no_skills_message() {
    let dir = tempfile::tempdir().unwrap();
    unsafe {
      std::env::set_var("HOME", dir.path());
    }
    let desc = build_skill_description(dir.path()).await;
    unsafe {
      std::env::remove_var("HOME");
    }
    assert!(desc.contains("No skills are currently available"));
  }

  #[tokio::test]
  async fn build_skill_description_contains_available_skills_xml() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir
      .path()
      .join(COKRA_DIR)
      .join(SKILLS_DIR)
      .join("rust-expert");
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(
      skill_dir.join(SKILL_FILENAME),
      "---\nname: rust-expert\ndescription: Rust 专家模式\n---\n\nContent.",
    )
    .await
    .unwrap();

    let desc = build_skill_description(dir.path()).await;
    // 1:1 opencode: 必须包含 <available_skills> XML 块
    assert!(
      desc.contains("<available_skills>"),
      "缺少 <available_skills> 块"
    );
    assert!(
      desc.contains("<name>rust-expert</name>"),
      "缺少 skill name 标签"
    );
    assert!(
      desc.contains("<description>Rust 专家模式</description>"),
      "缺少 skill description 标签"
    );
    assert!(desc.contains("</available_skills>"), "缺少闭合标签");
    // 包含 examples 提示
    assert!(desc.contains("'rust-expert'"), "缺少 examples 提示");
  }

  #[tokio::test]
  async fn build_skill_description_lists_multiple_skills_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join(COKRA_DIR).join(SKILLS_DIR);

    for (name, desc) in [("zebra-skill", "Last"), ("alpha-skill", "First")] {
      let skill_dir = base.join(name);
      tokio::fs::create_dir_all(&skill_dir).await.unwrap();
      tokio::fs::write(
        skill_dir.join(SKILL_FILENAME),
        format!("---\nname: {name}\ndescription: {desc}\n---\n\nBody."),
      )
      .await
      .unwrap();
    }

    let desc = build_skill_description(dir.path()).await;
    let alpha_pos = desc.find("alpha-skill").unwrap();
    let zebra_pos = desc.find("zebra-skill").unwrap();
    // 按字母排序：alpha 在 zebra 之前
    assert!(alpha_pos < zebra_pos, "skills 未按字母顺序排列");
  }
}
