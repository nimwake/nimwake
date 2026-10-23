# Latency Instrumentation

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §72

---

## 72. Latency Instrumentation

구간별 계측은 부하 테스트 도구가 아니라 **Core가 제공해야 한다.**

외부에서는 총 latency밖에 볼 수 없어 Gateway / Actor / Runtime 중 어디가 느린지 구분할 수 없기 때문이다.

도입 시점: `v0.0.4`

---

### Trace 구조

요청 경로:

```rust
struct RequestTrace {
    received_at: Instant,
    acquire_started_at: Instant,
    acquire_completed_at: Instant,
    proxy_started_at: Instant,
    response_started_at: Instant,
}
```

기동 경로:

```rust
struct StartupTrace {
    requested_at: Instant,
    spawn_started_at: Instant,
    spawned_at: Instant,
    ready_at: Instant,
}
```

---

### 파생 지표

```text
actor_queue_latency      acquire_started   → acquire_completed 중 대기 구간
startup_latency          requested         → ready
spawn_latency            spawn_started     → spawned
readiness_latency        spawned           → ready

proxy_connect_latency    proxy_started     → 연결 수립
time_to_first_byte       received          → response_started
request_total_latency    received          → 응답 완료
```

### 단위는 나노초다 — 마이크로초로는 routing을 재지 못했다

trace의 각 구간은 `<이름>_ns` 필드로 나간다.

```text
{"routing_ns":1078,"acquire_ns":1127283,"proxy_connect_ns":17517, …}
```

처음에는 마이크로초(`_us`)였고 `as_micros()`가 버리므로 **1 µs 아래인 구간은
`0`으로 나갔다.** routing은 host header를 보고 app을 찾는 것뿐이어서 개발
기계에서 2~5 µs, CI에서는 그 아래였다 — release baseline **다섯 개에서
`request_routing_ms`가 모두 정확히 0이었다.** 위의 파생 지표 목록에 올려 두고
아무것도 재지 않고 있었다.

나노초의 `i64`는 292년을 담는다. `u128`로 내보내면 `tracing`이 JSON에 **문자열**로
적으므로 `i64`로 맞춘다 — 소비하는 쪽이 숫자로 읽어야 한다.

**아직 지나지 않은 구간은 `-1`이다.** `0`과 구분되어야 한다. 나노초로 올린 뒤에는
`0`이 "너무 짧아서 못 쟀다"가 아니라 실제로 거의 0인 경우뿐이다.

내보내는 쪽(`nimwake-core/src/trace.rs`)과 읽는 쪽(`nimwake-bench/src/daemon.rs`)은
**짝인데 컴파일러가 묶어 주지 않는다** — 한쪽은 `tracing` 매크로의 식별자이고
다른 쪽은 문자열이다. 이름이 어긋나면 bench가 멈추고 그렇게 말한다.

---

게이지:

```text
active_requests
pending_requests
running_apps

daemon_cpu
daemon_rss
open_fds
```

---

### 구현 수준

MVP에서 **매 요청마다 무거운 metrics framework를 붙이지 않는다.**

```text
tracing
  +
benchmark mode
```

정도면 충분하다.

| 모드            | 동작                                                      |
| --------------- | --------------------------------------------------------- |
| 기본            | `tracing` span / event, 집계 없음                          |
| benchmark mode  | trace를 수집해 구간별 히스토그램과 리포트를 생성            |

Prometheus exporter, 상시 히스토그램 집계 등은 Post-MVP 주제다.
→ [§60 Logging & Observability](../05-beyond-mvp/02-platform-features.md)

---

### 사용처

같은 계측을 세 곳에서 재사용한다.

```text
CI 부하 테스트      구간별 baseline / regression
로컬 진단           profiling 진입 전 1차 분류
Android 실기기      CI Linux와 동일 workload 비교
```

Android 검증에서 CI Linux와 **같은 metric**을 쓰는 것이 중요하다.
저사양 환경에서 어떤 구간이 먼저 무너지는지가 Nimwake의 핵심 관심사이기 때문이다.
→ [§50 Android Validation](../04-platform/02-android.md)

---

← [Lifecycle Stress Test](../03-delivery/04-stress-tests.md) · [문서 인덱스](../README.md) · [CI Strategy](../03-delivery/06-ci-strategy.md) →
