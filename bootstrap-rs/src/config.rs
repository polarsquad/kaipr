//! Repository-owned bootstrap configuration (`bootstrap.toml`).
//!
//! Everything repository-specific the binary needs is declared in this
//! file; the binary itself is a generic bootstrap engine. The environment
//! knobs (`Config::from_env`, see main.rs) keep their precedence over these
//! values at runtime. The file is located via `BOOTSTRAP_CONFIG` or
//! defaults to `./bootstrap.toml`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;

/// Root of the parsed `bootstrap.toml`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapConfig {
    pub bootstrap: BootstrapSection,
    pub charts: IndexMap<String, String>,
    /// Environments keyed by section name, in file order (error messages
    /// list environments in file order).
    pub environments: IndexMap<String, Environment>,
}

/// Global names carried in `[bootstrap]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BootstrapSection {
    pub default_environment: String,
    pub kind_cluster: String,
    pub kind_context: String,
    pub registry_name: String,
    pub flux_namespace: String,
}

/// The Flux sync source for an environment (`[environments.<name>].sync`):
/// where the cluster's FluxInstance pulls its configuration from. The
/// repository ships the OCI source only: the artifact is published from
/// the checkout to the local registry, so the stack carries no plaintext
/// secrets and no Git clone credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncSource {
    /// OCI artifact published from the checkout to the local registry.
    Oci,
}

/// One `[environments.<name>]` section.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Environment {
    pub kind: String,
    pub sync: SyncSource,
    /// Root path of the artifact the FluxInstance reconciles (relative to
    /// the artifact root, which is the repository root).
    pub sync_path: String,
}

impl BootstrapConfig {
    /// Locate and parse the repository config file. `BOOTSTRAP_CONFIG`
    /// overrides the default `./bootstrap.toml` location.
    pub fn locate_and_load() -> Result<Self> {
        let path = Self::locate();
        Self::load_from(&path)
    }

