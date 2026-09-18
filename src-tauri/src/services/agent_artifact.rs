use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::adapters::agent_bootstrap::DeviceAbi;
use crate::services::config_service::{ConfigService, KEY_AGENT_PATH};

pub const AGENT_PATH_ENV: &str = "APP_REVERSE_TOOLS_AGENT_PATH";
pub const AGENT_RESOURCE_ARM64: &str = "aarch64/android-agent";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentArtifactSource {
    Config,
    Environment,
    Resource,
    Development,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentArtifact {
    pub path: PathBuf,
    pub abi: DeviceAbi,
    pub sha256: String,
    pub expected_agent_version: String,
    pub source: AgentArtifactSource,
}

impl AgentArtifact {
    pub fn explicit(path: &Path, abi: DeviceAbi) -> Result<Self, AgentArtifactError> {
        build_artifact(path, abi, AgentArtifactSource::Explicit)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentArtifactError {
    #[error("Agent artifact discovery only supports arm64-v8a, got {0}")]
    UnsupportedAbi(&'static str),
    #[error("configured Agent artifact from {artifact_source:?} is unavailable: {path}")]
    ExplicitPathUnavailable {
        artifact_source: AgentArtifactSource,
        path: String,
    },
    #[error("no Agent artifact was found; checked Tauri resource and workspace target")]
    NotFound,
    #[error("failed to read Agent artifact {path}: {detail}")]
    Read { path: String, detail: String },
}

pub struct AgentArtifactResolver {
    config: Arc<ConfigService>,
    resource_arm64: Option<PathBuf>,
    development_arm64: PathBuf,
}

impl AgentArtifactResolver {
    pub fn new(config: Arc<ConfigService>, resource_arm64: Option<PathBuf>) -> Self {
        let development_arm64 = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri must have a workspace parent")
            .join("target/aarch64-linux-android/release/android-agent");
        Self {
            config,
            resource_arm64,
            development_arm64,
        }
    }

    pub fn resolve(&self, abi: DeviceAbi) -> Result<AgentArtifact, AgentArtifactError> {
        self.resolve_with_environment(abi, std::env::var_os(AGENT_PATH_ENV))
    }

    fn resolve_with_environment(
        &self,
        abi: DeviceAbi,
        environment_path: Option<OsString>,
    ) -> Result<AgentArtifact, AgentArtifactError> {
        if abi != DeviceAbi::Arm64V8a {
            return Err(AgentArtifactError::UnsupportedAbi(abi.as_str()));
        }

        let configured = self
            .config
            .get(KEY_AGENT_PATH, "")
            .unwrap_or_default()
            .trim()
            .to_owned();
        if !configured.is_empty() {
            return explicit_candidate(Path::new(&configured), abi, AgentArtifactSource::Config);
        }
        if let Some(environment_path) = environment_path.filter(|path| !path.is_empty()) {
            return explicit_candidate(
                Path::new(&environment_path),
                abi,
                AgentArtifactSource::Environment,
            );
        }
        if let Some(resource) = self.resource_arm64.as_deref().filter(|path| path.is_file()) {
            return build_artifact(resource, abi, AgentArtifactSource::Resource);
        }
        if self.development_arm64.is_file() {
            return build_artifact(
                &self.development_arm64,
                abi,
                AgentArtifactSource::Development,
            );
        }
        Err(AgentArtifactError::NotFound)
    }

    #[cfg(test)]
    fn with_paths(
        config: Arc<ConfigService>,
        resource_arm64: Option<PathBuf>,
        development_arm64: PathBuf,
    ) -> Self {
        Self {
            config,
            resource_arm64,
            development_arm64,
        }
    }
}

fn explicit_candidate(
    path: &Path,
    abi: DeviceAbi,
    source: AgentArtifactSource,
) -> Result<AgentArtifact, AgentArtifactError> {
    if !path.is_file() {
        return Err(AgentArtifactError::ExplicitPathUnavailable {
            artifact_source: source,
            path: path.display().to_string(),
        });
    }
    build_artifact(path, abi, source)
}

fn build_artifact(
    path: &Path,
    abi: DeviceAbi,
    source: AgentArtifactSource,
) -> Result<AgentArtifact, AgentArtifactError> {
    Ok(AgentArtifact {
        path: path.to_owned(),
        abi,
        sha256: sha256_file(path)?,
        expected_agent_version: env!("CARGO_PKG_VERSION").into(),
        source,
    })
}

fn sha256_file(path: &Path) -> Result<String, AgentArtifactError> {
    let file = File::open(path).map_err(|error| AgentArtifactError::Read {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| AgentArtifactError::Read {
                path: path.display().to_string(),
                detail: error.to_string(),
            })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let digest = digest.finalize();
    Ok(format!("{digest:x}"))
}

#[cfg(test)]
mod tests {
    use crate::db::Db;

    use super::*;

    fn config() -> Arc<ConfigService> {
        Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())))
    }

    #[test]
    fn explicit_config_has_priority_and_invalid_explicit_path_does_not_fall_back() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("configured-agent");
        let resource = directory.path().join("resource-agent");
        let development = directory.path().join("development-agent");
        std::fs::write(&configured, b"configured").unwrap();
        std::fs::write(&resource, b"resource").unwrap();
        std::fs::write(&development, b"development").unwrap();
        let config = config();
        config
            .set(KEY_AGENT_PATH, configured.to_str().unwrap())
            .unwrap();
        let resolver =
            AgentArtifactResolver::with_paths(config.clone(), Some(resource), development);

        let artifact = resolver
            .resolve_with_environment(DeviceAbi::Arm64V8a, Some(OsString::from("/ignored")))
            .unwrap();
        assert_eq!(artifact.path, configured);
        assert_eq!(artifact.source, AgentArtifactSource::Config);
        assert_eq!(artifact.sha256.len(), 64);

        config.set(KEY_AGENT_PATH, "/missing/agent").unwrap();
        assert!(matches!(
            resolver.resolve_with_environment(DeviceAbi::Arm64V8a, None),
            Err(AgentArtifactError::ExplicitPathUnavailable { .. })
        ));
    }

    #[test]
    fn environment_then_resource_then_development_are_ordered() {
        let directory = tempfile::tempdir().unwrap();
        let environment = directory.path().join("environment-agent");
        let resource = directory.path().join("resource-agent");
        let development = directory.path().join("development-agent");
        std::fs::write(&environment, b"environment").unwrap();
        std::fs::write(&resource, b"resource").unwrap();
        std::fs::write(&development, b"development").unwrap();
        let resolver = AgentArtifactResolver::with_paths(
            config(),
            Some(resource.clone()),
            development.clone(),
        );

        let from_environment = resolver
            .resolve_with_environment(
                DeviceAbi::Arm64V8a,
                Some(environment.clone().into_os_string()),
            )
            .unwrap();
        assert_eq!(from_environment.source, AgentArtifactSource::Environment);
        assert_eq!(from_environment.path, environment);

        let from_resource = resolver
            .resolve_with_environment(DeviceAbi::Arm64V8a, None)
            .unwrap();
        assert_eq!(from_resource.source, AgentArtifactSource::Resource);
        std::fs::remove_file(resource).unwrap();
        let from_development = resolver
            .resolve_with_environment(DeviceAbi::Arm64V8a, None)
            .unwrap();
        assert_eq!(from_development.source, AgentArtifactSource::Development);
        assert_eq!(from_development.path, development);
    }

    #[test]
    fn artifact_sha256_is_stable_and_other_abis_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let development = directory.path().join("android-agent");
        std::fs::write(&development, b"abc").unwrap();
        let resolver = AgentArtifactResolver::with_paths(config(), None, development);
        let artifact = resolver
            .resolve_with_environment(DeviceAbi::Arm64V8a, None)
            .unwrap();
        assert_eq!(
            artifact.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(matches!(
            resolver.resolve_with_environment(DeviceAbi::X86_64, None),
            Err(AgentArtifactError::UnsupportedAbi("x86_64"))
        ));
    }
}
