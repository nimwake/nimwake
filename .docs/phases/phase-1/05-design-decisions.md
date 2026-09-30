# 설계 결정 기록

> **Phase 1 — Core MVP** › 설계 결정
>
> 구현 착수 전 설계 리뷰에서 나온 지적과, 각 항목이 PRD 어디에 반영되었는지의 추적표.

---

이 문서는 **결정의 이력**을 남기기 위한 것이다.
결정의 내용 자체는 PRD가 소유하며, 여기서는 "무엇을 왜 바꿨고 어디에 반영했는가"만 기록한다.

---

## P0 — 구현 시작 전 반드시 반영

| #   | 결정                                                     | 반영 위치                                                                 | 마일스톤 |
| --- | -------------------------------------------------------- | ------------------------------------------------------------------------- | -------- |
| D1  | `ensure_running()` + `RequestStarted` → 원자적 `acquire()` / `RequestLease` | [§26](../../prd/02-architecture/06-gateway-and-requests.md), [Invariant 7](../../prd/02-architecture/02-lifecycle-invariants.md) | `v0.0.1` |
| D2  | startup / shutdown에 `OperationId` 부여                  | [§68](../../prd/02-architecture/02-lifecycle-invariants.md), [§15](../../prd/02-architecture/03-concurrency.md) | `v0.0.1` |
| D3  | waiter를 `AppState`가 직접 소유                          | [§11](../../prd/02-architecture/01-core-architecture.md)                   | `v0.0.1` |
| D4  | `ProcessExited`에 `ProcessId` 포함                       | [§32](../../prd/02-architecture/08-failure-handling.md), [§68](../../prd/02-architecture/02-lifecycle-invariants.md) | `v0.0.2` |
| D5  | `tokio::process::Child` 소유권을 `ProcessController` 단일화 | [§70](../../prd/02-architecture/04-runtime.md)                            | `v0.0.2` |

### D1이 가장 중요한 이유

`ensure_running()`은 "프로세스가 살아 있다"까지만 보장하고,
그 직후 idle shutdown되지 않는다는 보장은 하지 못한다.

```text
ensure_running()
      ↓
   RUNNING
             ← idle timer 발화
   STOPPING
      ↓
RequestStarted   ← 종료 중인 process로 proxy
```

`acquire()`가 RUNNING 확보와 active request 예약을 **하나의 actor transition**으로 처리하면
이 race가 구조적으로 사라진다.

---

## P1 — `v0.0.1`~`v0.0.3` 구간에 반영

| #   | 결정                                        | 반영 위치                                                          | 마일스톤 |
| --- | ------------------------------------------- | ------------------------------------------------------------------ | -------- |
| D6  | AppManager mailbox를 bounded channel로      | [§14](../../prd/02-architecture/03-concurrency.md), [§69](../../prd/02-architecture/06-gateway-and-requests.md) | `v0.0.1` |
| D7  | pending waiter 상한 (`max_pending_requests`) → `503` | [§69](../../prd/02-architecture/06-gateway-and-requests.md)  | `v0.0.3` |
| D8  | daemon–child lifetime 정책 (process group)  | [§71](../../prd/02-architecture/08-failure-handling.md), [Invariant 10](../../prd/02-architecture/02-lifecycle-invariants.md) | `v0.0.2` |
| D9  | manifest validation (host/port 충돌 거부)   | [§21](../../prd/02-architecture/05-manifest-and-readiness.md)       | `v0.0.1` |
| D10 | stdout/stderr는 inherit 또는 drain          | [§19](../../prd/02-architecture/04-runtime.md)                      | `v0.0.2` |
| D11 | 구간별 latency instrumentation              | [§72](../../prd/03-delivery/05-instrumentation.md)                  | `v0.0.4` |

### D6 + D7이 함께 필요한 이유

mailbox만 제한해도 충분하지 않다.
AppManager가 command를 빠르게 소비하면서 waiter를 계속 누적시킬 수 있다.

```text
10,000 cold-start requests

mailbox capacity = 128

AppManager가 계속 consume

        ↓

waiters = 10,000      ← mailbox 제한만으로는 막지 못한다
```

---

## 구현 중 확정 (`v0.0.1`)

문서만으로는 결정되지 않았고, 코드를 쓰면서 확정한 것들. PRD에 반영을 끝냈다.

