use std::sync::Arc;

use super::auth::AuthManager;
use super::models_dev::ModelsDevDatabase;
use super::provider::ProviderInfo;
use super::provider_catalog::connect_provider_catalog;
use super::provider_catalog::find_provider_catalog_entry;
use super::registry::ProviderRegistry;

#[derive(Debug, Clone)]
pub struct PostConnectProbeResult {
  pub connect_provider_id: String,
  pub connect_provider_name: String,
  pub runtime_provider_id: String,
  pub runtime_ready: bool,
  pub used_live_models: bool,
  pub models: Vec<String>,
  pub warning: Option<String>,
}

pub async fn build_connect_catalog(auth: Option<&Arc<AuthManager>>) -> Vec<ProviderInfo> {
  let mut providers = connect_provider_catalog()
    .into_iter()
    .map(|entry| {
      ProviderInfo::new(entry.id, entry.name)
        .connect_method(entry.connect_method)
        .connectable(true)
        .env_vars(entry.env_vars())
        .models(entry.default_models())
    })
    .collect::<Vec<_>>();

  for provider in &mut providers {
    let env_connected = provider.env_vars.iter().any(|env| {
      std::env::var(env)
        .ok()
        .filter(|value| !value.is_empty())
        .is_some()
    });
    let stored_connected = if let Some(auth) = auth {
      auth.load(&provider.id).await.ok().flatten().is_some()
    } else {
      false
    };
    provider.authenticated = env_connected || stored_connected;
  }

  providers.sort_by(|a, b| a.name.cmp(&b.name));
  providers
}

pub fn build_connected_models_catalog(
  models_dev_db: &ModelsDevDatabase,
  connected: Vec<ProviderInfo>,
) -> Vec<ProviderInfo> {
  let mut results = Vec::new();
  for provider in connected {
    let Some(entry) = find_provider_catalog_entry(&provider.id) else {
      continue;
    };
    let Some(model_provider_id) = entry.primary_model_provider_id() else {
      continue;
    };

    let (models, is_live) = if entry.supports_model_runtime() {
      let models = models_from_models_dev(models_dev_db, model_provider_id.as_str(), entry.id);
      if models.is_empty() {
        (provider.models, false)
      } else {
        (models, false)
      }
    } else {
      (provider.models, false)
    };

    let mut info = ProviderInfo::new(model_provider_id, provider.name)
      .models(models)
      .authenticated(true)
      .visible(true)
      .live(is_live);
    info.options = serde_json::json!({
      "runtime_ready": entry.supports_model_runtime(),
    });
    results.push(info);
  }
  results.sort_by(|a, b| a.name.cmp(&b.name));
  results
}

pub async fn post_connect_probe(
  registry: &ProviderRegistry,
  models_dev_db: &ModelsDevDatabase,
  connect_provider_id: &str,
) -> PostConnectProbeResult {
  let Some(entry) = find_provider_catalog_entry(connect_provider_id) else {
    let models = models_from_models_dev(models_dev_db, connect_provider_id, connect_provider_id);
    return PostConnectProbeResult {
      connect_provider_id: connect_provider_id.to_string(),
      connect_provider_name: connect_provider_id.to_string(),
      runtime_provider_id: connect_provider_id.to_string(),
      runtime_ready: registry.has_provider(connect_provider_id).await,
      used_live_models: false,
      models,
      warning: Some("provider metadata not found in connect catalog".to_string()),
    };
  };

  let runtime_provider_id = entry
    .primary_model_provider_id()
    .unwrap_or_else(|| entry.id.to_string());
  let runtime_ready = registry.has_provider(runtime_provider_id.as_str()).await;
  if !runtime_ready {
    return PostConnectProbeResult {
      connect_provider_id: entry.id.to_string(),
      connect_provider_name: entry.name.to_string(),
      runtime_provider_id,
      runtime_ready: false,
      used_live_models: false,
      models: fallback_models_for_entry(models_dev_db, &entry),
      warning: Some("connected but runtime provider is not registered yet".to_string()),
    };
  }

  let Some(provider) = registry.get(runtime_provider_id.as_str()).await else {
    return PostConnectProbeResult {
      connect_provider_id: entry.id.to_string(),
      connect_provider_name: entry.name.to_string(),
      runtime_provider_id,
      runtime_ready: false,
      used_live_models: false,
      models: fallback_models_for_entry(models_dev_db, &entry),
      warning: Some("runtime provider missing from registry after connect".to_string()),
    };
  };

  match provider.list_models().await {
    Ok(response) => {
      let models = response
        .data
        .into_iter()
        .map(|model| model.id)
        .collect::<Vec<_>>();
      if models.is_empty() {
        PostConnectProbeResult {
          connect_provider_id: entry.id.to_string(),
          connect_provider_name: entry.name.to_string(),
          runtime_provider_id,
          runtime_ready: true,
          used_live_models: false,
          models: fallback_models_for_entry(models_dev_db, &entry),
          warning: Some(
            "provider returned an empty model list; using catalog fallback".to_string(),
          ),
        }
      } else {
        PostConnectProbeResult {
          connect_provider_id: entry.id.to_string(),
          connect_provider_name: entry.name.to_string(),
          runtime_provider_id,
          runtime_ready: true,
          used_live_models: true,
          models,
          warning: None,
        }
      }
    }
    Err(err) => PostConnectProbeResult {
      connect_provider_id: entry.id.to_string(),
      connect_provider_name: entry.name.to_string(),
      runtime_provider_id,
      runtime_ready: true,
      used_live_models: false,
      models: fallback_models_for_entry(models_dev_db, &entry),
      warning: Some(format!(
        "model probe failed after connect: {err}; using catalog fallback"
      )),
    },
  }
}

