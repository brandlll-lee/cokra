use cokra_core::tool_runtime::BuiltinToolProvider;
use cokra_core::tool_runtime::ToolProvider;
use cokra_core::tools::CONTAINER_EXEC_TOOL_ALIAS;
use cokra_core::tools::LOCAL_SHELL_TOOL_ALIAS;
use cokra_core::tools::ToolRegistry;
use cokra_core::tools::UNIFIED_EXEC_TOOL_NAME;
use cokra_core::tools::spec::build_specs;
use serde_json::json;
use serde_json::Value;

fn canonicalize_json(value: Value) -> Value {
  match value {
    Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_json).collect()),
    Value::Object(map) => {
      let mut entries = map.into_iter().collect::<Vec<_>>();
      entries.sort_by(|(left, _), (right, _)| left.cmp(right));
      Value::Object(
        entries
          .into_iter()
          .map(|(key, value)| (key, canonicalize_json(value)))
          .collect(),
      )
    }
    other => other,
  }
}

fn schema_shape(schema: &Value) -> Value {
  match schema.get("type").and_then(Value::as_str) {
    Some("object") => {
      let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
          properties
            .iter()
            .map(|(key, value)| (key.clone(), schema_shape(value)))
            .collect::<serde_json::Map<String, Value>>()
        })
        .unwrap_or_default();
      json!({
        "type": "object",
        "required": schema.get("required").cloned().unwrap_or_else(|| json!([])),
        "properties": properties
      })
    }
    Some("array") => json!({
      "type": "array",
      "items": schema.get("items").map(schema_shape).unwrap_or_else(|| json!(null))
    }),
    Some(kind) => json!({ "type": kind }),
    None => schema.clone(),
  }
}