| #   | 결정                                                        | 이유                                                                                     | 반영 위치                                                   |
| --- | ----------------------------------------------------------- | ---------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| D16 | `active_requests: usize` → `leases: HashSet<LeaseId>`        | 정수 카운터는 같은 lease가 두 번 반납돼도 그대로 감소한다. 중복 반납 무시가 불가능하다.   | [§11](../../prd/02-architecture/01-core-architecture.md)     |
| D17 | `Runtime` trait에 `wait()` 추가                              | `Child`를 AppManager로 넘기지 않고 exit를 감시하려면 runtime 쪽에 대기 지점이 필요하다.   | [§18](../../prd/02-architecture/04-runtime.md)               |
| D18 | mailbox가 가득 차도 lease 반납은 반드시 전달                 | 반납을 버리면 `active_requests`가 0이 되지 못해 app이 영원히 잠들지 않는다.               | [§69](../../prd/02-architecture/06-gateway-and-requests.md)  |
| D19 | `AppCommand::Status` 추가                                    | 상태 관측 경로가 없으면 테스트가 내부 필드를 들여다봐야 한다. 나중에 `nimwake status`로 이어진다. | [§15](../../prd/02-architecture/03-concurrency.md)      |

## 구현 중 확정 (`v0.0.2`)

| #   | 결정                                                              | 이유                                                                                                       | 반영 위치                                                    |
| --- | ----------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| D20 | `ProcessHandle`의 생존이 process의 생존 조건                       | startup timeout은 `Runtime::start`를 **취소**하는 방식으로 적용된다. 취소된 future가 child를 남기면 cold start마다 orphan이 쌓인다. handle이 모두 drop되면 controller가 정리하게 해서 구조적으로 막았다. | [§70](../../prd/02-architecture/04-runtime.md)               |
| D21 | `control`을 `Arc<dyn ProcessControl>`로 handle에 넣음              | runtime이 `ProcessId → Controller` 맵을 들면 죽은 entry를 걷어낼 책임이 새로 생긴다. 그 맵 자체가 누수 경로다. handle의 동일성은 `ProcessId`가 결정하도록 `PartialEq`를 직접 구현했다. | [§70](../../prd/02-architecture/04-runtime.md)               |
| D22 | `ProcessRuntime`은 Unix 전용 (`#[cfg(unix)]`)                      | SIGTERM / SIGKILL / process group은 Windows에 대응물이 없다. Tier 1/2가 모두 Linux 계열이므로 이식 비용을 지불할 이유가 없다. | [§19](../../prd/02-architecture/04-runtime.md), [§48](../../prd/04-platform/01-environments.md) |
| D23 | stdout/stderr는 inherit이 아니라 **drain**                          | daemon의 stdout이 어디로 가는지는 운영자가 정한다. daemon이 직접 읽어 app 이름과 함께 `tracing`으로 흘리면 그 선택과 무관하게 child가 막히지 않는다. | [§19](../../prd/02-architecture/04-runtime.md)               |

## 구현 중 확정 (`v0.0.3`)

| #   | 결정                                                       | 이유                                                                                                       | 반영 위치                                                    |
| --- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| D24 | lease를 응답 body에 실어 보낸다 (`LeasedBody`)              | service 함수가 반환하는 순간 반납하면, backend에서 응답이 흘러나오는 중인데도 `active_requests`가 0이 되어 idle shutdown 대상이 된다. 실제로 body가 중간에서 잘린다. | [§25–§26](../../prd/02-architecture/06-gateway-and-requests.md) |
| D25 | backend connection pool을 두지 않는다                       | scale-to-zero는 backend를 주기적으로 죽인다. pool에 남은 keep-alive 연결은 다음 요청에서 그대로 실패한다. 쓰려면 lifecycle 이벤트로 무효화해야 하고 그것은 MVP 범위가 아니다. | [§24](../../prd/02-architecture/06-gateway-and-requests.md)  |
| D26 | overload는 `503`, 기동 실패는 `502`                         | 둘을 같은 코드로 뭉개면 운영자가 "부하 때문인지 app이 깨진 것인지"를 로그 없이 구분할 수 없다.                | [§69](../../prd/02-architecture/06-gateway-and-requests.md)  |
| D27 | `AppRegistry::shutdown()`이 **정지 완료까지 기다린다**      | 정지 요청만 보내고 반환하면 daemon이 먼저 종료해 `shutdown_timeout_seconds`가 무시된다. SIGTERM을 받고 정리하는 application이 유예를 전혀 받지 못한다. | [§30](../../prd/02-architecture/07-scale-to-zero.md), [§71](../../prd/02-architecture/08-failure-handling.md) |

## 구현 중 확정 (`v0.0.4`)

