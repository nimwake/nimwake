//! 구간별 latency 계측.
//!
//! 요구사항: PRD §72 Latency Instrumentation.
//!
//! 구간 측정은 부하 테스트 도구가 아니라 **Core가 제공한다.**
//! 밖에서는 총 latency밖에 볼 수 없어 Gateway / Actor / Runtime 중 어디가 느린지 구분할 수 없다.
//!
//! `cold start = 900ms`가 아니라 이렇게 보여야 한다.
//!
//! ```text
//! actor queue       3ms
//! spawn             8ms
//! app readiness   821ms  ← bottleneck
//! proxy connect     2ms
//! ```
//!
//! 이 형태여야 Nimwake를 고칠지 application을 고칠지 판단할 수 있다. (PRD §42)
//!
//! 매 요청마다 metrics framework를 붙이지 않는다. `tracing` event로 흘리고,
//! 집계는 benchmark mode에서만 한다.

use std::time::Duration;

use tokio::time::Instant;

/// 요청 하나가 지나온 구간. (PRD §72)
///
/// 각 시각은 해당 지점을 **지난 뒤** 기록한다. 아직 지나지 않은 구간은 `None`이다.
/// 실패한 요청도 어디까지 갔는지 남기기 위해서다.
#[derive(Debug, Clone, Copy)]
pub struct RequestTrace {
    received_at: Instant,
    acquire_started_at: Option<Instant>,
    acquire_completed_at: Option<Instant>,
    proxy_started_at: Option<Instant>,
    connected_at: Option<Instant>,
    response_started_at: Option<Instant>,
}

impl RequestTrace {
    /// gateway가 요청 header를 다 받은 시점.
    #[must_use]
    pub fn received() -> Self {
        Self {
            received_at: Instant::now(),
            acquire_started_at: None,
            acquire_completed_at: None,
            proxy_started_at: None,
            connected_at: None,
            response_started_at: None,
        }
    }

    pub fn acquire_started(&mut self) {
        self.acquire_started_at = Some(Instant::now());
    }

    pub fn acquire_completed(&mut self) {
        self.acquire_completed_at = Some(Instant::now());
    }

    pub fn proxy_started(&mut self) {
        self.proxy_started_at = Some(Instant::now());
    }

    /// backend와 TCP 연결이 수립된 시점.
    pub fn backend_connected(&mut self) {
        self.connected_at = Some(Instant::now());
    }

    /// 이미 찍어 둔 시각으로 연결 수립을 기록한다.
    ///
    /// proxy는 timeout 블록 안에서 연결하므로 그 안에서 찍은 값을 받아야 한다.
    /// 블록이 trace를 붙잡으면 await를 넘는 가변 대여가 된다.
    pub fn backend_connected_at(&mut self, at: Instant) {
        self.connected_at = Some(at);
    }

    pub fn response_started(&mut self) {
        self.response_started_at = Some(Instant::now());
    }

    /// `acquire`가 RUNNING을 확보하기까지 걸린 시간.
    ///
    /// cold start면 기동 전체가, warm이면 actor mailbox 대기만 포함된다.
    #[must_use]
    pub fn acquire_latency(&self) -> Option<Duration> {
        span(self.acquire_started_at, self.acquire_completed_at)
    }

    /// backend와 연결이 수립되기까지 걸린 시간. (PRD §72)
    ///
    /// 이 구간은 **연결 수립까지만**이다. backend가 응답을 만드는 시간은
    /// [`Self::backend_latency`]로 따로 본다.
    ///
    /// 한동안 이 함수가 `proxy_started → response_started`를 돌려주었다.
    /// 그것은 연결 수립이 아니라 backend 왕복 전체이고, 둘을 합쳐 두면
    /// "연결이 느린가 / application이 느린가"를 구분할 수 없다. 저사양 환경에서
    /// 어느 구간이 먼저 무너지는지를 보려는 것이 §50의 목적이므로 나눈다.
    #[must_use]
    pub fn proxy_connect_latency(&self) -> Option<Duration> {
        span(self.proxy_started_at, self.connected_at)
    }

    /// 연결 수립부터 응답 header가 돌아오기까지. backend가 일한 시간이다.
    #[must_use]
    pub fn backend_latency(&self) -> Option<Duration> {
        span(self.connected_at, self.response_started_at)
    }

    /// 요청 수신부터 응답 header가 나가기까지. (`time_to_first_byte`)
    #[must_use]
    pub fn time_to_first_byte(&self) -> Option<Duration> {
        span(Some(self.received_at), self.response_started_at)
    }

    /// gateway가 acquire에 들어가기까지의 자체 처리 시간.
    #[must_use]
    pub fn routing_latency(&self) -> Option<Duration> {
        span(Some(self.received_at), self.acquire_started_at)
    }