    /// The config path: `BOOTSTRAP_CONFIG` when set (non-empty), else
    /// `./bootstrap.toml` in the working directory.
    fn locate() -> PathBuf {
        std::env::var_os("BOOTSTRAP_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("bootstrap.toml"))
    }

    /// Parse a config file from disk, with validation beyond deserialization.
    pub fn load_from(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read bootstrap config '{path:?}'"))?;
        let config: BootstrapConfig = toml::from_str(&raw)
            .with_context(|| format!("failed to parse bootstrap config '{path:?}'"))?;
        config.validate()?;
        Ok(config)
    }

    /// Cross-field validation done after parsing (the Python cross-check
    /// covers repo-side consistency; this covers config-internal rules).
    fn validate(&self) -> Result<()> {
        if self.environments.is_empty() {
            bail!("no [environments.*] sections in bootstrap config");
        }
        for (name, env) in &self.environments {
            if env.kind != name.as_str() {
                bail!(
                    "environments.{name} kind '{}' must match its section name",
                    env.kind
                );
            }
        }
        let default = &self.bootstrap.default_environment;
        if !self.environments.contains_key(default) {
            bail!("bootstrap.default-environment '{default}' is not an [environments.*] section");
        }
        Ok(())
    }

    /// Resolve an environment by name (KAIPR_PROFILE value or positional
    /// argument); the error lists the valid names in file order.
    pub fn environment(&self, name: &str) -> Result<&Environment> {
        self.environments
            .get(name)
            .with_context(|| self.unsupported_message(name))
    }

    /// The error message for an unknown environment name; the script's
    /// wording, listing valid names in file order.
    fn unsupported_message(&self, name: &str) -> String {
        let names: Vec<String> = self.environments.keys().map(|n| format!("'{n}'")).collect();
        format!(
            "unsupported profile '{name}' (expected {})",
            names.join(" or ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r##"
[bootstrap]
default-environment = "local-host"
kind-cluster = "kaipr"
kind-context = "kind-kaipr"
registry-name = "kaipr-registry"
flux-namespace = "flux-system"

[charts]
flux-operator = "0.61.0"

[environments.local-host]
kind = "local-host"
sync = "oci"
sync-path = "workload/local-host"

[environments.dev-laptop]
kind = "dev-laptop"
sync = "oci"
sync-path = "workload/local-host"
"##;

    fn parse(text: &str) -> Result<BootstrapConfig> {
        let config: BootstrapConfig = toml::from_str(text).unwrap();
        config.validate()?;
        Ok(config)
    }

    #[test]
    fn parses_and_resolves_environments() {
        let config = parse(MINIMAL).unwrap();
        assert_eq!(config.bootstrap.default_environment, "local-host");
        assert_eq!(config.bootstrap.kind_cluster, "kaipr");
        assert_eq!(config.bootstrap.kind_context, "kind-kaipr");
        assert_eq!(config.charts["flux-operator"], "0.61.0");

        let local = config.environment("local-host").unwrap();
        assert_eq!(local.sync, SyncSource::Oci);
        assert_eq!(local.sync_path, "workload/local-host");
    }

    #[test]
    fn environment_order_preserved_from_file() {
        // Error messages list environments in file order
        // ('local-host' before 'dev-laptop').
        let config = parse(MINIMAL).unwrap();
        let msg = config.unsupported_message("bogus");
        assert_eq!(
            msg,
            "unsupported profile 'bogus' (expected 'local-host' or 'dev-laptop')"
        );
    }

    #[test]
    fn unknown_environment_error_lists_file_order() {
        let config = parse(MINIMAL).unwrap();
        let err = config.environment("talos-local").unwrap_err();
        assert_eq!(
            err.to_string(),
            "unsupported profile 'talos-local' (expected 'local-host' or 'dev-laptop')"
        );
    }

    #[test]
    fn rejects_unknown_field() {
        let bad = MINIMAL.replace("kind-cluster", "kind_cluster_typo");
        assert!(toml::from_str::<BootstrapConfig>(&bad).is_err());
    }

    #[test]
    fn rejects_legacy_pivot_fields() {
        // The pre-single-cluster schema must fail loudly, not silently
        // ignore the removed keys.
        let bad = format!(
            "{MINIMAL}\n[environments.dev-laptop]\nkind = \"dev-laptop\"\nsync = \"oci\"\n\
             sync-path = \"workload/local-host\"\nmgmt-cluster = \"dev-management\"\n"
        );
        assert!(toml::from_str::<BootstrapConfig>(&bad).is_err());
    }

    #[test]
    fn rejects_legacy_github_sync_source() {
        let bad = format!(
            "{MINIMAL}\n[environments.github-env]\nkind = \"github-env\"\nsync = \"github\"\n\
             sync-path = \"mgmt/aws\"\n"
        );
        assert!(toml::from_str::<BootstrapConfig>(&bad).is_err());
    }

    #[test]
    fn rejects_kind_section_mismatch() {
        let bad = MINIMAL.replace(
            "[environments.dev-laptop]\nkind = \"dev-laptop\"",
            "[environments.dev-laptop]\nkind = \"local-host\"",
        );
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn rejects_unknown_default_environment() {
        let bad = MINIMAL.replace(
            "default-environment = \"local-host\"",
            "default-environment = \"gcp\"",
        );
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn rejects_empty_environments() {
        let bad = MINIMAL.split("[environments.local-host]").next().unwrap();
        // Deserialization itself rejects a missing environments table.
        assert!(toml::from_str::<BootstrapConfig>(bad).is_err());
        // An explicitly empty one is caught by validate().
        let empty = format!("{bad}[environments]\n");
        let config: BootstrapConfig = toml::from_str(&empty).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn loads_the_repository_config() {
        // The checked-in file must parse and validate as shipped.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let config = BootstrapConfig::load_from(&root.join("../bootstrap.toml")).unwrap();
        assert!(config.environments.contains_key("local-host"));
        assert_eq!(config.environments.len(), 1);
        assert_eq!(config.charts.len(), 1);
        let local_host = config.environment("local-host").unwrap();
        assert_eq!(local_host.sync, SyncSource::Oci);
        assert_eq!(local_host.sync_path, "workload/local-host");
    }

    #[test]
    fn missing_config_file_is_a_clean_error() {
        let err = BootstrapConfig::load_from(Path::new("/nonexistent/bootstrap.toml")).unwrap_err();
        assert!(err.to_string().contains("failed to read"));
    }
}