| #   | 결정                                                       | 이유                                                                                                       | 반영 위치                                                    |
| --- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| D29 | 계측 값을 `u64`로 내보낸다                                  | `u128`을 쓰면 `tracing`이 JSON에 **문자열**로 적는다(`"queue_us":"142"`). 읽는 쪽이 숫자로 다룰 수 없다. 마이크로초 단위 `i64`는 29만 년을 담으므로 정밀도 손실이 없다. | [§72](../../prd/03-delivery/05-instrumentation.md)           |
| D30 | E2E를 `nimwaked` 패키지에 두고 fixture를 형제 경로에서 찾는다 | `CARGO_BIN_EXE_*`는 같은 패키지의 bin만 노출한다. daemon과 fixture를 동시에 쓰려면 한쪽은 경로로 찾아야 한다. workspace 바이너리는 모두 같은 디렉터리에 놓인다. | [§41](../../prd/03-delivery/02-testing-strategy.md)          |
| D31 | 부하 도구를 직접 만든다                                     | k6를 쓰면 CI에 외부 의존이 생기고, cold-start breakdown을 위해 daemon의 trace 출력과 맞물려야 한다. 그 결합을 외부 도구로 하면 더 복잡해진다. `v0.0.5` storm 테스트에서도 재사용한다. | [§42](../../prd/03-delivery/03-load-tests.md)                |
| D32 | daemon CLI를 직접 파싱한다                                  | 옵션이 셋뿐이다. `clap`은 `nimwake` CLI가 생길 때 함께 들인다. (PRD §35의 스택 선택은 CLI 도구를 가리킨다)     | [§35](../../prd/02-architecture/09-repository-and-stack.md)  |

### D29를 발견한 경위

E2E는 통과하는데 벤치 도구가 구간을 하나도 못 읽었다. daemon의 출력을 직접 보고 알았다.

```text
"queue_us":"142"      ← 문자열
"queue_us":142        ← 고친 뒤
```

E2E는 필드 **이름**만 확인했기 때문에 잡히지 않았다. 형식을 소비하는 쪽이 생겨야 드러나는 종류의 결함이다.

### D28 — 전이를 순수 함수로 분리

`AppManager`가 세 가지를 동시에 하고 있었다.

```text
(a) 전이 결정      state + event -> new state
(b) 부수효과 실행  tokio::spawn, 타이머, 채널 전송
(c) 식별자 발급    OperationId / LeaseId / IdleGeneration
```

(a)는 순수하게 쓸 수 있는데 (b)와 얽혀 있어서, 전이 하나를 검증하려면 actor 전체를
조립하고 mailbox를 굴려야 했다. 증상은 셋이었다.

| 증상 | 내용 |
| --- | --- |
| 무관한 변경이 테스트를 깬다 | `shutdown_ack` 필드 하나를 추가했더니 테스트의 `Actor { .. }` 구조체 리터럴이 컴파일되지 않았다 |
| 전이를 직접 단언할 수 없다 | `pump_until`이 최대 16회 임의 이벤트를 밀어 넣어야 목표 상태에 도달했다 |
| 예약된 일을 볼 수 없다 | 부수효과가 `tokio::spawn`으로 즉시 나가버려 `FakeRuntime`을 통해서만 간접 관측됐다 |

그래서 부수효과를 **값으로** 바꿨다.

```text
transition.rs   (상태, 이벤트, 문맥) -> (새 상태, Vec<Effect>)     순수
effect.rs       Effect — 무엇을 할지 기술하는 데이터
manager.rs      Effect를 실행하는 껍데기. 상태 판단이 한 줄도 없다
```

시각도 `Ctx::now`로 주입한다. idle 판정을 시계 조작 없이 검증할 수 있다.

**비용도 적어 둔다.** core 코드가 453줄에서 666줄로 약 210줄 늘었다. `Effect`를 만들고
푸는 보일러플레이트다. 대신 `manager.rs`는 453줄에서 147줄이 됐고, 단위 테스트는
async runtime·fake·이벤트 루프 없이 도는 30개에서 39개가 됐다.

`v0.0.4`가 `RequestTrace`/`StartupTrace`를 도입하기 **직전**에 한 것은 의도적이다.
계측 지점은 전부 전이와 부수효과 안이고, 지금 나눠 두면 계측은 `Actor::apply` 한 곳에만 붙는다.

### 왜 파일을 더 쪼개지 않았나

`transition.rs`가 493줄로 가장 큰 파일이 됐지만 여기서 멈춘다.
상태 머신은 하나의 응집된 것이고, Invariant 2가 상태 변경 주체를 하나로 요구한다.
상태별로 파일을 나누면 전수 패턴 매칭이 흩어져서, 길이를 줄이는 대신 이 설계를
성립시키는 성질을 잃는다. 더 커진다면 상태가 아니라 **이벤트 묶음** 기준으로 나눈다.

### D27을 발견한 경위

`nimwaked`를 처음 실제로 띄워 본 스모크 테스트에서 나왔다. 단위 테스트와 통합 테스트는
`shutdown()` 이후에도 테스트 프로세스가 계속 살아 있어 detached stop task가 끝날 시간을 준다.
**daemon만이 shutdown 직후 종료한다.** 그래서 이 결함은 daemon을 실제로 띄워야만 드러났다.

