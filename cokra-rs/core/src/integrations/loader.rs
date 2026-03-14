use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use crate::skills::loader::SkillScope;
use crate::skills::loader::ordered_cokra_roots;

use super::manifest::IntegrationManifest;

pub const INTEGRATIONS_DIR: &str = "integrations";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationScope {
  Project,
  User,
}

#[derive(Debug, Clone)]
pub struct LoadedIntegrationManifest {
  pub manifest: IntegrationManifest,
  pub location: PathBuf,
  pub scope: IntegrationScope,
}

#[derive(Debug, Default, Clone)]
pub struct IntegrationCatalog {
  pub manifests: Vec<LoadedIntegrationManifest>,
  pub warnings: Vec<String>,
}

pub async fn discover_integrations(cwd: &Path) -> IntegrationCatalog {
  let mut manifests_by_name: HashMap<String, LoadedIntegrationManifest> = HashMap::new();
  let mut warnings = Vec::new();

  for root in ordered_cokra_roots(cwd) {
    let Some(scope) = map_scope(root.scope) else {
      continue;
    };
    let integration_dir = root.config_dir.join(INTEGRATIONS_DIR);
    for path in collect_toml_files(&integration_dir).await {
      match parse_manifest(&path, scope).await {
        Ok(loaded) => {
          manifests_by_name.insert(loaded.manifest.name.clone(), loaded);
        }
        Err(err) => warnings.push(format!(
          "failed to load integration {}: {err}",
          path.display()
        )),
      }
    }
  }

  let mut manifests = manifests_by_name.into_values().collect::<Vec<_>>();
  manifests.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
  IntegrationCatalog {
    manifests,
    warnings,
  }
}

fn map_scope(scope: SkillScope) -> Option<IntegrationScope> {
  match scope {
    SkillScope::Project => Some(IntegrationScope::Project),
    SkillScope::User => Some(IntegrationScope::User),
    SkillScope::System => None,
  }
}

async fn parse_manifest(
  path: &Path,
  scope: IntegrationScope,
) -> Result<LoadedIntegrationManifest, String> {
  let raw = tokio::fs::read_to_string(path)
    .await
    .map_err(|err| format!("read error: {err}"))?;
  let manifest = toml::from_str::<IntegrationManifest>(&raw)
    .map_err(|err| format!("toml parse error: {err}"))?;

  Ok(LoadedIntegrationManifest {
    manifest,
    location: path.to_path_buf(),
    scope,
  })
}

async fn collect_toml_files(dir: &Path) -> Vec<PathBuf> {
  if !tokio::fs::metadata(dir)
    .await
    .map(|meta| meta.is_dir())
    .unwrap_or(false)
  {
    return Vec::new();
  }

  let mut files = Vec::new();
  let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
    return files;
  };
  while let Ok(Some(entry)) = entries.next_entry().await {
    let path = entry.path();
    let Ok(file_type) = entry.file_type().await else {
      continue;
    };
    if file_type.is_file()
      && path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("toml"))
        .unwrap_or(false)
    {
      files.push(path);
    }
  }
  files.sort();
  files
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn discover_integrations_loads_project_manifests() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join(".cokra").join(INTEGRATIONS_DIR);
    tokio::fs::create_dir_all(&dir)
      .await
      .expect("create integration dir");
    tokio::fs::write(
      dir.join("demo.toml"),
      "name = \"demo\"\nkind = \"cli\"\n[[tools]]\nid = \"echo_demo\"\ndescription = \"echo\"\ninput_schema = { type = \"object\", properties = {} }\ntype = \"command\"\ncommand = [\"echo\", \"demo\"]\n",
    )
    .await
    .expect("write manifest");

    let catalog = discover_integrations(root.path()).await;
    let demo = catalog
      .manifests
      .iter()
      .find(|loaded| loaded.manifest.name == "demo")
      .expect("project manifest discovered");
    assert_eq!(demo.scope, IntegrationScope::Project);
    assert_eq!(demo.manifest.name, "demo");
  }
}
