# Core MVP Version Plan

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §37

---

## 37. Core MVP Version Plan

Core MVP는 다섯 개의 마일스톤을 거쳐 `v0.1.0`에 도달한다.

각 버전은 작업량이 아니라 **아키텍처 마일스톤**을 의미한다.

```text
v0.0.1   Lifecycle Foundation
   ↓
v0.0.2   Process Runtime
   ↓
v0.0.3   HTTP + Scale-to-Zero
   ↓
v0.0.4   E2E + Performance Baseline
   ↓
v0.0.5   Stress + Regression Qualification
   ↓
v0.1.0   Core MVP
```

| 버전     | 아키텍처 마일스톤                 | 핵심 acceptance                                        | 실행 계획                                                                                 |
| -------- | --------------------------------- | ------------------------------------------------------ | ----------------------------------------------------------------------------------------- |
| `v0.0.1` | Lifecycle Foundation              | `FakeRuntime.start_count == 1`                         | [Phase 1 › v0.0.1](../../phases/phase-1/03-milestones/01-v0.0.1-lifecycle-foundation.md)    |
| `v0.0.2` | Process Runtime                   | 실제 process spawn → ready → stop, 소유권 단일화       | [Phase 1 › v0.0.2](../../phases/phase-1/03-milestones/02-v0.0.2-process-runtime.md)         |
| `v0.0.3` | HTTP + Scale-to-Zero              | cold start → 200 → idle → 재기동, overload 시 `503`    | [Phase 1 › v0.0.3](../../phases/phase-1/03-milestones/03-v0.0.3-http-scale-to-zero.md)      |
| `v0.0.4` | E2E + Performance Baseline        | E2E 자동화 + 구간별 latency baseline 확보              | [Phase 1 › v0.0.4](../../phases/phase-1/03-milestones/04-v0.0.4-e2e-performance-baseline.md) |
| `v0.0.5` | Stress + Regression Qualification | storm / stress / leak 통과 + 이전 release 대비 비교    | [Phase 1 › v0.0.5](../../phases/phase-1/03-milestones/05-v0.0.5-stress-regression.md)       |
| `v0.1.0` | Core MVP                          | [Definition of Done](08-definition-of-done.md) 전 항목 | [Phase 1 › v0.1.0](../../phases/phase-1/03-milestones/06-v0.1.0-core-mvp.md)                 |

`v0.1.0`은 다음 의미를 가진다.

> Nimwake의 Scale-to-Zero Core lifecycle이 automated correctness, concurrency, E2E, stress test를 통과한 상태.

단순히 수동 테스트에서 동작하는 것을 의미하지 않는다.

---

### v0.0.4 / v0.0.5의 역할 분리

`v0.0.4`는 **측정 가능하게 만드는 단계**이고, `v0.0.5`는 **측정값을 비교해 판정하는 단계**다.

```text
v0.0.4 — E2E + Performance Baseline

Correctness
├── E2E
├── cold-start concurrency
├── crash recovery
└── lifecycle

Performance
├── direct-backend baseline
├── warm proxy benchmark
├── cold-start breakdown
├── concurrency sweep
├── CPU / RSS / FD
└── benchmark report


v0.0.5 — Stress + Regression Qualification

├── lifecycle stress
├── cold-start storm
├── sustained warm load
├── memory leak detection
├── FD leak detection
├── orphan detection
├── previous-release comparison
└── performance regression report
```

성능 수치는 초기에 release hard gate로 사용하지 않는다.
→ [§45 Performance Goals](07-performance-baseline.md)

---

### 이 문서의 범위

PRD는 각 마일스톤이 **무엇을 달성해야 하는지**만 정의한다.

마일스톤별 구현 항목, 작업 순서, 테스트 fixture 같은 실행 세부사항은
[Phase 1 실행 계획](../../phases/phase-1/README.md)에서 관리한다.

---

← [Repository Architecture와 Technology Stack](../02-architecture/09-repository-and-stack.md) · [문서 인덱스](../README.md) · [Testing Strategy](../03-delivery/02-testing-strategy.md) →

---

## 88. 1.0 재고 조사

