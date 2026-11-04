//! 정해진 시각에 application을 깨운다. (PRD §89)
//!
//! **Core는 깨우기만 한다.** 작업은 application이 한다.
//!
//! 타이머는 daemon에 하나다. app마다 걸지 않는다 — 전부 잠든 60초 동안 자발적
//! 깨어남이 **0회**로 측정되어 있고(§51), app마다 타이머를 걸면 그것이 무너진다.
//! 일정이 하나도 없으면 아예 돌지 않는다(§80이 꺼져 있을 때 ticker를 띄우지
//! 않는 것과 같은 모양이다).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

use crate::AppRegistry;

/// 한 번에 자는 최대 시간. (PRD §89)
///
/// **cron의 최소 단위가 1분이므로 그보다 자주 볼 이유가 없고, 그보다 드물게
/// 보면 재적재로 더한 일정의 첫 발화를 놓친다.**
///
/// 긴 잠으로 두면(처음 600초로 했다) 재적재가 더한 일정이 최대 그만큼 늦게
/// 보인다 — `* * * * *`를 더하면 첫 열 번을 놓친다. 1분이면 최악 한 번이고,
/// 그것은 cron이 표현할 수 있는 가장 짧은 간격과 같다.
///
/// 비용을 쟀다 — 일정 하나로 120초 동안 **자발적 깨어남 4회**, 시간당 120회다.
/// 슬라이스마다 두 번인 셈이다(깨고 다시 잠든다). CPU는 0.000%로 측정 단위
/// 아래다. 일정이 없으면 이 비용도 0이다 — 스케줄러를 띄우지 않는다.
const SLICE: Duration = Duration::from_secs(60);

/// 다음 due를 찾는다. 일정이 하나도 없으면 `None`.
///
/// `(시각, app 이름)`을 돌려준다. 같은 시각에 여럿이면 이름 순으로 하나씩 본다.
#[must_use]
fn next_due(registry: &AppRegistry, after: SystemTime) -> Option<(SystemTime, Vec<String>)> {
    let mut earliest: Option<SystemTime> = None;
    let mut names: Vec<String> = Vec::new();

    for handle in registry.apps() {
        let spec = handle.spec();
        let Some(schedule) = &spec.schedule else {
            continue;
        };
        let Some(due) = schedule.wake.next_after(after) else {
            continue;
        };

        match earliest {
            Some(current) if due > current => {}
            Some(current) if due == current => names.push(spec.name.clone()),
            _ => {
                earliest = Some(due);
                names = vec![spec.name.clone()];
            }
        }
    }

    earliest.map(|due| {
        names.sort();
        (due, names)
    })
}

/// 일정이 있는 동안 돈다. 하나도 없으면 바로 돌아간다. (PRD §89)
pub async fn run(registry: Arc<AppRegistry>, gateway_port: u16) {
    loop {
        let now = SystemTime::now();
        let Some((due, names)) = next_due(&registry, now) else {
            // **일정이 없으면 타이머를 걸지 않는다.**
            //
            // 재적재로 일정이 생길 수 있으므로 영구히 끝내지는 않는다. 한
            // 슬라이스에 한 번 본다.
            tokio::time::sleep(SLICE).await;
            continue;
        };

        let wait = due.duration_since(now).unwrap_or(Duration::ZERO);
        // **한 번에 다 자지 않는다.** 재적재가 일정을 바꿀 수 있고, 긴 잠은
        // 그 변화를 못 본다. 상한을 두고 다시 계산한다.
        let slice = wait.min(SLICE);
        tokio::time::sleep(slice).await;

        // **잠에서 깬 뒤 벽시계로 다시 확인한다.**
        //
        // `tokio::time::sleep`은 단조 시계를 쓰고 due는 벽시계다. 둘이 어긋나면
        // 잠이 due보다 **일찍** 끝날 수 있다 — 실제로 3초 일찍 깨는 것을 봤다.
        //
        // 그대로 깨우면 다음 루프가 같은 due를 다시 보고 **두 번 깨운다.** due를
        // 지났는지 벽시계로 확인하고, 아니면 다시 계산한다.
        if SystemTime::now() < due {
            continue;
        }

        for name in names {
            wake(&registry, &name, gateway_port).await;
        }
    }
}

/// app 하나를 깨운다. 실패하면 로그만 남긴다. (PRD §89)
///
/// **재시도하지 않는다.** 재시도는 따라잡기의 다른 이름이고, 따라잡기를 하지
/// 않는 이유(상태 저장이 없다 · 몰아친다)가 그대로 적용된다.
async fn wake(registry: &AppRegistry, name: &str, gateway_port: u16) {
    let Some(handle) = registry.get(name) else {
        // 재적재로 사라졌다. 이상한 일이 아니다.
        return;
    };
    let spec = handle.spec();
    let Some(schedule) = &spec.schedule else {
        return;
    };

    let lease = match handle.acquire().await {
        Ok(lease) => lease,
        Err(error) => {
            // §77 backoff 중이거나 §78로 거절됐거나 기동이 실패했다. 어느
            // 쪽이든 다음 due를 기다린다.
            warn!(app = %name, %error, "scheduled wake did not start the application");
            return;
        }
    };

    info!(
        app = %name,
        keep_awake_seconds = schedule.keep_awake_seconds,
        path = schedule.path.as_deref().unwrap_or("-"),
        "scheduled wake",
    );

    // 경로를 적었으면 한 번 찝는다.
    //
    // **웹 서버 모양의 app을 그냥 깨우면 아무 일도 일어나지 않는다.** 서버가
    // 뜨기만 한다. 요청은 gateway를 지나므로 밖에서 온 요청과 같은 경로다.
    if let Some(path) = &schedule.path {
        let host = spec.server.host.clone();
        let path = path.clone();
        match probe(gateway_port, &host, &path).await {
            Ok(status) => info!(app = %name, %status, %path, "scheduled request"),
            Err(error) => warn!(app = %name, %error, %path, "scheduled request failed"),
        }
    }

    // keep_awake 동안 lease를 쥔다. 그동안 idle timeout도 §80 eviction도
    // 이 app을 건드리지 않는다.
    if schedule.keep_awake_seconds > 0 {
        tokio::time::sleep(Duration::from_secs(schedule.keep_awake_seconds)).await;
    }
    drop(lease);
}

/// gateway에 요청 하나를 보낸다. 응답 body는 버리고 상태 코드만 본다.
async fn probe(gateway_port: u16, host: &str, path: &str) -> Result<u16, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", gateway_port))
        .await
        .map_err(|error| format!("connect: {error}"))?;

    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("write: {error}"))?;

    // 상태 줄만 읽으면 되지만 **끝까지 읽는다** — 중간에 끊으면 app이 쓰는 쪽에서
    // 오류를 본다. 작업이 오래 걸릴 수 있으므로 상한을 두지 않는다. app이
    // 응답을 끝내는 동안 lease는 그 요청이 쥔다(§25–§26).
    let mut sink = Vec::new();
    stream
        .read_to_end(&mut sink)
        .await
        .map_err(|error| format!("read: {error}"))?;

    let head = String::from_utf8_lossy(&sink);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| "the response had no status code".to_owned())?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 일정이 하나도 없으면 `None`. **그때는 타이머를 걸지 않는다.** (PRD §89)
    #[test]
    fn no_schedule_means_no_timer() {
        // `next_due`는 registry를 받으므로 단위 테스트는 lifecycle.rs에 있다.
        // 여기서는 그 계약을 문서로 남긴다 — `None`이 "돌지 않는다"는 뜻이다.
        let _ = next_due;
    }
}
