# Performance Goals와 Regression

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §45, §73

---

## 45. Performance Goals

초기 성능 목표는 release hard gate와 분리한다.

제품 수준 목표 예시:

| 지표                        |        초기 목표 |
| --------------------------- | ---------------: |
| Nimwake idle daemon RAM     |       100MB 이하 |
| Duplicate cold start        |                0 |
| Orphan process after stress |                0 |
| Mandatory SaaS dependency   |                0 |
| Lightweight API cold start  | 약 2초 이내 목표 |
| Next.js cold start          | 약 5초 이내 목표 |

실제 cold-start latency는 application runtime 특성에 따라 달라진다.
따라서 총합이 아니라 **구간별 breakdown**으로 판단한다.
→ [§42 Cold Start Breakdown](03-load-tests.md)

CI에서는 다음과 같은 correctness metric을 더 중요하게 본다.

```text
duplicate start = 0
deadlock = 0
orphan process = 0
lost waiter = 0
```

---

## 73. Performance Report와 Regression

### 리포트 artifact

release마다 다음 형태의 리포트를 artifact로 남긴다.

```text
Nimwake Performance Report
commit: a82fd91

Warm path
--------------------------------
Direct backend p50      1.8 ms
Nimwake p50             2.3 ms
Proxy overhead          0.5 ms

Direct backend p95      3.1 ms
Nimwake p95             4.0 ms
Proxy overhead          0.9 ms

Nimwake RSS            14.2 MB
CPU                    11.3%

Cold-start storm (1000 requests)
--------------------------------
Spawn count                 1
Pending peak             1000
Failed requests              0

Actor queue p95          0.7 ms
Process spawn            4.8 ms
Readiness              712.4 ms
Cold start total       719.1 ms

Lifecycle stress
--------------------------------
Cycles                   1000
Orphan processes            0
FD before                   9
FD after                    9
RSS before             13.8 MB
RSS after              14.1 MB
```

---

### 이전 release와 비교

리포트는 단독 수치가 아니라 **직전 release 대비 변화**로 읽는다.

```text
                    v0.0.4     current      Δ

proxy p95           1.20ms      0.91ms    -24%
daemon RSS          16.8MB      14.2MB    -15%
actor queue p95     0.45ms      0.70ms    +56%  ⚠
cold start          720ms       719ms       ~
```

이 비교가 있어야 "이번 변경이 실제로 빨라졌는가"를 객관적으로 판단할 수 있다.

---

### Regression 정책

```text
baseline 축적
    ↓
measurement
    ↓
trend
    ↓
regression warning
    ↓
(데이터 충분) threshold 도입
```

- 초기에는 **경고만** 남기고 release를 막지 않는다.
- GitHub-hosted runner의 편차 때문에 absolute RPS/latency는 초기 gate로 부적절하다.
- baseline이 충분히 쌓이면 `proxy overhead +30% 이상` 같은 상대 threshold를 도입한다.

---

### 잠든 발자국 — 전제를 받치는 수치

README가 "작은 기계에 앱 여러 개"를 약속한다. 그 약속의 핵심 수치는 **app 하나당
Core가 얼마를 쓰는가**인데 재어 둔 적이 없었다. 전부 잠든 상태로 쟀다 — 깨어 있는
app의 메모리는 app의 것이고 Core의 비용이 아니다.

```text
app 수      RSS       fd   스레드
     1   5,056 KB     10        9
    10   5,004 KB     10        9     ← 1개보다 작다(할당기)
    50   5,308 KB     10        9
   200   6,312 KB     10        9
```

**app 하나당 기울기 5~6 KB.** fd와 스레드는 app 수와 무관하다. 200개가 전부 잠들어
6.2 MB다.

유휴 비용도 쟀다. 전부 잠든 60초 동안 **자발적 깨어남 0회, CPU 0.000%**다.
eviction(§80)이 꺼져 있으면(기본) ticker를 아예 띄우지 않기 때문이다 — 켜면
시간당 960회 깬다.

### 총 RSS를 app 수로 나누면 안 된다

