//! 메모리가 부족할 때 내릴 application을 고른다.
//!
//! 요구사항: PRD §80 Idle Eviction.
//!
//! **순수 함수다.** 시계도, 메모리 판독도, 상태 변경도 없다. 입력은 지금 상태의
//! 목록이고 출력은 "몇 번째를 내릴지"뿐이다.
//!
//! 처음에는 이 판단을 daemon의 loop 안에 두었다. 그러면 테스트가 실제 메모리
//! 압박을 만들어야 하는데, `echo-server`는 2MB라 개발 기계에서 임계값을 넘길 수
//! 없다. 임계값을 도달 불가능하게 높이면 §78이 모든 기동을 막아 **깨어 있는 app이
//! 없어** 고를 것이 없다. 규칙을 검증할 길이 없었다.
//!
//! §54의 우선순위 다섯 단계 중 **두 개만** 쓴다. `priority`와 `always_on`은
//! 여전히 Post-MVP다.

use crate::app::{AppStateName, AppStatus};

/// 내릴 application의 번째. 없으면 `None`.
///
/// ```text
/// 1. state == RUNNING
/// 2. active_requests == 0
/// 3. 그중 idle_for가 가장 큰 것
/// ```
///
/// **활성 요청이 있는 application은 절대 고르지 않는다.** 그것을 내리면 lease를
/// 쥔 요청이 깨지고, §78이 지키려던 것을 스스로 깨뜨린다.
///
/// 동점이면 먼저 나온 것을 고른다. 목록의 순서는 호출자가 정하고, 동점은 같은
/// 시각에 유휴가 된 경우뿐이라 어느 쪽을 골라도 같다.
#[must_use]
pub fn victim(statuses: &[&AppStatus]) -> Option<usize> {
    statuses
        .iter()
        .enumerate()
        .filter(|(_, status)| status.state == AppStateName::Running && status.active_requests == 0)
        // `idle_for`가 없는 것은 `RUNNING`이 아니다. 위 filter가 이미 걸렀지만
        // 값이 없으면 비교할 수 없으므로 건너뛴다.
        .filter_map(|(index, status)| status.idle_for.map(|idle| (index, idle)))
        .max_by_key(|(_, idle)| *idle)
        .map(|(index, _)| index)
}

