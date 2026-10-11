//! Crash-loop backoff.
//!
//! 요구사항: PRD §77 Crash-Loop Backoff.
//!
//! backoff가 없으면 계속 실패하는 application은 **요청마다** 기동을 재시도한다.
//! 측정된 상한은 초당 623회 spawn이다.
//!
//! 여기에는 판정만 있다. 시계도, 상태 변경도, side effect도 없다.
//! 전이 함수가 동기이고 런타임에 무관해야 하기 때문이다. (PRD §18)

use std::time::Duration;

use tokio::time::Instant;

/// 창 안에서 이 횟수에 도달하면 backoff에 들어간다. (PRD §77)
pub(crate) const FAILURE_THRESHOLD: usize = 3;

/// 실패를 세는 창. (PRD §77)
///
/// 연속 실패가 아니라 창으로 보는 이유: 하루를 정상으로 돌다가 한 번 죽은
/// application은 crash loop가 아니다.
pub(crate) const FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// 임계값에 도달한 첫 실패의 대기 시간. 이후 2배씩 늘어난다. (PRD §77)
pub(crate) const BASE_DELAY: Duration = Duration::from_secs(1);

/// 대기 시간의 상한. (PRD §77)
///
/// 창(60초)보다 작아야 한다. 크면 backoff에서 빠졌다 들어오기를 반복한다.
pub(crate) const MAX_DELAY: Duration = Duration::from_secs(30);

/// 실패 이력.
///
/// 두 가지를 따로 들고 있다. **같은 값으로 둘 다 할 수 없기 때문이다.**
///
/// ```text
/// recent   창 판정용. "최근 3회가 60초 안에 있는가"
/// streak   지연 크기용. 3회, 4회, 5회를 구분해야 2배씩 늘릴 수 있다
/// ```
///
/// `recent`를 임계값 길이로 묶으면 그 길이로는 3회와 10회를 구분할 수 없다.
/// 처음에 `recent`만 두었다가 지연이 영원히 1초에 머무는 것을 발견했다.
///
/// `recent`의 길이는 [`FAILURE_THRESHOLD`]로 묶인다. 무한히 자라는 수집은
/// 금지되어 있다. (PRD §69)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FailureHistory {
    /// 오래된 것이 앞에 온다. 길이 상한은 [`FAILURE_THRESHOLD`].
    recent: Vec<Instant>,
    /// 창이 비어 있던 시점부터 센 실패 횟수.
    streak: u32,
}

impl FailureHistory {
    /// 실패를 하나 기록한다.
    pub(crate) fn record(&mut self, now: Instant) {
        if self.recent.len() == FAILURE_THRESHOLD {
            self.recent.remove(0);
        }
        self.recent.push(now);

        // 창에 이것뿐이면 새 연속의 시작이다. 앞의 실패들은 너무 오래됐다.
        if self.count_within(now, FAILURE_WINDOW) <= 1 {
            self.streak = 1;
        } else {
            self.streak = self.streak.saturating_add(1);
        }
    }

    /// 창 안의 실패 횟수.
    pub(crate) fn count_within(&self, now: Instant, window: Duration) -> usize {
        self.recent
            .iter()
            .filter(|at| now.saturating_duration_since(**at) <= window)
            .count()
    }

    /// `now`에 실패한 뒤 다음 재시도까지 **얼마나 기다려야 하는가.**
    ///
    /// `None`은 즉시 재시도한다는 뜻이다. 실패 1, 2회가 여기 해당한다.
    /// 일시적 실패까지 기다리게 만들면 다음 요청이 앱을 깨운다는 약속이 깨진다.
    pub(crate) fn delay_after_failure(&self, now: Instant) -> Option<Duration> {
        // 들어가는 판정은 창이 한다. `streak`은 창보다 느슨하다 —
        // 0초, 50초, 100초에 실패하면 streak은 3이지만 창 안에는 2회뿐이다.
        if self.count_within(now, FAILURE_WINDOW) < FAILURE_THRESHOLD {
            return None;
        }

        let threshold = u32::try_from(FAILURE_THRESHOLD).unwrap_or(u32::MAX);
        let steps = self.streak.saturating_sub(threshold).min(16);
        Some(BASE_DELAY.saturating_mul(1u32 << steps).min(MAX_DELAY))
    }
}