fn fallback_models_for_entry(
  models_dev_db: &ModelsDevDatabase,
  entry: &super::provider_catalog::ProviderCatalogEntry,
) -> Vec<String> {
  let Some(model_provider_id) = entry.primary_model_provider_id() else {
    return entry.default_models();
  };
  let mut models = models_from_models_dev(models_dev_db, model_provider_id.as_str(), entry.id);
  if models.is_empty() {
    models = entry.default_models();
  }
  models
}

fn models_from_models_dev(
  models_dev_db: &ModelsDevDatabase,
  primary_provider_id: &str,
  connect_provider_id: &str,
) -> Vec<String> {
  let provider = models_dev_db
    .get(primary_provider_id)
    .or_else(|| models_dev_db.get(connect_provider_id));
  let Some(provider) = provider else {
    return Vec::new();
  };

  let mut models = provider
    .models
    .iter()
    .filter_map(|(model_id, model)| {
      (model.status.as_deref() != Some("deprecated")).then_some(model_id.clone())
    })
    .collect::<Vec<_>>();
  models.sort();
  models
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::model::provider::ProviderConnectMethod;
  use crate::model::provider_catalog::ProviderCatalogEntry;
  use crate::model::provider_catalog::RuntimeRegistrationKind;

  #[test]
  fn fallback_models_prefers_models_dev() {
    let mut db = ModelsDevDatabase::new();
    db.insert(
      "openai".to_string(),
      super::super::models_dev::ModelsDevProvider {
        id: "openai".to_string(),
        name: "OpenAI".to_string(),
        api: None,
        env: vec![],
        npm: None,
        models: std::iter::once((
          "gpt-5".to_string(),
          super::super::models_dev::ModelsDevModel {
            id: "gpt-5".to_string(),
            name: "GPT-5".to_string(),
            family: None,
            release_date: String::new(),
            attachment: false,
            reasoning: false,
            temperature: false,
            tool_call: false,
            cost: None,
            limit: None,
            modalities: None,
            status: Some("active".to_string()),
            options: None,
            headers: None,
            provider: None,
          },
        ))
        .collect(),
      },
    );

    let entry = ProviderCatalogEntry {
      id: "openai",
      name: "OpenAI",
      connect_method: ProviderConnectMethod::ApiKey,
      env_vars: &["OPENAI_API_KEY"],
      default_models: &["gpt-4o"],
      runtime_registration: RuntimeRegistrationKind::OpenAI,
      runtime_provider_id: Some("openai"),
      oauth_client_env: None,
      visible_in_connect_catalog: true,
      plugin_kind: None,
    };

    assert_eq!(
      fallback_models_for_entry(&db, &entry),
      vec!["gpt-5".to_string()]
    );
  }
}