**이 절은 아무것도 결정하지 않는다.** §62의 로드맵은 `v0.3.x` 다음을 "Later"로
두고 끝나고, PRD 어디에도 `v1.0`이라는 말이 없다. 그래서 "1.0까지 남은 일"을
세려면 먼저 **1.0이 무엇을 약속하는가**를 정해야 한다.

이 절은 그 결정을 위한 재고 조사다. 지금 무엇이 동결 가능하고, 무엇이 열려
있고, 무엇을 끝까지 안 하기로 했는지를 **사실만** 적는다. 동결 여부는 정하지
않는다 — 결정이 서면 그때 이 절을 고치거나 새 절을 쓴다.

### v0.1.0 이후 실제로 바뀐 것

git diff로 센 것이다. 기억이 아니다.

```text
새 절          §84 WebSocket 501 거절 · §85 control socket · §86 nimwake CLI
               §87 host aliases
manifest       aliases 하나. `#[serde(default)]`이므로 기존 manifest는 그대로 동작한다
CLI 플래그      --control-socket 하나
새 바이너리      nimwake (nimwaked의 두 번째 [[bin]])
HTTP 응답       501 하나 (§84). 그 전에는 Upgrade header를 지우고 평범한 요청으로 넘겼다
trace 단위      _us → _ns (§72). benchmark mode 전용이고 계약으로 약속한 적이 없다
```

**§82(peer allowlist)와 §83(요청 입구 제한)은 v0.1.0에 이미 있었다.** 이 세션에
쓴 것으로 착각하기 쉽다.

### 동결 후보 — manifest

`deny_unknown_fields`이므로 **모르는 키는 기동을 막는다.** 즉 키를 더하는 것은
이전 manifest에 안전하지만, 새 manifest를 옛 daemon에 주면 뜨지 않는다.

```text
[server]     host · aliases · port · allow
[runtime]    command · working_dir · env
[lifecycle]  idle_timeout_seconds · startup_timeout_seconds ·
             shutdown_timeout_seconds · response_header_timeout_seconds ·
             max_pending_requests