/// `FAILED`에서 지금 재시도할 수 있는가. 아니라면 얼마나 남았는가.
///
/// backoff는 상태가 아니다. `FAILED`에 머문다. (PRD §77)
pub(crate) fn remaining_backoff(
    history: &FailureHistory,
    failed_at: Instant,
    now: Instant,
) -> Option<Duration> {
    let delay = history.delay_after_failure(failed_at)?;
    let elapsed = now.saturating_duration_since(failed_at);
    delay.checked_sub(elapsed).filter(|left| !left.is_zero())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 테스트 안에서 시각을 다루려면 timer가 필요하다.
    fn history_at(offsets_secs: &[u64]) -> (FailureHistory, Instant) {
        let base = Instant::now();
        let mut history = FailureHistory::default();
        for offset in offsets_secs {
            history.record(base + Duration::from_secs(*offset));
        }
        let last = base + Duration::from_secs(offsets_secs.last().copied().unwrap_or(0));
        (history, last)
    }

    /// 실패 1, 2회는 즉시 재시도한다. (PRD §77)
    ///
    /// 이것이 깨지면 port가 잠깐 잡혀 있던 것 같은 일시적 실패에도 앱이
    /// 몇 초 동안 깨지 않는다. "다음 요청이 앱을 깨운다"는 약속이 깨진다.
    #[tokio::test(start_paused = true)]
    async fn the_first_two_failures_retry_immediately() {
        let (history, now) = history_at(&[0]);
        assert_eq!(history.delay_after_failure(now), None, "1회");

        let (history, now) = history_at(&[0, 1]);
        assert_eq!(history.delay_after_failure(now), None, "2회");
    }

    /// 임계값에 도달하면 기다린다. 이후 2배씩. (PRD §77)
    #[tokio::test(start_paused = true)]
    async fn the_delay_doubles_past_the_threshold() {
        for (failures, expected_secs) in [
            (vec![0, 1, 2], 1),
            (vec![0, 1, 2, 3], 2),
            (vec![0, 1, 2, 3, 4], 4),
            (vec![0, 1, 2, 3, 4, 5], 8),
            (vec![0, 1, 2, 3, 4, 5, 6], 16),
        ] {
            let count = failures.len();
            let (history, now) = history_at(&failures);
            assert_eq!(
                history.delay_after_failure(now),
                Some(Duration::from_secs(expected_secs)),
                "실패 {count}회",
            );
        }
    }

    /// 상한을 넘지 않는다. 창보다 커지면 backoff에서 빠졌다 들어오기를 반복한다. (PRD §77)
    #[tokio::test(start_paused = true)]
    async fn the_delay_is_capped() {
        let failures: Vec<u64> = (0..40).collect();
        let (history, now) = history_at(&failures);
        assert_eq!(history.delay_after_failure(now), Some(MAX_DELAY));
        assert!(MAX_DELAY < FAILURE_WINDOW, "상한은 창보다 작아야 한다");
    }

    /// 창 밖의 실패는 세지 않는다. (PRD §77)
    ///
    /// 하루를 정상으로 돌다가 한 번 죽은 application은 crash loop가 아니다.
    #[tokio::test(start_paused = true)]
    async fn failures_outside_the_window_do_not_count() {
        // 1000초씩 떨어진 실패 3회. 창(60초)에는 언제나 1회뿐이다.
        let (history, now) = history_at(&[0, 1000, 2000]);
        assert_eq!(history.count_within(now, FAILURE_WINDOW), 1);
        assert_eq!(history.delay_after_failure(now), None);
    }

    /// 창이 판정하고 streak이 크기를 정한다. 둘은 다르다. (PRD §77)
    ///
    /// 0 · 50 · 100초의 실패는 streak으로는 3회지만 창 안에는 2회뿐이다.
    /// 창이 막지 않으면 1분에 한 번 실패하는 — 이미 무해한 — application도
    /// backoff에 들어간다.
    #[tokio::test(start_paused = true)]
    async fn the_window_gates_and_the_streak_scales() {
        let (history, now) = history_at(&[0, 50, 100]);
        assert_eq!(history.streak, 3, "streak은 이어진다");
        assert_eq!(history.count_within(now, FAILURE_WINDOW), 2, "창에는 2회");
        assert_eq!(
            history.delay_after_failure(now),
            None,
            "창이 임계값에 못 미치면 backoff가 아니다",
        );
    }

    /// 이력의 길이는 묶여 있다. 무한히 자라는 수집은 금지되어 있다. (PRD §69)
    #[tokio::test(start_paused = true)]
    async fn the_history_is_bounded() {
        let failures: Vec<u64> = (0..500).collect();
        let (history, _) = history_at(&failures);
        assert_eq!(history.recent.len(), FAILURE_THRESHOLD);
    }

    /// 시간이 지나면 재시도할 수 있다. (PRD §77)
    #[tokio::test(start_paused = true)]
    async fn the_backoff_expires() {
        let base = Instant::now();
        let mut history = FailureHistory::default();
        for offset in [0, 1, 2] {
            history.record(base + Duration::from_secs(offset));
        }
        let failed_at = base + Duration::from_secs(2);

        // 지연은 1초다. 그 안에는 남아 있고, 지나면 없다.
        assert_eq!(
            remaining_backoff(&history, failed_at, failed_at),
            Some(Duration::from_secs(1)),
        );
        assert_eq!(
            remaining_backoff(&history, failed_at, failed_at + Duration::from_millis(400)),
            Some(Duration::from_millis(600)),
        );
        assert_eq!(
            remaining_backoff(&history, failed_at, failed_at + Duration::from_secs(1)),
            None,
            "지연이 지나면 재시도할 수 있다",
        );
        assert_eq!(
            remaining_backoff(&history, failed_at, failed_at + Duration::from_secs(5)),
            None,
        );
    }

    /// 실패 1, 2회에는 남은 시간이 없다. (PRD §77)
    #[tokio::test(start_paused = true)]
    async fn there_is_no_backoff_below_the_threshold() {
        let (history, failed_at) = history_at(&[0, 1]);
        assert_eq!(remaining_backoff(&history, failed_at, failed_at), None);
    }

    /// 최악의 spawn 속도를 계산해 둔다. (PRD §77)
    ///
    /// 측정된 backoff 없는 상한이 초당 623회였다. 상한 지연에서 30초당 1회가 된다.
    #[tokio::test(start_paused = true)]
    async fn the_worst_case_spawn_rate_is_bounded() {
        let failures: Vec<u64> = (0..40).collect();
        let (history, now) = history_at(&failures);
        let delay = history.delay_after_failure(now).expect("backing off");
        assert!(
            delay >= Duration::from_secs(30),
            "상한 지연에서 30초당 1회를 넘지 않아야 한다: {delay:?}",
        );
    }
}