    /// 요청 하나가 끝난 시점에 구간을 한 줄로 흘린다.
    ///
    /// 집계는 하지 않는다. benchmark mode가 이 event를 모아 히스토그램을 만든다.
    ///
    /// **`debug`인 이유가 있다.** 이 event는 요청마다 나간다.
    /// `info`로 두면 운영 중인 daemon이 요청 하나당 로그 한 줄을 남긴다.
    ///
    /// ```text
    /// 켜져 있을 때   ~2.5 µs/req   (subscriber가 event마다 mutex를 잡는다)
    /// 꺼져 있을 때    ~0.3 ns/req   (callsite에서 걸러진다)
    /// ```
    ///
    /// 시각을 찍는 비용(`Instant::now()` 5회, ~0.1 µs)은 어느 쪽이든 남지만 무시할 수 있다.
    /// 기동 구간([`StartupTrace`])은 드물게 나가고 운영자가 보고 싶어 하므로 `info`로 둔다.
    pub fn emit(&self, app: &str, status: u16) {
        tracing::debug!(
            target: "nimwake::request",
            app,
            status,
            routing_us = micros(self.routing_latency()),
            acquire_us = micros(self.acquire_latency()),
            proxy_connect_us = micros(self.proxy_connect_latency()),
            backend_us = micros(self.backend_latency()),
            ttfb_us = micros(self.time_to_first_byte()),
            "request completed",
        );
    }
}

/// runtime이 채우는 기동 구간.
///
/// `requested_at`은 runtime이 알 수 없다. 기동을 요청한 actor가 채운다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartTimings {
    pub spawn_started_at: Instant,
    pub spawned_at: Instant,
    pub ready_at: Instant,
}

impl StartTimings {
    /// 지금 이 순간 모든 구간이 끝난 것으로 본다. 계측이 없는 runtime을 위한 값이다.
    #[must_use]
    pub fn instant() -> Self {
        let now = Instant::now();
        Self {
            spawn_started_at: now,
            spawned_at: now,
            ready_at: now,
        }
    }
}

/// application 기동 하나가 지나온 구간. (PRD §72)
#[derive(Debug, Clone, Copy)]
pub struct StartupTrace {
    requested_at: Instant,
    timings: StartTimings,
}

impl StartupTrace {
    /// `requested_at`은 actor가 기동을 결정한 시각이다.
    #[must_use]
    pub const fn new(requested_at: Instant, timings: StartTimings) -> Self {
        Self {
            requested_at,
            timings,
        }
    }

    /// 기동 결정부터 spawn 호출까지. 여기가 크면 actor나 스케줄러 쪽 문제다.
    #[must_use]
    pub fn queue_latency(&self) -> Duration {
        self.timings
            .spawn_started_at
            .saturating_duration_since(self.requested_at)
    }

    /// `Command::spawn` 자체에 걸린 시간.
    #[must_use]
    pub fn spawn_latency(&self) -> Duration {
        self.timings
            .spawned_at
            .saturating_duration_since(self.timings.spawn_started_at)
    }

    /// spawn 이후 application이 port를 열기까지. 대개 여기가 지배적이다.
    #[must_use]
    pub fn readiness_latency(&self) -> Duration {
        self.timings
            .ready_at
            .saturating_duration_since(self.timings.spawned_at)
    }

    /// 기동 전체.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.timings
            .ready_at
            .saturating_duration_since(self.requested_at)
    }

    pub fn emit(&self, app: &str) {
        tracing::info!(
            target: "nimwake::startup",
            app,
            queue_us = micros_of(self.queue_latency()),
            spawn_us = micros_of(self.spawn_latency()),
            readiness_us = micros_of(self.readiness_latency()),
            total_us = micros_of(self.total()),
            "application started",
        );
    }
}

fn span(from: Option<Instant>, to: Option<Instant>) -> Option<Duration> {
    Some(to?.saturating_duration_since(from?))
}

/// 마이크로초. `u128`로 내보내면 `tracing`이 JSON에 **문자열**로 적는다.
/// 소비하는 쪽이 숫자로 읽을 수 있어야 하므로 `i64`로 맞춘다.
/// 마이크로초 단위의 `i64`는 29만 년을 담는다.
fn micros(span: Option<Duration>) -> i64 {
    // 아직 지나지 않은 구간은 -1로 남긴다. 0과 구분되어야 한다.
    span.map_or(-1, |span| {
        i64::try_from(span.as_micros()).unwrap_or(i64::MAX)
    })
}

fn micros_of(span: Duration) -> u64 {
    u64::try_from(span.as_micros()).unwrap_or(u64::MAX)
}
