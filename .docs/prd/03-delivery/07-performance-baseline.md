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
