use serde::Deserialize;
use serde::Serialize;

use crate::tools::spec::ToolPermissions;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskLevel {
  #[default]
  Low,
  Medium,
  High,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
  #[default]
  Auto,
  Manual,
  Never,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolApproval {
  pub risk_level: ToolRiskLevel,
  pub approval_mode: ApprovalMode,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub permission_key: Option<String>,
  #[serde(default)]
  pub allow_network: bool,
  #[serde(default)]
  pub allow_fs_write: bool,
}

impl ToolApproval {
  pub fn from_permissions(
    permissions: &ToolPermissions,
    permission_key: Option<String>,
    mutates_state: bool,
  ) -> Self {
    let risk_level = if permissions.allow_fs_write {
      ToolRiskLevel::High
    } else if permissions.allow_network || mutates_state {
      ToolRiskLevel::Medium
    } else {
      ToolRiskLevel::Low
    };

    let approval_mode = if permissions.requires_approval || permissions.allow_fs_write {
      ApprovalMode::Manual
    } else {
      ApprovalMode::Auto
    };

    Self {
      risk_level,
      approval_mode,
      permission_key,
      allow_network: permissions.allow_network,
      allow_fs_write: permissions.allow_fs_write,
    }
  }
}