처음 리포트에 "app당 56 KB"로 적었다. 총 RSS ÷ app 수여서 **5 MB쯤의 고정 비용이
섞인 값**이다 — 실제 한계 비용의 열 배 가까이 크다. 그 차이가 "이 기계에 app
100개를 올릴 수 있나"의 답을 바꾼다.

두 점(1개와 N개)을 재서 기울기를 낸다. 100개에서 세 번 재어 5.48~5.59 MB
(흔들림 2.0%)였고, 문턱은 5%다.

### 잡음을 재 두었다 — 그리고 절대값으로는 문턱을 걸 수 없다

`v0.1.0`부터 `v0.1.4`까지 다섯 release의 baseline을 비교했다. **그 구간에
요청 경로의 런타임 변경이 거의 없다**(control socket과 CLI는 요청 경로를 지나지
않고, §84의 거절은 WebSocket에만 걸린다). 그래서 차이는 거의 전부 runner 편차다.

```text
지표                              최소        최대   폭(5개)   폭(4개)
concurrency_knee_p95_ms          4.700    12.196     159%      86%
warm_p95_ms                      0.835     1.811     117%      57%
proxy_overhead_p95_ms            0.702     1.544     120%      51%
proxy_overhead_p50_ms            0.323     0.709     119%      58%
warm_p50_ms                      0.503     1.076     114%      57%
request_proxy_connect_ms         0.021     0.045     114%      57%
cold_start_queue_ms              0.025     0.053     112%      36%
concurrency_knee                     50       100     100%     100%
cold_start_readiness_ms          0.058     0.079      36%      29%
cold_start_spawn_ms              0.134     0.179      34%      28%
cold_start_total_ms              0.891     1.101      24%      24%
sustained_cpu_pct_of_wall        163.5     187.6      15%      15%
daemon_rss_mb                    5.766     6.039       5%       2%
```

### 표본을 하나 더하니 폭이 두 배가 되었다

네 개로 쟀을 때 51~58%였던 것들이 다섯 개에서 114~120%다. **`(최대−최소)`는
표본이 늘면 커질 수밖에 없는 추정량이다** — 줄어드는 일이 없다. 그러니 "표본을
모아 문턱을 정확히 한다"는 방향이 성립하지 않는다.

`v0.1.4`의 baseline이 특히 느린 runner에서 돌았고, **그 run에서 절대 지표 셋이
문턱을 넘었다**(`proxy overhead p95` +55% · `actor queue` +56% · `p95 at the
knee` +116%). 셋 다 오경보였다.

### 대조군이 그것을 증명한다

`warm_direct_*`는 **Core를 지나지 않는다** — 테스트 클라이언트가 backend에 직접
보낸 것이다. 그것이 같은 방향으로 같은 크기로 움직였다.

```text
                       v0.1.0   v0.1.1   v0.1.2   v0.1.3   v0.1.4
warm_direct_p50_ms      0.212    0.243    0.159    0.253    0.351
warm_direct_p95_ms      0.549    0.550    0.358    0.539    0.844
```

기계가 느렸던 것이고 Core가 느려진 것이 아니다.

### 그래서 direct로 나눈 비율에 문턱을 건다

```text
비율                     v0.1.0  v0.1.1  v0.1.2  v0.1.3  v0.1.4   폭   절대값 폭
warm_p50 / direct_p50      2.82    3.25    3.16    3.04    3.07   15%     114%
warm_p95 / direct_p95      1.83    2.37    2.33    2.30    2.15   30%     117%
proxy_connect / direct     0.12    0.14    0.13    0.13    0.13   10%     114%
```

**기계가 두 배 느려져도 비율은 그대로다.** 비율이 움직이면 Core가 움직인 것이다.

### 문턱은 폭이 아니라 최대 전이의 1.5배다

처음에 관측한 폭을 그대로 문턱으로 썼더니 과거 전이를 재생했을 때
`v0.1.0 → v0.1.1`이 경계에서 정확히 터졌다. **전이가 폭만큼 움직일 수 있으므로
당연하다.** 표본 네 전이로는 분포를 모르니 여유를 둔다.

