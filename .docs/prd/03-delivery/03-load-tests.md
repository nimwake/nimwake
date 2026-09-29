# Load Tests

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §42

---

## 42. Load Tests

부하 테스트의 목적은 **통과/실패 판정이 아니라 어디를 최적화해야 하는지 알아내는 것**이다.

Nimwake는 `Gateway → Actor → Process lifecycle → Proxy`라는 구간을 가지므로,
총합 수치만 보면 Nimwake를 고쳐야 할지 application을 고쳐야 할지 구분할 수 없다.

따라서 부하 테스트를 두 경로로 분리한다.

```text
Warm Path   이미 RUNNING인 앱 → Nimwake 자체의 proxy overhead
Cold Path   SLEEPING 상태 → 구간별 cold start breakdown
```

---

### Warm Proxy Load

application이 이미 RUNNING 상태일 때 측정한다.

동일 runner에서 direct 경로와 비교한다.

```text
Client
  │
  ├──────────────▶ Backend
  │                 direct_latency
  │
  └──▶ Nimwake ──▶ Backend
       nimwake_latency

proxy_overhead
  = nimwake_latency - direct_latency
```

측정:

- requests/sec
- error rate
- p50 / p95 / p99 latency
- daemon CPU
- daemon RSS
- connection reuse 비율
- open file descriptors

절대 수치보다 **상대 proxy overhead**를 우선 분석한다.

---

### Cold-Start Storm

application이 SLEEPING인 상태에서 동시에 많은 request를 보낸다.

```text
SLEEPING
   ↓
1000 requests
   ↓
ONE startup
   ↓
ONE process
   ↓
READY
   ↓
requests released
```

correctness 검증:

```text
spawn_count == 1

application_process_count == 1

duplicate_process == 0

deadlock == 0

failed_waiters == 0
```

Nimwake에서는 일반적인 최대 RPS 벤치마크보다 이 테스트가 더 중요하다.

---

### Cold Start Breakdown

cold start는 **단일 숫자로 기록하지 않는다.**

다음 구간을 각각 측정한다.

```text
request_received
       │
       ▼
actor_acquire
       │
       ▼
startup_queue
       │
       ▼
process_spawn
       │
       ▼
readiness
       │
       ▼
proxy_connect
       │
       ▼
first_byte
```

`cold start = 900ms`가 아니라 다음처럼 보여야 한다.

```text
actor queue       3ms
spawn             8ms
app readiness   821ms  ← bottleneck
proxy connect      2ms
other             66ms
```

이 형태여야 **Nimwake를 최적화할지, application startup을 최적화할지** 판단할 수 있다.

구간 측정에 필요한 timestamp는 Core가 제공한다.
→ [§72 Latency Instrumentation](05-instrumentation.md)

---

### Concurrency Sweep

고정된 동시성 하나만 측정하지 않는다.

```text
1
10
50
100
250
500
1000
```

로 단계적으로 올리며 knee point를 찾는다.

```text
Latency
  │
  │                       /
  │                    __/
  │                 __/
  │________________/
  └────────────────────── Concurrency
                 ↑
              knee point
```

기록 형태:

```text
Concurrency    p95      CPU     pending

1              2ms       1%       0
10             2ms       4%       0
50             3ms      12%       1
100            4ms      24%       3
250            9ms      55%      12
500           31ms      91%     130  ← saturation
1000         180ms      99%     256
```

해석 기준:

| 관찰                                        | 추정 원인             |
| ------------------------------------------- | --------------------- |
| CPU가 함께 포화                             | 처리량 한계           |
| CPU는 여유 있는데 `actor_queue_latency` 급증 | actor / mailbox 설계  |
| `pending`이 상한에 붙고 `503` 증가          | backpressure 정상 동작 |

---

← [Testing Strategy](../03-delivery/02-testing-strategy.md) · [문서 인덱스](../README.md) · [Lifecycle Stress Test](../03-delivery/04-stress-tests.md) →