`kill_on_drop(true)` 덕분에 orphan은 생기지 않았다. 대신 조용히 잘못된 것은 종료 방식이다.

```text
고치기 전   SIGTERM 전송 → daemon 종료(0.003s) → kill_on_drop이 SIGKILL
고친 후     SIGTERM 전송 → shutdown timeout(2s) 대기 → SIGKILL → daemon 종료(2.004s)
```

즉 `shutdown_timeout_seconds`가 daemon 종료 경로에서만 무시되고 있었다. (PRD §30)

### 코드 수준 주의사항

- lifecycle 시간 판단은 **`tokio::time::Instant`** 를 쓴다.
  `std::time::Instant`는 `tokio::time::pause()`의 가상 시계를 따르지 않아 idle timeout 테스트가 실시간에 묶인다.
- `stop`이 끝나면 `StopCompleted`와 `ProcessExited`가 **둘 다** 도착한다.
  먼저 온 것이 전이를 수행하고 나중 것은 stale로 무시된다. 테스트는 event 개수를 세지 말고 목표 상태까지 진행시켜야 한다.
- process group에 `SIGKILL`을 보내는 것은 **reap보다 먼저**여야 한다.
  직접 child가 zombie로 남아 있는 동안에만 그 pid(= pgid)가 예약되어 있다. 거둔 뒤에 보내면 재사용된 pid의 엉뚱한 group을 죽일 수 있다.
- fixture는 **응답 전에 요청을 읽어야 한다.**
  readiness probe는 아무것도 보내지 않고 연결을 닫는데, 읽지 않고 응답부터 쓰면
  그 죽은 연결을 붙들고 있는 동안 진짜 요청이 backlog에서 기다린다.
  `slow-body-server`가 이것 때문에 5회 중 1회꼴로 `operation was canceled`를 냈다.
- Gateway E2E에서 HTTP client는 blocking이므로 반드시 `spawn_blocking`으로 부른다.
  테스트 thread에서 곧바로 부르면 같은 runtime 위의 gateway task가 굶어 deadlock이 된다.
- orphan 여부를 `pgrep -f`로 확인하지 말 것.
  검사 스크립트 자신의 command line이 패턴에 걸려 **오탐**이 난다. 실제로 이 오탐 때문에
  있지도 않은 orphan을 쫓았다. 대상 pid를 직접 들고 `kill -0`으로 보거나 listener를 본다.
- 통합 테스트의 port를 `bind(:0)`으로 고르면 안 된다.
  돌려준 직후 커널이 같은 번호를 다른 연결의 ephemeral port로 내주고, readiness probe가 엉뚱한 listener에 붙는다.
  실제로 이 경로에서 테스트가 간헐적으로 깨졌다. ephemeral range 밖에서 원자 counter로 고른다.

---

## P2 — Core MVP 이후

| #   | 항목                                    | 처리                                                            |
| --- | --------------------------------------- | --------------------------------------------------------------- |
| D12 | HTTP body 크기 상한 / long-lived 정책   | [§24](../../prd/02-architecture/06-gateway-and-requests.md)에 계약만 명시, 상세는 Post-MVP |
| D13 | crash-loop backoff                      | [§33](../../prd/02-architecture/08-failure-handling.md) · [§63](../../prd/05-beyond-mvp/03-long-term-roadmap.md) |
| D14 | dynamic port allocation                 | [§21](../../prd/02-architecture/05-manifest-and-readiness.md) · [§63](../../prd/05-beyond-mvp/03-long-term-roadmap.md) |
| D15 | process adoption (daemon restart 후 인수) | [§71](../../prd/02-architecture/08-failure-handling.md)에서 명시적으로 제외, Phase 2 재검토 |

---

## 유지하기로 한 결정

| 항목                          | 판단                                                              |
| ----------------------------- | ----------------------------------------------------------------- |
| per-app actor 모델            | 유지. 이 프로젝트 규모에 적합                                      |
| Runtime abstraction           | 유지. adapter 교체 시 lifecycle semantics 불변                     |
| raw process runtime           | 유지                                                               |
| Gateway와 lifecycle 분리      | 유지. `acquire()` 하나로 표면을 더 좁힘                            |
| STOPPING 취소 불가 정책       | 유지. SIGTERM 이후 되돌리기는 application마다 의미가 달라진다      |
| static port                   | 유지하되 load 시점 validation 필수                                 |
| 성능 수치를 release gate로 안 씀 | 유지. 공유 runner 편차 때문 ([§73](../../prd/03-delivery/07-performance-baseline.md)) |

구조적 재설계가 필요한 결함은 발견되지 않았다.

---

← [완료 체크리스트](04-acceptance.md) · [페이즈 개요](README.md) · [페이즈 로드맵](../README.md)
