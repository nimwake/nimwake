//! Application registry.
//!
//! 요구사항: PRD §21 Manifest Validation, §23 Gateway Routing.
//!
//! 검증은 daemon 기동 단계에서 수행한다. 검증 실패를 런타임까지 미루면
//! cold start 시점에야 port 충돌이 드러난다.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::app::{self, AppHandle, AppSpec};
use crate::error::{ManifestError, ValidationError};
use crate::runtime::Runtime;

/// Gateway 기본 listen port. (PRD §23)
pub const DEFAULT_GATEWAY_PORT: u16 = 8080;

const MANIFEST_FILE_NAME: &str = "nimwake.toml";

#[derive(Debug, Clone)]
pub struct AppRegistry {
    by_host: HashMap<String, AppHandle>,
    by_name: HashMap<String, AppHandle>,
}

impl AppRegistry {
    /// 검증을 통과한 spec들로 actor를 띄우고 registry를 만든다.
    ///
    /// 하나라도 검증에 실패하면 actor를 띄우지 않고 실패를 돌려준다.
    pub fn build(
        specs: Vec<AppSpec>,
        gateway_port: u16,
        runtime: Arc<dyn Runtime>,
    ) -> Result<Self, ValidationError> {
        validate(&specs, gateway_port)?;

        let mut by_host = HashMap::with_capacity(specs.len());
        let mut by_name = HashMap::with_capacity(specs.len());

        for spec in specs {
            let host = normalize_host(&spec.server.host);
            let name = spec.name.clone();
            let handle = app::spawn(Arc::new(spec), Arc::clone(&runtime));
            by_host.insert(host, handle.clone());
            by_name.insert(name, handle);
        }

        Ok(Self { by_host, by_name })
    }

    /// `Host` header로 application을 찾는다. port suffix와 대소문자는 무시한다.
    #[must_use]
    pub fn resolve(&self, host: &str) -> Option<&AppHandle> {
        self.by_host.get(&normalize_host(host))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&AppHandle> {
        self.by_name.get(name)
    }

    pub fn apps(&self) -> impl Iterator<Item = &AppHandle> {
        self.by_name.values()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// 모든 application을 정지시키고 actor를 종료한다.
    ///
    /// child process의 수명은 daemon의 수명을 넘지 않는다. (Invariant 10 / PRD §71)
    ///
    /// **process가 실제로 사라질 때까지 기다린다.** 정지 요청만 보내고 반환하면
    /// daemon이 먼저 종료해 stop task가 끝나지 못하고 child가 남는다.
    /// app 수만큼 순차로 기다리면 종료가 그만큼 느려지므로 동시에 진행한다.
    pub async fn shutdown(&self) {
        let mut pending = Vec::with_capacity(self.len());

        for handle in self.apps() {
            let handle = handle.clone();
            pending.push(tokio::spawn(async move { handle.shutdown().await }));
        }

        for task in pending {
            let _ = task.await;
        }
    }
}

/// registry 전체 수준의 검증.
///
/// ```text
/// unique name
/// unique host
/// unique port
/// port != gateway port
/// ```
pub fn validate(specs: &[AppSpec], gateway_port: u16) -> Result<(), ValidationError> {
    let mut names: HashMap<&str, &str> = HashMap::new();
    let mut hosts: HashMap<String, &str> = HashMap::new();
    let mut ports: HashMap<u16, &str> = HashMap::new();

    for spec in specs {
        spec.validate()?;

        if names
            .insert(spec.name.as_str(), spec.name.as_str())
            .is_some()
        {
            return Err(ValidationError::DuplicateName {
                name: spec.name.clone(),
            });
        }

        let host = normalize_host(&spec.server.host);
        if let Some(first) = hosts.insert(host.clone(), spec.name.as_str()) {
            return Err(ValidationError::DuplicateHost {
                host,
                first: first.to_owned(),
                second: spec.name.clone(),
            });
        }

        if spec.server.port == gateway_port {
            return Err(ValidationError::GatewayPortConflict {
                app: spec.name.clone(),
                port: gateway_port,
            });
        }

        if let Some(first) = ports.insert(spec.server.port, spec.name.as_str()) {
            return Err(ValidationError::DuplicatePort {
                port: spec.server.port,
                first: first.to_owned(),
                second: spec.name.clone(),
            });
        }
    }

    Ok(())
}

/// 디렉터리에서 manifest를 읽는다.
///
/// `<dir>/nimwake.toml`이 있으면 그것 하나를, 없으면 `<dir>/*/nimwake.toml`을 모두 읽는다.
pub fn load_dir(dir: impl AsRef<Path>) -> Result<Vec<AppSpec>, ManifestError> {
    let dir = dir.as_ref();

    let single = dir.join(MANIFEST_FILE_NAME);
    if single.is_file() {
        return Ok(vec![AppSpec::load(&single)?]);
    }

    let entries = std::fs::read_dir(dir).map_err(|source| ManifestError::Read {
        path: dir.to_path_buf(),
        source,
    })?;

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ManifestError::Read {
            path: dir.to_path_buf(),
            source,
        })?;
        let manifest = entry.path().join(MANIFEST_FILE_NAME);
        if manifest.is_file() {
            paths.push(manifest);
        }
    }