```text
지표                        최대 상승 전이    문턱
warm p50 / direct                  15%       25%
warm p95 / direct                  30%       45%
proxy connect / direct             10%       20%
daemon RSS                          1%        5%
sustained CPU                      12%       20%
cold start                         24%       40%
cold start spawn                   22%       35%
cold start readiness               19%       30%
```

### 비교할 수 없는 것은 그렇게 적는다

`warm_p50_ms` 같은 절대 지표는 표에 남기지만 **문턱을 적지 않는다**(`비교 불가`).
전에는 비교 불가를 큰 숫자(100% 이상)로 적었는데, 그러면 표에 문턱이 있는 것처럼
보이면서 실제로는 아무것도 경보하지 않는다 — **있는 척하는 guard는 없는 것보다
나쁘다.**

```text
문턱이 있는 것   비율 셋 + RSS · CPU · cold start 셋          8개
비교 불가        warm p50/p95 · overhead p50/p95 ·
                 knee · knee p95 · actor queue               7개
```

`knee rps / direct rps`는 폭이 14%로 좁은데도 넣지 않았다. **처리량은 내려갈 때가
회귀인데 회귀 판정은 상승만 본다** — 터질 수 없는 방향의 guard다.

### 과거 전이로 확인했다

v0.1.0부터 v0.1.4까지 네 전이를 그대로 재생했다. 그 구간에 요청 경로의 런타임이
바뀌지 않았으므로 **울린 것은 모두 오경보다.**

```text
전 (절대값 문턱 12개, 표본 4개로 측정)   오경보 4개
후 (비율 셋 + 비교 불가 명시)            오경보 0개
```

그리고 반대 방향도 테스트로 고정했다 — **기계는 그대로인데 gateway만 30%
느려지면 비율이 잡는다.** 아무것도 울리지 않는 문턱은 오경보가 0이기도 하므로,
두 방향을 모두 확인해야 뜻이 있다.

### 무릎은 단계 탐색이다

`concurrency_knee`가 50과 100을 왕복하는 것이 특히 그렇다 — 무릎은 단계
탐색이라 한 단계만 움직여도 두 배가 된다.

### 그래서 어떻게 하나

| | |
| --- | --- |
| 지금 | 경고만 남긴다. 리포트의 `직전 대비` 표는 **읽는 사람이 잡음을 아는 상태에서** 봐야 한다 |
| threshold를 켤 때 | 한 번의 측정이 아니라 **여러 release의 추세**로 판단한다. 같은 commit을 여러 번 돌려 잡음의 분포를 먼저 재는 편이 낫다 |
| 지금 쓸 수 있는 것 | `daemon_rss_mb`와 **실패/누수 판정**(spawn 수 · 고아 · 좀비 · FD)이다. 그것들은 숫자가 아니라 불변식이라 잡음이 없다 |

**수치를 인용할 때 범위를 적는다.** 한 release의 값을 정확한 값처럼 쓰면
안 된다 — 실제로 그렇게 했다가 고쳤다(`request_proxy_connect_ms`를 `0.033ms`로
인용했는데 측정 범위가 `0.021~0.033`이다).

---

### Profiling은 gate가 아니라 진단 도구

부하 테스트에서 regression이 발견되면 그때 profiling으로 내려간다.

```text
Load test
   │
   ├── latency regression?
   ├── CPU regression?
   └── memory regression?
           │
           YES
           ▼
       profiling
           │
      ┌────┼────┐
      ▼    ▼    ▼
    CPU   heap syscall
```

예를 들어 proxy overhead가 커졌다면 원인을 다음과 같이 구분한다.

```text
Hyper
 ↓
body forwarding
 ↓
buffer copy
 ↓
allocation
```

인지,

```text
Gateway
 ↓
AppManager channel
 ↓
contention / queue
```

인지를 `actor_queue_latency`와 CPU 프로파일로 판별한다.
→ [§72 Latency Instrumentation](05-instrumentation.md)

profiling은 CI 필수 단계가 아니다. 개발 환경에서 필요할 때 수행한다.

---

← [CI Strategy](../03-delivery/06-ci-strategy.md) · [문서 인덱스](../README.md) · [Core MVP Definition of Done](../03-delivery/08-definition-of-done.md) →
