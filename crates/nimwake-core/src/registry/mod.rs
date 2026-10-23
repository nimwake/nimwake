//! Application registry.
//!
//! 요구사항: PRD §21 Manifest Validation, §23 Gateway Routing, §76 Manifest Reload.
//!
//! 검증은 daemon 기동 단계에서 수행한다. 검증 실패를 런타임까지 미루면
//! cold start 시점에야 port 충돌이 드러난다.
//!
//! 재적재(§76) 때문에 내부가 잠금 뒤에 있다. 요청 경로는 읽기 잠금을 한 번
//! 잡고 handle을 복제해 나온다. `AppHandle`은 `Arc`와 channel sender뿐이라
//! 복제가 싸고, **잠금을 들고 await하지 않는다.**

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use tracing::info;

use crate::app::{self, AppHandle, AppSpec};
use crate::error::{ManifestError, ValidationError};
use crate::limits::Limits;
use crate::runtime::Runtime;

/// Gateway 기본 listen port. (PRD §23)
pub const DEFAULT_GATEWAY_PORT: u16 = 8080;

const MANIFEST_FILE_NAME: &str = "nimwake.toml";

/// 재적재가 무엇을 바꿨는지. (PRD §76)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReloadOutcome {
    /// 새로 등록된 application 이름.
    pub added: Vec<String>,
    /// manifest에서 사라져 정지된 application 이름.
    pub removed: Vec<String>,
    /// spec이 바뀌어 정지된 application 이름. 다음 요청이 새 spec으로 기동한다.
    pub replaced: Vec<String>,
    /// spec이 같아 **건드리지 않은** application 이름.
    pub unchanged: Vec<String>,
}

impl ReloadOutcome {
    #[must_use]
    pub fn changed(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty() || !self.replaced.is_empty()
    }
}

struct Inner {
    by_host: HashMap<String, AppHandle>,
    by_name: HashMap<String, AppHandle>,
}

impl Inner {
    fn from_specs(specs: Vec<AppSpec>, runtime: &Arc<dyn Runtime>, limits: Limits) -> Self {
        let mut by_host = HashMap::with_capacity(specs.len());
        let mut by_name = HashMap::with_capacity(specs.len());

        for spec in specs {
            let host = normalize_host(&spec.server.host);
            let name = spec.name.clone();
            let handle = app::spawn_with_limits(Arc::new(spec), Arc::clone(runtime), limits);
            by_host.insert(host, handle.clone());
            // alias도 같은 handle을 가리킨다. (PRD §87)
            for alias in &handle.spec().server.aliases {
                by_host.insert(normalize_host(alias), handle.clone());
            }
            by_name.insert(name, handle);
        }

        Self { by_host, by_name }
    }

    fn insert(&mut self, name: String, handle: AppHandle) {
        self.by_host
            .insert(normalize_host(&handle.spec().server.host), handle.clone());
        // alias도 같은 handle을 가리킨다. (PRD §87)
        for alias in &handle.spec().server.aliases {
            self.by_host.insert(normalize_host(alias), handle.clone());
        }
        self.by_name.insert(name, handle);
    }
}

/// 잠금 뒤의 맵을 그대로 찍지 않는다. 등록된 수만 보여 준다.
///
/// `Debug`를 파생할 수 없다. `Arc<dyn Runtime>`이 `Debug`가 아니고, 맵을 찍으려면
/// 잠금을 잡아야 한다. `Debug`가 잠금을 잡으면 로그 한 줄이 요청 경로와 다툰다.
impl std::fmt::Debug for AppRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppRegistry")
            .field("gateway_port", &self.gateway_port)
            .finish_non_exhaustive()
    }
}

pub struct AppRegistry {
    inner: RwLock<Inner>,
    runtime: Arc<dyn Runtime>,
    gateway_port: u16,
    limits: Limits,
}

impl AppRegistry {
    /// 검증을 통과한 spec들로 actor를 띄우고 registry를 만든다.
    ///
    /// 하나라도 검증에 실패하면 actor를 띄우지 않고 실패를 돌려준다.
    pub fn build(
        specs: Vec<AppSpec>,
        gateway_port: u16,
        runtime: Arc<dyn Runtime>,
        limits: Limits,
    ) -> Result<Self, ValidationError> {
        validate(&specs, gateway_port)?;

        Ok(Self {
            inner: RwLock::new(Inner::from_specs(specs, &runtime, limits)),
            runtime,
            gateway_port,
            limits,
        })
    }