/// 한 걸음: 메모리를 보고, 부족하면 하나 내린다. (PRD §80)
///
/// 내린 application의 이름을 돌려준다. 아무것도 내리지 않았으면 `None`이다.
///
/// **loop가 아니라 한 걸음인 이유**: loop를 core에 두면 테스트가 시간을 돌려야
/// 하고, daemon에 두면 `FakeRuntime`으로 검증할 수 없다. 한 걸음이면 둘 다 된다 —
/// daemon은 ticker로 부르고, 테스트는 직접 부른다.
///
/// 한 걸음에 **하나만** 내린다. 정지는 메모리를 즉시 돌려주지 않으므로 여럿을
/// 내리면 첫 정지의 효과를 보기 전에 과하게 내린다.
///
/// # Errors
///
/// 내릴 대상을 찾았으나 정지 요청이 거부되면 그 오류를 돌려준다. actor가 이미
/// 내려간 경우가 그것이다.
pub async fn sweep(
    registry: &crate::AppRegistry,
    runtime: &std::sync::Arc<dyn crate::Runtime>,
    limits: crate::Limits,
) -> Result<Option<String>, crate::error::AcquireError> {
    use crate::app::StopReason;

    if !limits.checks_memory() {
        return Ok(None);
    }

    let Some(available_mb) = runtime.available_memory_mb() else {
        return Ok(None);
    };
    if available_mb >= limits.min_free_memory_mb {
        return Ok(None);
    }

    // **handle과 status를 한 Vec에 둔다.**
    //
    // 전에는 둘을 나란한 Vec 둘에 담고 한쪽의 번째로 다른 쪽을 꺼냈다. 길이가
    // 같다는 보장이 코드 모양에 없어서, 나중에 한쪽만 걸러도 조용히 어긋난다.
    let handles = registry.apps();
    let mut apps = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(status) = handle.status().await {
            apps.push((handle, status));
        }
    }

    let statuses: Vec<&AppStatus> = apps.iter().map(|(_, status)| status).collect();

    let Some(index) = victim(&statuses) else {
        // 전부 바쁘면 내릴 것이 없다. 그때는 §78의 거절이 맞는 답이다.
        tracing::warn!(
            available_mb,
            required_mb = limits.min_free_memory_mb,
            "memory is low but no application is idle; nothing to evict",
        );
        return Ok(None);
    };

    let (victim, status) = &apps[index];
    let name = status.name.clone();
    tracing::info!(
        app = name,
        idle_ms = status.idle_for.unwrap_or_default().as_millis(),
        available_mb,
        required_mb = limits.min_free_memory_mb,
        "evicting the longest-idle application to free memory",
    );

    victim.stop(StopReason::MemoryPressure).await?;
    Ok(Some(name))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn status(name: &str, state: AppStateName, active: usize, idle_secs: Option<u64>) -> AppStatus {
        AppStatus {
            name: name.to_owned(),
            state,
            active_requests: active,
            pending_waiters: 0,
            process_id: None,
            os_pid: None,
            idle_for: idle_secs.map(Duration::from_secs),
            last_error: None,
            since: None,
        }
    }

    /// 가장 오래 유휴인 것을 고른다. (PRD §80)
    #[test]
    fn it_picks_the_longest_idle() {
        let statuses = [
            status("fresh", AppStateName::Running, 0, Some(10)),
            status("oldest", AppStateName::Running, 0, Some(300)),
            status("middle", AppStateName::Running, 0, Some(120)),
        ];
        assert_eq!(victim(&statuses.iter().collect::<Vec<_>>()), Some(1));
    }

    /// **활성 요청이 있으면 절대 고르지 않는다.** (PRD §80)
    ///
    /// 이것이 §80의 가장 중요한 규칙이다. 내리면 lease를 쥔 요청이 깨진다.
    /// 더 오래 유휴여도 바쁜 쪽을 고르면 안 된다.
    #[test]
    fn it_never_picks_a_serving_app() {
        let statuses = [
            // 훨씬 오래 유휴지만 요청을 처리하고 있다.
            status("busy", AppStateName::Running, 3, Some(9999)),
            status("idle", AppStateName::Running, 0, Some(5)),
        ];
        assert_eq!(
            victim(&statuses.iter().collect::<Vec<_>>()),
            Some(1),
            "바쁜 쪽을 고르면 안 된다"
        );
    }

    /// 전부 바쁘면 고를 것이 없다. 그때는 §78의 거절이 맞는 답이다.
    #[test]
    fn everything_busy_means_no_victim() {
        let statuses = [
            status("a", AppStateName::Running, 1, Some(100)),
            status("b", AppStateName::Running, 2, Some(200)),
        ];
        assert_eq!(victim(&statuses.iter().collect::<Vec<_>>()), None);
    }

    /// `RUNNING`이 아닌 것은 고르지 않는다. 이미 메모리를 쓰지 않거나 정리 중이다.
    #[test]
    fn only_running_apps_are_candidates() {
        for state in [
            AppStateName::Sleeping,
            AppStateName::Starting,
            AppStateName::Stopping,
            AppStateName::Failed,
        ] {
            let statuses = [status("x", state, 0, Some(999))];
            assert_eq!(
                victim(&statuses.iter().collect::<Vec<_>>()),
                None,
                "{state}은 후보가 아니다"
            );
        }
    }

    /// 목록이 비어 있으면 `None`.
    #[test]
    fn an_empty_list_has_no_victim() {
        assert_eq!(victim(&[]), None);
    }

    /// `idle_for`가 없으면 비교할 수 없으므로 건너뛴다.
    ///
    /// `RUNNING`이면 항상 있지만, 둘이 어긋나도 panic하지 않아야 한다.
    #[test]
    fn a_missing_idle_time_is_skipped() {
        let statuses = [
            status("no-idle", AppStateName::Running, 0, None),
            status("has-idle", AppStateName::Running, 0, Some(1)),
        ];
        assert_eq!(victim(&statuses.iter().collect::<Vec<_>>()), Some(1));
    }

    /// 후보가 하나뿐이면 그것을 고른다.
    #[test]
    fn a_single_candidate_is_chosen() {
        let statuses = [
            status("sleeping", AppStateName::Sleeping, 0, None),
            status("only", AppStateName::Running, 0, Some(1)),
            status("busy", AppStateName::Running, 1, Some(500)),
        ];
        assert_eq!(victim(&statuses.iter().collect::<Vec<_>>()), Some(1));
    }
}
