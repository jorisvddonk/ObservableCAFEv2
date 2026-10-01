use serde::{Deserialize, Serialize};

/// The set of models a single LLM backend serves.
///
/// `backend` is the value a session sets in `config.llm.backend` to route to
/// this provider; `default_model` is what it uses when a session selects the
/// backend but no explicit `config.llm.model`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendModels {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    pub models: Vec<String>,
}

/// The full LLM model catalog published to the registry session by cafe-llm.
///
/// This is the authoritative structure the web UI reads to offer model
/// selection: it knows both the model name and which backend serves it, so
/// choosing a model also selects the correct backend.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModelCatalog {
    /// The backend a session uses when it sets no `config.llm.backend`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_backend: Option<String>,
    /// Every configured backend and the models it serves.
    #[serde(default)]
    pub backends: Vec<BackendModels>,
}

impl ModelCatalog {
    /// Flat, de-duplicated list of every model across all backends, sorted.
    pub fn all_models(&self) -> Vec<String> {
        let mut models: Vec<String> = self
            .backends
            .iter()
            .flat_map(|b| b.models.iter().cloned())
            .collect();
        models.sort();
        models.dedup();
        models
    }

    /// Resolve the backend that serves `model`, preferring `preferred` when
    /// several backends offer it.
    pub fn backend_for(&self, model: &str, preferred: Option<&str>) -> Option<&BackendModels> {
        let candidates: Vec<&BackendModels> = self
            .backends
            .iter()
            .filter(|b| b.models.iter().any(|m| m == model))
            .collect();
        if let Some(pref) = preferred {
            if let Some(hit) = candidates.iter().find(|b| b.backend == pref) {
                return Some(hit);
            }
        }
        candidates.first().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> ModelCatalog {
        ModelCatalog {
            default_backend: Some("ollama".into()),
            backends: vec![
                BackendModels {
                    backend: "alpha".into(),
                    default_model: Some("alpha-small".into()),
                    models: vec!["alpha-small".into(), "shared".into()],
                },
                BackendModels {
                    backend: "beta".into(),
                    default_model: Some("beta-large".into()),
                    models: vec!["beta-large".into(), "shared".into(), "beta-x".into()],
                },
            ],
        }
    }

    #[test]
    fn all_models_is_sorted_and_deduped() {
        assert_eq!(
            catalog().all_models(),
            vec!["alpha-small", "beta-large", "beta-x", "shared"]
        );
    }

    #[test]
    fn backend_for_prefers_requested_backend() {
        let c = catalog();
        assert_eq!(
            c.backend_for("shared", Some("beta")).unwrap().backend,
            "beta"
        );
        assert_eq!(
            c.backend_for("shared", Some("alpha")).unwrap().backend,
            "alpha"
        );
        // No preference: first matching backend wins.
        assert_eq!(c.backend_for("shared", None).unwrap().backend, "alpha");
    }

    #[test]
    fn backend_for_unknown_model_is_none() {
        assert!(catalog().backend_for("nope", None).is_none());
    }

    #[test]
    fn serde_round_trips() {
        let c = catalog();
        let json = serde_json::to_string(&c).unwrap();
        let back: ModelCatalog = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