#[tokio::test]
async fn builtin_tool_schema_baseline_snapshot() -> anyhow::Result<()> {
  let mut registry = ToolRegistry::new();
  for spec in build_specs() {
    registry.register_spec(spec);
  }
  registry.register_alias(LOCAL_SHELL_TOOL_ALIAS, UNIFIED_EXEC_TOOL_NAME);
  registry.register_alias(CONTAINER_EXEC_TOOL_ALIAS, UNIFIED_EXEC_TOOL_NAME);

  let provider = BuiltinToolProvider::from_registry(&registry);
  let mut selected = provider
    .list_tools()
    .await?
    .into_iter()
    .filter(|tool| {
      matches!(
        tool.name.as_str(),
        "shell" | "unified_exec" | "web_search" | "web_fetch" | "diagnostics"
      )
    })
    .map(|tool| {
      json!({
        "name": tool.name,
        "aliases": tool.aliases,
        "source": tool.source,
        "source_kind": tool.source_kind,
        "approval": tool.approval,
        "supports_parallel": tool.supports_parallel,
        "mutates_state": tool.mutates_state,
        "input_keys": tool.input_keys,
        "input_schema": schema_shape(&tool.input_schema)
      })
    })
    .collect::<Vec<_>>();
  selected.sort_by(|left, right| {
    left["name"]
      .as_str()
      .cmp(&right["name"].as_str())
  });
  let selected = canonicalize_json(Value::Array(selected));

  insta::assert_json_snapshot!(
    selected,
    @r###"
[
  {
    "aliases": [],
    "approval": {
      "allow_fs_write": false,
      "allow_network": false,
      "approval_mode": "auto",
      "permission_key": "read",
      "risk_level": "low"
    },
    "input_keys": [
      "max_diagnostics",
      "path"
    ],
    "input_schema": {
      "properties": {
        "max_diagnostics": {
          "type": "number"
        },
        "path": {
          "type": "string"
        }
      },
      "required": [
        "path"
      ],
      "type": "object"
    },
    "mutates_state": false,
    "name": "diagnostics",
    "source": "builtin",
    "source_kind": "builtin_primitive",
    "supports_parallel": true
  },
  {
    "aliases": [],
    "approval": {
      "allow_fs_write": true,
      "allow_network": false,
      "approval_mode": "manual",
      "permission_key": "exec",
      "risk_level": "high"
    },
    "input_keys": [
      "additional_permissions",
      "command",
      "justification",
      "prefix_rule",
      "sandbox_permissions",
      "timeout_ms",
      "workdir"
    ],
    "input_schema": {
      "properties": {
        "additional_permissions": {
          "properties": {
            "file_system": {
              "properties": {
                "read": {
                  "items": {
                    "type": "string"
                  },
                  "type": "array"
                },
                "write": {
                  "items": {
                    "type": "string"
                  },
                  "type": "array"
                }
              },
              "required": [],
              "type": "object"
            },
            "macos": {
              "properties": {},
              "required": [],
              "type": "object"
            },
            "network": {
              "properties": {
                "enabled": {
                  "type": "boolean"
                }
              },
              "required": [],
              "type": "object"
            }
          },
          "required": [],
          "type": "object"
        },
        "command": {
          "type": "string"
        },
        "justification": {
          "type": "string"
        },
        "prefix_rule": {
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "sandbox_permissions": {
          "type": "string"
        },
        "timeout_ms": {
          "type": "number"
        },
        "workdir": {
          "type": "string"
        }
      },
      "required": [
        "command"
      ],
      "type": "object"
    },
    "mutates_state": true,
    "name": "shell",
    "source": "builtin",
    "source_kind": "builtin_primitive",
    "supports_parallel": false
  },
  {
    "aliases": [
      "container.exec",
      "local_shell"
    ],
    "approval": {
      "allow_fs_write": true,
      "allow_network": false,
      "approval_mode": "manual",
      "permission_key": "exec",
      "risk_level": "high"
    },
    "input_keys": [
      "additional_permissions",
      "command",
      "justification",
      "prefix_rule",
      "sandbox_permissions",
      "timeout_ms",
      "workdir"
    ],
    "input_schema": {
      "properties": {
        "additional_permissions": {
          "properties": {
            "file_system": {
              "properties": {
                "read": {
                  "items": {
                    "type": "string"
                  },
                  "type": "array"
                },
                "write": {
                  "items": {
                    "type": "string"
                  },
                  "type": "array"
                }
              },
              "required": [],
              "type": "object"
            },
            "macos": {
              "properties": {},
              "required": [],
              "type": "object"
            },
            "network": {
              "properties": {
                "enabled": {
                  "type": "boolean"
                }
              },
              "required": [],
              "type": "object"
            }
          },
          "required": [],
          "type": "object"
        },
        "command": {
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "justification": {
          "type": "string"
        },
        "prefix_rule": {
          "items": {
            "type": "string"
          },
          "type": "array"
        },
        "sandbox_permissions": {
          "type": "string"
        },
        "timeout_ms": {
          "type": "number"
        },
        "workdir": {
          "type": "string"
        }
      },
      "required": [
        "command"
      ],
      "type": "object"
    },
    "mutates_state": true,
    "name": "unified_exec",
    "source": "builtin",
    "source_kind": "builtin_primitive",
    "supports_parallel": false
  },
  {
    "aliases": [],
    "approval": {
      "allow_fs_write": false,
      "allow_network": true,
      "approval_mode": "manual",
      "permission_key": "web",
      "risk_level": "medium"
    },
    "input_keys": [
      "format",
      "timeout",
      "url"
    ],
    "input_schema": {
      "properties": {
        "format": {
          "type": "string"
        },
        "timeout": {
          "type": "number"
        },
        "url": {
          "type": "string"
        }
      },
      "required": [
        "url"
      ],
      "type": "object"
    },
    "mutates_state": false,
    "name": "web_fetch",
    "source": "builtin",
    "source_kind": "builtin_primitive",
    "supports_parallel": true
  },
  {
    "aliases": [],
    "approval": {
      "allow_fs_write": false,
      "allow_network": true,
      "approval_mode": "manual",
      "permission_key": "web",
      "risk_level": "medium"
    },
    "input_keys": [
      "context_max_characters",
      "livecrawl",
      "num_results",
      "query"
    ],
    "input_schema": {
      "properties": {
        "context_max_characters": {
          "type": "number"
        },
        "livecrawl": {
          "type": "string"
        },
        "num_results": {
          "type": "number"
        },
        "query": {
          "type": "string"
        }
      },
      "required": [
        "query"
      ],
      "type": "object"
    },
    "mutates_state": false,
    "name": "web_search",
    "source": "builtin",
    "source_kind": "builtin_primitive",
    "supports_parallel": true
  }
]
"###
  );

  Ok(())
}