    /// `Host` header로 application을 찾는다. port suffix와 대소문자는 무시한다.
    #[must_use]
    pub fn resolve(&self, host: &str) -> Option<AppHandle> {
        let host = normalize_host(host);
        self.read().by_host.get(&host).cloned()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<AppHandle> {
        self.read().by_name.get(name).cloned()
    }

    #[must_use]
    pub fn apps(&self) -> Vec<AppHandle> {
        self.read().by_name.values().cloned().collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.read().by_name.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read().by_name.is_empty()
    }

    /// manifest를 다시 읽어 **차이만** 적용한다. (PRD §76)
    ///
    /// 검증이 실패하면 아무것도 바꾸지 않는다. §22의 검증은 집합 전체를 보므로
    /// (unique host / unique port) 일부만 적용하면 어떤 manifest 파일과도
    /// 일치하지 않는 상태가 남는다.
    ///
    /// spec이 같은 application은 건드리지 않는다. **그것이 이 기능의 전부다.**
    /// 정지는 §30의 graceful shutdown을 그대로 쓰며 lease 반납을 기다리지 않는다.
    ///
    /// # Errors
    ///
    /// 새 집합이 검증을 통과하지 못하면 그 오류를 돌려준다. 기존 상태는 그대로다.
    pub async fn reload(&self, specs: Vec<AppSpec>) -> Result<ReloadOutcome, ValidationError> {
        validate(&specs, self.gateway_port)?;

        let mut incoming: HashMap<String, AppSpec> = specs
            .into_iter()
            .map(|spec| (spec.name.clone(), spec))
            .collect();

        let mut outcome = ReloadOutcome::default();
        let mut retired: Vec<AppHandle> = Vec::new();
        let mut keep: Vec<(String, AppHandle)> = Vec::new();

        // 무엇을 바꿀지 먼저 정한다. 잠금을 들고 await하지 않는다.
        {
            let current = self.read();
            for (name, handle) in &current.by_name {
                match incoming.get(name) {
                    Some(spec) if spec == handle.spec().as_ref() => {
                        outcome.unchanged.push(name.clone());
                        keep.push((name.clone(), handle.clone()));
                        incoming.remove(name);
                    }
                    Some(_) => {
                        outcome.replaced.push(name.clone());
                        retired.push(handle.clone());
                    }
                    None => {
                        outcome.removed.push(name.clone());
                        retired.push(handle.clone());
                    }
                }
            }
        }

        // 남은 것은 새로 띄울 것이다. 이름이 기존에 있었으면 교체, 없었으면 추가.
        for name in incoming.keys() {
            if !outcome.replaced.contains(name) {
                outcome.added.push(name.clone());
            }
        }

        let fresh: Vec<AppSpec> = incoming.into_values().collect();

        for list in [
            &mut outcome.added,
            &mut outcome.removed,
            &mut outcome.replaced,
            &mut outcome.unchanged,
        ] {
            list.sort();
        }

        // 새 actor를 띄우고 유지할 것과 합쳐 **한 번에** 교체한다.
        {
            let mut next = Inner::from_specs(fresh, &self.runtime, self.limits);
            for (name, handle) in keep {
                next.insert(name, handle);
            }
            *self.write() = next;
        }

        // 교체가 끝난 뒤에 정지시킨다. 이 시점부터 routing은 새 집합을 본다.
        let mut pending = Vec::with_capacity(retired.len());
        for handle in retired {
            pending.push(tokio::spawn(async move { handle.shutdown().await }));
        }
        for task in pending {
            let _ = task.await;
        }

        info!(
            added = outcome.added.len(),
            removed = outcome.removed.len(),
            replaced = outcome.replaced.len(),
            unchanged = outcome.unchanged.len(),
            "manifests reloaded",
        );

        Ok(outcome)
    }

    /// 모든 application을 정지시키고 actor를 종료한다.
    ///
    /// child process의 수명은 daemon의 수명을 넘지 않는다. (Invariant 10 / PRD §71)
    ///
    /// **process가 실제로 사라질 때까지 기다린다.** 정지 요청만 보내고 반환하면
    /// daemon이 먼저 종료해 stop task가 끝나지 못하고 child가 남는다.
    /// app 수만큼 순차로 기다리면 종료가 그만큼 느려지므로 동시에 진행한다.
    pub async fn shutdown(&self) {
        let apps = self.apps();
        let mut pending = Vec::with_capacity(apps.len());

        for handle in apps {
            pending.push(tokio::spawn(async move { handle.shutdown().await }));
        }

        for task in pending {
            let _ = task.await;
        }
    }

    /// 잠금이 poisoned여도 서빙을 멈추지 않는다.
    ///
    /// panic이 난 경로가 남긴 상태가 깨져 있을 수는 있지만, registry는 handle
    /// 맵이고 그 자체로 불변식을 갖지 않는다. 여기서 panic하면 요청마다 panic한다.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// registry 전체 수준의 검증.
///
/// ```text
/// unique name
/// unique host (alias를 포함한다 — PRD §87)
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

        // **alias를 같은 맵에 넣는다.** 다른 app의 host와 겹치는 것도, 다른
        // app의 alias와 겹치는 것도 같은 검사로 걸린다. (PRD §87)
        //
        // 조용히 덮으면 manifest 둘을 둔 사람이 한쪽이 사라진 것을 모른다.
        for alias in &spec.server.aliases {
            let alias = normalize_host(alias);
            if let Some(first) = hosts.insert(alias.clone(), spec.name.as_str()) {
                return Err(ValidationError::DuplicateHost {
                    host: alias,
                    first: first.to_owned(),
                    second: spec.name.clone(),
                });
            }
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

pub(crate) fn normalize_host(host: &str) -> String {
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

    /// alias를 가진 spec. (PRD §87)
    fn spec_with_aliases(name: &str, host: &str, port: u16, aliases: &[&str]) -> AppSpec {
        let list = aliases
            .iter()
            .map(|alias| format!("\"{alias}\""))
            .collect::<Vec<_>>()
            .join(", ");
        AppSpec::from_toml(&format!(
            r#"
name = "{name}"

[server]
host = "{host}"
aliases = [{list}]
port = {port}

[runtime]
command = ["true"]
working_dir = "."
"#
        ))
        .expect("manifest parses")
    }

    /// **alias가 없으면 기존 manifest가 그대로 동작한다.** (PRD §87)
    #[test]
    fn a_manifest_without_aliases_still_parses() {
        let parsed = spec("blog", "blog.localhost", 3101);
        assert!(parsed.server.aliases.is_empty());
        assert_eq!(validate(&[parsed], DEFAULT_GATEWAY_PORT), Ok(()));
    }

    /// alias가 다른 app의 host와 겹치면 거부한다. (PRD §87)
    ///
    /// **조용히 덮으면** manifest 둘을 둔 사람이 한쪽이 사라진 것을 모른다.
    #[test]
    fn rejects_an_alias_that_collides_with_another_host() {
        let specs = vec![
            spec_with_aliases("one", "one.localhost", 3101, &["two.localhost"]),
            spec("two", "two.localhost", 3102),
        ];
        assert!(matches!(
            validate(&specs, DEFAULT_GATEWAY_PORT),
            Err(ValidationError::DuplicateHost { .. })
        ));
    }

    /// 두 app이 같은 alias를 주장하는 것도 거부한다. (PRD §87)
    #[test]
    fn rejects_two_apps_claiming_the_same_alias() {
        let specs = vec![
            spec_with_aliases("one", "one.localhost", 3101, &["shared.localhost"]),
            spec_with_aliases("two", "two.localhost", 3102, &["shared.localhost"]),
        ];
        assert!(matches!(
            validate(&specs, DEFAULT_GATEWAY_PORT),
            Err(ValidationError::DuplicateHost { .. })
        ));
    }

    /// 자기 host와 같은 alias, 와일드카드, 빈 값은 거부한다. (PRD §87)
    #[test]
    fn rejects_aliases_that_say_nothing_or_too_much() {
        // 대소문자와 port suffix를 무시하고 비교한다.
        for alias in ["blog.localhost", "BLOG.LocalHost", "blog.localhost:8080"] {
            let specs = vec![spec_with_aliases("blog", "blog.localhost", 3101, &[alias])];
            assert!(
                matches!(
                    validate(&specs, DEFAULT_GATEWAY_PORT),
                    Err(ValidationError::AliasMatchesOwnHost { .. })
                ),
                "`{alias}`는 자기 host와 같다",
            );
        }

        // **와일드카드를 받지 않는다.** §74의 "등록되지 않은 Host는 404"가
        // 뜻을 잃는다 — supervisor의 probe가 app을 깨우게 된다.
        let specs = vec![spec_with_aliases(
            "blog",
            "blog.localhost",
            3101,
            &["*.localhost"],
        )];
        assert!(matches!(
            validate(&specs, DEFAULT_GATEWAY_PORT),
            Err(ValidationError::WildcardAlias { .. })
        ));

        let specs = vec![spec_with_aliases("blog", "blog.localhost", 3101, &["  "])];
        assert!(matches!(
            validate(&specs, DEFAULT_GATEWAY_PORT),
            Err(ValidationError::EmptyAlias { .. })
        ));
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