```

`[lifecycle]`의 다섯 기본값은 `scripts/check-docs.py`가 §21과 코드를 대조한다.

### 동결 후보 — HTTP 계약

§24가 범위를 적어 두었다. 지금 실제로 돌려주는 것:

```text
200 계열     정상 proxy. 응답 body는 흘려보낸다(모아 두지 않는다 — 테스트로 고정)
400          `Host` header가 없다
403          §82의 대역 밖 peer. app을 깨우지 않는다
404          등록되지 않은 Host. **아무것도 깨우지 않는다** — §74의 probe가 이것에 의존한다
413          §83의 본문 상한 초과. 기본은 꺼짐
501          §84의 Upgrade 요청
502          backend 연결·응답 실패
503          §69의 대기자 상한 초과, §78의 메모리 부족
504          §75의 응답 header 상한 초과
```

**404가 가장 조심할 것이다.** supervisor의 health probe가 "없는 host는 404이고
아무것도 깨우지 않는다"에 걸려 있다. 와일드카드 host를 받지 않는 이유도 그것이다
(§87).

### 동결 후보 — 창구와 종료 코드

control socket은 **이미 버전 훅을 갖고 있다.**

```rust
/// **다른 저장소가 이 창구에 의존한다.** 필드를 없애거나 뜻을 바꾸면 올린다.
/// 모르는 필드를 더하는 것은 올리지 않는다 — 읽는 쪽이 무시하면 된다.
pub const CONTRACT: u32 = 1;
```

종료 코드는 두 바이너리가 같은 뜻을 쓴다.

```text
nimwaked   0 정상 · 78 설정 오류 · 69 bind 실패
nimwake    0 답을 받았다 · 69 창구에 닿지 못했다 · 70 daemon이 오류 · 78 인자가 틀렸다
```

### 열려 있는 것 — §24가 "보장하지 않음"에 둔 넷

각각 **지금 실제로 무엇이 일어나는지**와 **무엇을 결정해야 하는지**를 적는다.

| | 지금 | 1.0에서 결정할 것 |
| --- | --- | --- |
| WebSocket | `501`로 거절. 로그를 남긴다 (§84) | 지원으로 올릴지. 올리면 "연결이 열려 있는 동안 잠들지 않는다"를 어떻게 다룰지 |
| SSE | 응답 body는 흘러간다. **연결이 열려 있는 동안 lease를 쥔다** | 같은 문제. 아래 참고 |
| 큰 streaming upload | `--max-request-body-mb`로 상한을 켤 수 있다. 기본 꺼짐 | 기본값을 둘지. 어떤 기본값이든 누군가의 업로드를 깨뜨린다 |
| long-lived request | 상한이 없다. §75는 **응답 header까지만** 센다 | 상한을 둘지. 두면 긴 다운로드가 끊긴다 |

**SSE와 WebSocket은 scale-to-zero와 정면으로 부딪힌다.** 연결이 열려 있는 동안
lease를 쥐므로 app이 잠들지 않는다 — 연결 하나가 idle timeout을 영구히 무력화한다.
"지원한다"고 적으려면 그 충돌을 다루는 절이 필요하다.

검증 상태도 적어 둔다. `text/event-stream`을 직접 시험하는 테스트는 **없다.**
다만 SSE가 의존하는 기제(응답 body가 끝날 때까지 lease가 살아 있다)는
`a_lease_outlives_the_response_body`가 고정하고 있다.

### 끝까지 안 하기로 한 것 — 1.0의 조건이 아니다

§61이 범위를 그었고 SECURITY.md가 그것을 설계 결정으로 적고 있다.

```text
인증        없다. 요청자를 구분하지 않는다
TLS         없다. 평문 HTTP만 말한다
속도 제한    없다. 요청자를 구분해야 하는데 그럴 수단이 없다
```

**"없으니 1.0이 아니다"가 아니라 "없는 것이 1.0이다".** 노출은 그 일을 하는
것(터널, reverse proxy)에 맡긴다. 1.0 정의에서 이 셋을 조건으로 올리면 §61을
뒤집는 것이고, 그것은 제품의 범위를 바꾸는 결정이다.

§87이 와일드카드를 거부하는 이유, §85가 상태를 바꾸는 명령을 두지 않는 이유도
같은 계열이다 — 하지 않기로 한 것에는 이유가 적혀 있다.

### 숫자로 고정된 것

약속이 아니라 **관측값**이다. 기계가 지키고 있어서 조용히 나빠지지 않는다.

```text
잠든 발자국      app 100개가 전부 잠들어 5.5 MB. app당 기울기 6 KB
                fd 11 · 스레드 9 — app 수와 무관하다
유휴 비용        전부 잠든 60초 동안 자발적 깨어남 0회, CPU 0.000%
                eviction(§80)을 켜면 시간당 960회
문턱이 있는 지표   비율 셋 + RSS · CPU · cold start 셋 + ttfb · acquire
비교 불가        warm p50/p95 · overhead p50/p95 · knee · knee p95 · actor queue
```

기기 수치는 따로다 — cold start 1.6초, 그중 Core의 몫이 ~30 ms, warm 27~52 ms.
배터리 1.8 %/h로 **상시 충전이 전제다.**

### 결정해야 하는 것 — 이 절은 정하지 않는다

```text
1. 1.0이 무엇을 약속하는가       동결할 표면과 그 뜻
2. §24의 넷의 거취              특히 SSE·WebSocket과 scale-to-zero의 충돌
3. Phase 2에서 Core의 몫         CLI는 들어왔고 배포·롤백은 nimwake-android가 한다.
                               남는 것은 "상태 저장"인데 필요한지 미확인
4. 호환성 이야기                 옛 manifest를 새 daemon이, 새 manifest를 옛 daemon이
5. 기기 재측정                   spawn 속도와 /proc 가용성 — 개발 기계 수치는 자릿수만 말한다
```

**2·3은 실제로 한 번 운영해 보면 답이 저절로 나온다.** 지금 정하면 추측으로
정하게 된다. Rule 8이 그것을 막는다.

→ [§24 HTTP Contract](../02-architecture/06-gateway-and-requests.md#24-http-contract) ·
[§61 Security](../05-beyond-mvp/02-platform-features.md#61-security) ·
[§62 Long-Term Roadmap](../05-beyond-mvp/03-long-term-roadmap.md#62-long-term-roadmap) ·
[§64 Rule 8](../06-conventions/01-development-rules.md)