    // 파일시스템 순서에 의존하지 않도록 정렬한다. 검증 오류 메시지를 재현 가능하게 만든다.
    paths.sort();

    paths.iter().map(AppSpec::load).collect()
}

fn normalize_host(host: &str) -> String {
    let host = host.trim();
    let host = host.split(':').next().unwrap_or(host);
    host.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, host: &str, port: u16) -> AppSpec {
        AppSpec::from_toml(&format!(
            r#"
name = "{name}"

[server]
host = "{host}"
port = {port}

[runtime]
command = ["true"]
working_dir = "."
"#
        ))
        .expect("manifest parses")
    }

    #[test]
    fn accepts_distinct_apps() {
        let specs = vec![
            spec("blog", "blog.localhost", 3101),
            spec("api", "api.localhost", 3102),
        ];
        assert_eq!(validate(&specs, DEFAULT_GATEWAY_PORT), Ok(()));
    }

    #[test]
    fn rejects_duplicate_port() {
        let specs = vec![
            spec("blog", "blog.localhost", 3101),
            spec("api", "api.localhost", 3101),
        ];
        assert_eq!(
            validate(&specs, DEFAULT_GATEWAY_PORT),
            Err(ValidationError::DuplicatePort {
                port: 3101,
                first: "blog".to_owned(),
                second: "api".to_owned(),
            })
        );
    }

    #[test]
    fn rejects_duplicate_host_ignoring_case() {
        let specs = vec![
            spec("blog", "Blog.localhost", 3101),
            spec("api", "blog.localhost", 3102),
        ];
        assert!(matches!(
            validate(&specs, DEFAULT_GATEWAY_PORT),
            Err(ValidationError::DuplicateHost { .. })
        ));
    }

    #[test]
    fn rejects_gateway_port_conflict() {
        let specs = vec![spec("blog", "blog.localhost", DEFAULT_GATEWAY_PORT)];
        assert_eq!(
            validate(&specs, DEFAULT_GATEWAY_PORT),
            Err(ValidationError::GatewayPortConflict {
                app: "blog".to_owned(),
                port: DEFAULT_GATEWAY_PORT,
            })
        );
    }

    #[test]
    fn rejects_duplicate_name() {
        let specs = vec![
            spec("blog", "a.localhost", 3101),
            spec("blog", "b.localhost", 3102),
        ];
        assert_eq!(
            validate(&specs, DEFAULT_GATEWAY_PORT),
            Err(ValidationError::DuplicateName {
                name: "blog".to_owned()
            })
        );
    }

    #[test]
    fn normalizes_host_with_port_and_case() {
        assert_eq!(normalize_host("Hello.Localhost:8080"), "hello.localhost");
    }
}
