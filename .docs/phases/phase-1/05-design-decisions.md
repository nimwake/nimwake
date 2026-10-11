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

## tarball의 바이너리 중 하나만 실행 확인되고 있었다

soak이 끝나기 전에 처리할 것을 찾다가 발견했다. `release.yml`의 qemu smoke는
**`nimwaked`만** 돌린다. tarball에는 여섯이 들어간다.

```text
nimwaked              smoke 있음
nimwake-bench         없음   ← §50 비교를 만드는 도구
echo-server           없음
crash-server          없음
group-server          없음
slow-start-server     없음
```

`nimwake-bench`가 특히 나쁘다. #26에서 tarball에 넣은 뒤로 **aarch64에서 한 번도
실행된 적이 없다.** 기기에서 처음 돌리다 못 뜨는 것을 발견하면 그날 측정을 못 하고,
soak이 끝난 직후가 바로 그 날이다.

### 각 바이너리의 깔끔한 실패 모드를 먼저 쟀다

추측하지 않고 하나씩 돌려 확인했다.

```text
nimwaked                      인자 없음        → 78   (EX_CONFIG)
nimwake-bench --nimwaked X    경로 없음        → 1    "not found at X"
echo-server                   port 인자 없음   → 101  (panic)
group-server                  같음             → 101
slow-start-server             같음             → 101
crash-server                  인자를 안 쓴다    → 1    "failing on purpose"
```

`nimwake-bench`를 **인자 없이** 부르면 옆 바이너리를 찾아 **전체 벤치를 돌린다**
(5초 timeout에 걸려 124가 나왔다). smoke에는 없는 경로를 주어 경로 검사에서
끝나게 한다.

### 깨뜨려 확인했다

smoke 스크립트를 workflow에서 추출해 네이티브로 돌렸다.

```text
nimwake-bench를 깨진 파일로    ::error::nimwake-bench did not run (exit 127)
echo-server를 빼면             ::error::echo-server did not run (exit 127, expected 101)
복원                            통과
```

통과만 보고 넘기면 아무것도 지키지 않는 smoke가 남는다. 이번 세션에서 네 번째로
같은 확인을 했다.

---

## cold start 분해를 표본 하나로 내고 있었다

`v0.1.0-rc2`를 찍고 baseline을 보다가 발견했다. 직전 run과 비교하니 같은 지표가
28배 달랐다.

```text
직전 run   cold_start_readiness_ms = 50.979   → 리포트: "지배적인 구간: app readiness"
rc2 run    cold_start_readiness_ms =  0.074   → 리포트: "지배적인 구간: process spawn"
```

**같은 코드, 같은 fixture인데 결론이 뒤집혔다.**

### 원인은 표본 수와 양자화다

cold-start storm은 **기동 중복 제거를 증명하는 것이 목적**이다. 1000개 동시 요청이
process 하나만 띄워야 한다. 그래서 `spawn count = 1`이고 기동 구간 표본도 **하나**다.

그 하나가 `READINESS_PROBE_INTERVAL = 50ms`에 양자화된다.

```text
첫 connect에 붙으면      readiness ≈ 0ms
한 번 더 기다리면         readiness ≈ 50ms
```

표본이 하나이므로 어느 쪽에 걸리는지가 그대로 결론이 된다. 50ms 쪽은 드물게
일어나는데(40 표본에서 p95도 0.04ms), 드물다는 것이 더 나쁘다 — 대부분은 맞게
나오고 가끔 틀리게 나온다.

### lifecycle stress가 표본을 버리고 있었다

stress는 cycle마다 cold start를 한다. 1000 cycle = 기동 1000건이다.
`daemon.startups()`가 그것을 모으고 있었는데 `StressResult`가 담지 않았다.

담게 하고, 분해를 **p50과 p95**로 낸다.

```text
회차 1   queue 0.034 · readiness 0.046 · spawn 0.189 · total 1.053   (40 표본)
회차 2   queue 0.036 · readiness 0.037 · spawn 0.144 · total 0.557   (40 표본)
```

재현된다. `cold_start_samples`도 baseline에 담아 읽는 쪽이 표본 수를 안다.

p95를 함께 적는 이유가 있다. 양자화된 꼬리가 있으면 **p95에 드러난다.**
p50만 적으면 "가끔 50ms"가 보이지 않는다.

### storm 절은 단일 표본임을 명시했다

지우지 않았다. storm의 기동 한 건도 정보다. 다만 리포트가 그것으로 "지배적인
구간"을 고르지 않게 하고, 표본이 하나라서 구간 비교에 쓰지 않는다고 적었다.

### §50 비교를 이 값으로 할 수 없었다

기기와 CI를 나란히 놓으려던 그 숫자다. 고치지 않았으면 **동전 던지기를 비교**했다.

---

## 같은 함정을 다른 테스트도 밟고 있었다

§19 작업의 교훈이 "한 파일의 경고가 옆 파일에 닿지 않았다"였으므로 전체를
쓸어 보았다.

```text
start_waits_for_readiness_and_stop_removes_the_process   http_get를 직접 부른다
registry_shutdown_leaves_no_managed_child                오탐 (registry.get)
```

앞의 것은 진짜다. `ProcessRuntime::start`가 drain task를 띄운 뒤 단일 스레드
runtime에서 blocking client를 직접 부른다. `echo-server`는 출력이 적어 pipe가
차지 않으므로 **지금은** 통과한다. fixture가 수다스러워지는 순간 깨진다.

`spawn_blocking`으로 감쌌다.

### 경고를 검사로 옮겼다

`scripts/check-blocking-clients.py`를 ci.yml에 연결했다. 통과 조건은 둘 중 하나다.

```text
flavor = "multi_thread"
spawn_blocking으로 감싼다
```

### guard가 주석에 속았다

처음 판은 본문에 `spawn_blocking`이라는 **단어**가 있는지만 보았다. 그래서
위반을 통과시켰다 — 내가 그 테스트에 써 넣은 설명 주석에 그 단어가 있었기
때문이다.

```text
"blocking client는 spawn_blocking으로 부른다"   ← 주석
```

주석을 걷어내고 `spawn_blocking(` 형태의 **호출**만 세도록 고쳤다.

**일부러 깨뜨려 보지 않았으면 몰랐을 것이다.** guard를 넣고 "통과한다"만
확인하면 아무것도 지키지 않는 guard가 남는다. 이번 루프에서 세 번째로 같은
방식에 걸렸다 — §29의 세대 비교, §19의 drain 테스트, 그리고 이 guard다.

---

## §19의 "pipe 방치 금지"를 지키는 테스트가 지키지 않고 있었다

§19는 분명하게 적는다.

> child를 `Stdio::piped()`로 실행하고 읽지 않으면 pipe buffer가 가득 차면서
> **child가 멈출 수 있다.** ... pipe를 생성해 두고 방치하는 구성은 허용하지 않는다.

`a_noisy_process_keeps_serving`이 그것을 본다고 적혀 있었다. **drain 호출을
제거해도 통과했다.**

### 두 가지가 동시에 틀려 있었다

**하나는 fixture다.** `noisy-server`가 stdout을 **별도 스레드에서** 쏟아 냈다.
pipe가 차면 그 스레드만 멈추고 main은 계속 accept하고 응답한다. 위험을 모델링하지
않은 것이다.

응답 직전에 쓰도록 바꿨다. 로그를 남기고 응답하는 보통의 application과 같은
모양이고, pipe가 막히면 응답도 막힌다. 요청마다 128KiB를 쓴다.

**다른 하나는 테스트다.** 고친 fixture로 돌리니 **drain이 있는데도** 실패했다.
`#[tokio::test]`는 단일 스레드이고 HTTP client는 blocking이다. 테스트 thread에서
직접 부르면 `tokio::spawn`으로 도는 **drain이 굶는다.** pipe가 차고 child가 멈춘다.

```text
EAGAIN (os error 11) — client 읽기 시간 초과
```

`flavor = "multi_thread"` + `spawn_blocking`으로 바꿨다.

### 이 교훈은 이미 저장소에 적혀 있었다

`gateway.rs`의 모듈 doc이다.

> HTTP client는 blocking이므로 항상 `spawn_blocking`으로 부른다.
> 테스트 thread에서 곧바로 부르면 같은 runtime 위에 있는 gateway task가 굶는다.

같은 함정을 `process_runtime.rs`가 밟고 있었다. 한 파일에 적어 둔 교훈이 옆
파일에 적용되지 않았다.

### 고친 뒤

```text
drain 있음   0.05초 통과
drain 제거   "stopped responding" 실패
```

이제 §19가 테스트로 강제된다.

---

## §20의 app별 분리가 암묵적으로만 확인되고 있었다

§20은 isolation 정책을 여섯 줄로 적는다. 그중 검증 가능한 것은 둘이다.

```text
app별 working directory 분리
app별 environment / secret 분리 (`[runtime.env]`, daemon 환경 위에 overlay)
```

`[runtime.env]`는 `hello-node` e2e에서 `PORT`를 넘기는 데 쓰인다. 그러면 "변수가
자식에게 닿는다"까지는 확인된다. **overlay인지는 확인되지 않는다.**

### overlay와 덮어쓰기는 다르다

```text
overlay   daemon 환경 + app 환경, 같은 키는 app이 이긴다
덮어쓰기   app 환경만
```

덮어쓰기로 구현하면 app이 지정하지 않은 변수가 사라진다. **`PATH`가 없으면
대부분의 런타임이 기동하지 못한다.** 격리를 강화한다는 이유로 `.env_clear()`를
넣기 쉬운 자리이고, 그러면 조용히 깨진다.

### 밖에서 관측할 수 있게 만들었다

`echo-server`에 두 경로를 더했다. fixture가 자기 상태를 말할 수 있어야 테스트가
단언할 수 있다.

```text
GET /cwd      자기 작업 디렉터리
GET /env/X    환경변수 X의 값
```

`/env/X`가 임의의 이름을 받는 것이 중요하다. app이 지정하지 **않은** 변수를
물어볼 수 있어야 overlay를 확인할 수 있다.

`the_child_gets_its_own_directory_and_an_overlaid_environment`가 넷을 단언한다.

```text
app이 지정한 변수          보인다
daemon에만 있는 변수       보인다        ← overlay의 증거
양쪽에 있는 변수            app이 이긴다
작업 디렉터리               app의 것이다
```

### 뮤테이션 둘로 확인했다

```text
.env_clear() 추가        "daemon 환경을 덮지 않고 올려 쓴다" 단언이 실패
working_dir를 "." 로 고정  "child must run in the app's working_dir" 단언이 실패
```

각 뮤테이션이 **의도한 단언에서** 잡힌다. 다른 단언이 먼저 걸렸다면 테스트가
우연히 통과하고 있었다는 뜻이다.

---

## §11이 집합을 쓰는 이유 중 하나만 테스트돼 있었다

§11은 `leases`를 `HashSet`으로 두는 이유를 **둘**로 적는다.

```text
중복 반납                       a_duplicate_request_finished_does_not_change_the_counter
이미 교체된 process에 대한 반납   없었다
```

앞의 것은 D16으로 들어와 있었다. 뒤의 것은 incarnation을 넘는 경우이고 테스트가
없었다.

### 왜 다른 경우인가

```text
중복   같은 incarnation에서 같은 lease를 두 번 반납
교체   incarnation 1의 lease가 incarnation 2에 도착
```

정수 카운터였다면 늦게 도착한 옛 반납이 그대로 감소시켜, **살아 있는 요청이
있는데도** idle timer가 걸린다. 그러면 응답 중에 process가 정지된다. §11이
집합을 고른 이유가 이것이다.

`returning_a_lease_from_a_replaced_process_changes_nothing`으로 고정했다.

### 뮤테이션으로 확인했다

집합 의미를 정수 카운터 의미로 바꿨다 — 들어 있는지 보지 않고 아무 하나를 지운다.

```rust
if let Some(any) = running.leases.iter().next().copied() {
    running.leases.remove(&any);
}
```

두 테스트가 모두 실패한다. §11의 두 이유가 이제 양쪽 다 받쳐진다.

---

## §23~26 감사 — 하나만 비어 있었다

gateway 절 넷을 코드와 대조했다.

```text
§23 Gateway Routing     Host 정규화까지 받쳐진다 (아래)
§24 HTTP Contract       메서드 여섯 중 둘만 테스트돼 있었다   ←
§25 Request Tracking    구조적 주장은 Invariant 3 guard와 privacy로 덮인다
§26 Acquire / Lease     테스트가 있다
```

### §24 — GET과 POST만 쓰고 있었다

§24는 `GET / HEAD / POST / PUT / DELETE / PATCH`를 보장 범위로 적는다.
테스트는 GET과 POST만 썼다. 나머지 넷은 **동작하지만 아무것도 고정하고 있지
않았다.** method 필터가 들어오거나 `to_backend_request`가 무언가를 떨어뜨리면
조용히 깨진다.

`every_contract_method_passes_through`로 여섯을 모두 돌린다. HEAD는 응답에 body가
없어야 하므로 따로 단언한다. gateway가 GET 외를 거부하도록 뒤집어 잡히는 것을
확인했다.

### 조사하다 한 번 헛짚었다

HEAD가 gateway를 지나면 10초 내내 멈추는 것으로 보였다. 앱에 직접 보내면
0.0002초였으니 gateway 결함으로 읽혔다.

**측정 방법이 틀렸다.** `curl -X HEAD`는 응답이 bodiless인 것을 모르고 body를
기다린다. 규격을 지키는 backend를 따로 만들어 비교해 보니 **직접 요청도 똑같이
멈췄다.** `curl -I`로 보면 양쪽 모두 정상이다.

거짓 결함을 보고하기 전에 걸러진 이유는 "앱에 직접"이라는 대조군을 함께
측정했기 때문이다. 한쪽만 재면 gateway 탓으로 끝났다.

### §23 — 우려했던 것이 이미 처리돼 있었다

기본 listen이 `127.0.0.1:8080`이므로 브라우저는 `Host: name:8080`을 보낸다.
정확히 일치 검사였다면 브라우저로 접근하는 가장 흔한 경우가 깨진다.

`normalize_host`가 포트를 떼고 소문자화하며, 단위·lifecycle·gateway 세 곳에서
테스트된다. 확인만 하고 지나갔다.

---

## §75를 넣으면서 §22의 검증 목록을 깨뜨렸다

§22는 manifest 검증 항목 여덟을 열거하고 "하나라도 위반하면 기동을 거부한다"고
적는다. 코드와 대조했다.

```text
unique name                       DuplicateName           있음
unique host                       DuplicateHost           있음
unique port                       DuplicatePort           있음
port != gateway port              GatewayPortConflict     있음
valid command (non-empty)         EmptyCommand            있음
valid working_dir (존재하는 경로)   MissingWorkingDir       있음
idle/startup/shutdown timeout > 0 NotPositive             셋만
max_pending_requests > 0          NotPositive             있음
```

`response_header_timeout_seconds`가 빠져 있었다. **#23에서 내가 추가한 필드다.**
양수 검증 목록에 넣지 않았다.

### 0이면 application이 영원히 응답하지 못한다

```text
고치기 전   검증 통과 → 모든 요청이 53ms에 504
고친 후     종료 코드 78 (EX_CONFIG)
```

상한이 0이므로 연결을 열기도 전에 만료된다. 멀쩡한 application을 쓸 수 없게
만드는 설정이 조용히 통과했다.

### 테스트가 하나만 보고 있었다

`rejects_zero_timeout`은 `idle_timeout_seconds` 하나만 단언했다. 그래서 목록에
필드가 늘었을 때 아무것도 알려주지 않았다.

네 timeout을 **하나씩 다 보도록** 넓혔다.

```text
rejects_a_zero_value_for_every_timeout
  idle · startup · shutdown · response_header
```

고친 뒤 네 경우 모두 종료 코드 78로 거부되는 것을 daemon으로 확인했다.

### 교훈은 "목록을 대표값 하나로 테스트하지 않는다"

이 종류를 오늘 두 번 겪었다. e2e가 계측 필드를 **이름만** 보고 있어서
`proxy_connect_us`가 잘못된 값을 담은 것을 못 잡았고, 여기서는 timeout **하나만**
보고 있어서 새 필드가 빠진 것을 못 잡았다.

목록을 검증할 때는 목록을 돌아야 한다.

---

## §29가 처방한 기제가 실제로 일하고 있지 않았다

§29는 stale idle timer를 `IdleGeneration` 비교로 막는다고 적고, §68이 그 identity를
정의한다. idle timeout 판정은 세 조건을 본다.

```rust
running.idle_generation == generation
    && running.leases.is_empty()
    && ctx.now.duration_since(running.last_request_at) >= idle_timeout
```

어느 조건이 실제로 막는지 하나씩 지워 보았다.

```text
leases.is_empty() 제거   returning_one_of_many_leases_does_not_arm_the_timer 실패
시간 검사 제거            an_early_idle_timer_does_not_stop_the_app 실패
세대 비교 제거            아무 테스트도 실패하지 않는다
```

**세대 비교에 의존하는 테스트가 하나도 없다.**

### 테스트를 쓸 수 없다

처음에는 테스트가 빠진 것으로 보고 쓰려 했다. 쓸 수 없다.

옛 timer가 도착한 시점에 lease가 비어 있고 idle timeout도 지났다면,
**어느 timer가 깨웠든 멈추는 것이 옳다.** 세대를 보든 보지 않든 결과가 같다.
행동으로 구분되는 경우가 없으므로 단언할 것이 없다.

기존 `a_stale_idle_timer_is_ignored`도 세대 때문에 통과하는 것이 아니다.
새 acquire가 lease를 발급하므로 `leases.is_empty()`에서 걸린다.

### 그래도 두고, 코드에 적었다

지우지 않았다. PRD가 처방한 기제이고 뒤의 두 조건이 약해지면 마지막 방어선이 된다.

대신 판정부 주석에 **주된 보호가 아니라는 것**을 적었다. §29를 읽고 "세대가
막아 준다"고 믿으면 틀린 전제로 코드를 고치게 된다. 시간 검사를 손보면서
"세대가 있으니 괜찮다"고 판단하는 것이 정확히 그 경우다.

### §29의 둘째 문단은 테스트가 없었다

```text
있던 것   Running(N) → 새 acquire로 N+1 → timer(N) 발화 → 무시
없던 것   Running(N) → SLEEPING을 거쳐 Running(M) → timer(N) 발화 → ?
```

후자를 `an_idle_timer_from_a_previous_incarnation_is_ignored`로 채웠다.
무시되는 것 자체는 `leases` 검사가 보장하지만, 이 테스트의
`assert_ne!(h.idle_generation(), previous)`는 **세대가 재사용되지 않는다**는
별개의 성질을 고정한다.

그 성질은 수열이 `Ctx`에 있어 application 수명 전체에 걸쳐 단조 증가하기
때문에 성립한다. PRD의 표현("`Running` state가 소유하므로")은 수열을 `Running`
안으로 옮기도록 오해를 부를 수 있고, 그러면 새 incarnation이 같은 번호를 다시
내준다. 그때 이 단언이 걸린다.

---

## §12의 불변식 열 중 셋은 테스트로 잡히지 않는 종류였다

완료 체크리스트는 DoD의 동시성 항목 일곱을 근거와 함께 담고 있었지만, **§12의
Invariant 1~10과 1:1이 아니었다.** 대조해 보니 셋은 어떤 테스트도 지키지 않고
있었다.

```text
2  Single Lifecycle Authority    state를 바꾸는 주체가 AppManager 하나
3  Gateway Isolation             gateway는 acquire()만 쓴다
6  Non-Blocking Actor            긴 작업은 event loop 밖에서
```

셋 다 **지금은 성립한다.** 문제는 테스트로 표현할 수 없는 종류라는 것이다.
"두 번째 장소에서 상태를 바꾸지 않는다"는 어떤 입력으로도 확인할 수 없다.
입력이 아니라 **구조**의 성질이다.

저장소에 이미 같은 종류의 CI guard가 둘 있었다 — `unbounded_channel` 금지(Invariant
9)와 `tokio::process` import 금지(§70). 그 패턴으로 셋을 더했다.

```text
Forbid a second lifecycle authority        transition::을 manager.rs 밖에서 적용하면 실패
Forbid the gateway from reaching into...   gateway가 runtime·AppManager를 import하면 실패
Keep the decision path synchronous         transition.rs에 async fn·.await가 들어오면 실패
```

### Invariant 6을 동기성으로 환원했다

이것이 세 guard 중 가장 깔끔하다. `transition`은 **동기 순수 함수**다.
`async`가 아니면 애초에 기다릴 수 없다. 기동·종료·monitor는 `Effect`로 나가
별도 task에서 수행된다.

```text
Effect::Start · Effect::Stop · Effect::Monitor · Effect::ScheduleIdleTimer ...
```

"오래 걸리는 작업을 event loop 안에서 하지 않는다"를 직접 검사하는 대신
**애초에 불가능하게 만든 것을 지킨다.** `transition.rs`에 `async fn`이 들어오는
순간 그 보장이 사라지므로 거기를 본다.

세 guard 모두 일부러 깨뜨려 잡히는지 확인했다.

### 매핑을 문서에 남겼다

체크리스트에 Invariant 1~10 표를 넣고 각 항목이 `test` / `guard` / `gate` /
`report` 중 무엇으로 지켜지는지 적었다. 인용한 식별자 17개가 실존하는지도
확인했다.

---

## `proxy_connect_latency`가 이름과 다른 것을 재고 있었다

§72의 파생 지표 일곱을 코드와 대조했다. 여섯은 맞고 하나가 달랐다.

```text
§72 정의                                        구현
actor_queue_latency                             queue_us            맞음
startup_latency                                 total_us            맞음
spawn_latency                                   spawn_us            맞음
readiness_latency                               readiness_us        맞음
time_to_first_byte                              ttfb_us             맞음
proxy_connect_latency  proxy_started → 연결 수립  proxy_started → response_started
request_total_latency  received → 응답 완료        없음
```

`proxy_connect_latency`가 **backend 왕복 전체**를 담고 있었다. 연결 수립,
handshake, 요청 전송, application 처리, 응답 header가 한 값에 섞여 있었다.

trace 모듈의 doc은 의도를 정확히 적어 두었다.

```text
actor queue       3ms
spawn             8ms
app readiness   821ms  ← bottleneck
proxy connect     2ms
```

`proxy connect`가 별도 구간인 이유는 분명하다. 둘을 합쳐 두면 **"연결이 느린가
application이 느린가"를 구분할 수 없다.** 저사양 환경에서 어느 구간이 먼저
무너지는지를 보려는 것이 §50의 목적이므로 나누어야 한다.

### 나눈 결과

```text
요청 1 (cold)  routing 2 · acquire 51,491 · connect 44 · backend 133 · ttfb 51,672
요청 2 (warm)  routing 1 · acquire     9 · connect 74 · backend 145 · ttfb    231
```

고치기 전에는 `proxy_connect_us`가 177µs(cold) · 219µs(warm)로 나왔다.
연결은 44~74µs이고 나머지는 backend다.

`forward`가 timeout 블록 안에서 연결하므로 그 안에서 찍은 시각을 **값으로
돌려받아** 밖에서 기록한다. 블록이 trace를 붙잡으면 await를 넘는 가변 대여가 된다.

### e2e가 이것을 못 잡고 있었다

단언이 **필드 이름만** 보고 있었다.

```rust
for field in ["acquire_us", "proxy_connect_us", "ttfb_us"] {
    assert!(request.contains(field));
}
```

D29와 같은 함정이다. 그때는 계측값을 `u128`로 두어 tracing이 JSON에 **문자열**로
적었고, 이름만 보는 단언이 통과시켰다.

숫자로 뽑아 **구간 관계**를 단언하도록 바꿨다. 문자열이면 panic한다.

```text
routing + acquire + connect + backend <= ttfb
connect + backend <= ttfb
```

### `request_total_latency`는 넣지 않았다

§72의 목록에 있지만 구현하지 않았다. 이유를 적어 둔다.

응답 **완료** 시점은 body가 다 흐른 뒤이고, 그것을 찍으려면 trace가 요청 handler
보다 오래 살아 `LeasedBody`로 넘어가야 한다. 로그가 나가는 시점도 "header 전송"
에서 "body 완료"로 바뀐다.

그리고 §72의 논지가 *"밖에서는 총 latency밖에 볼 수 없어"* 구간 분해를 Core가
제공해야 한다는 것이다. **총 latency는 client가 이미 재고 있다** —
`warm_p50_ms`와 `warm_p95_ms`가 그것이다. 목록에서 Core가 더할 값이 가장 적은
항목이다.

### 게이지

```text
active_requests · pending_requests   내부적으로 추적한다. trace로 내보내지는 않는다
running_apps                         개념이 없다
daemon_cpu · daemon_rss · open_fds   nimwake-bench가 /proc에서 읽는다
```

뒤의 셋은 Core가 아니라 부하 도구가 읽는다. §72가 "Core가 제공해야 한다"고 한
대상은 **구간 분해**이고 그 근거는 밖에서 볼 수 없다는 것이다. process RSS와 CPU는
밖에서 `/proc`으로 볼 수 있으므로 daemon 안에 같은 것을 두 벌 두지 않았다.

---

## 비교용 `baseline.txt`가 §50의 항목을 담지 않았다

§50은 "CI Linux와 Android 실기기에서 같은 workload로 비교"를 요구하고, 비교에
쓰이는 기계 판독 형식이 `baseline.txt`(`key=value`)다. 그런데 거기에 담긴 것은
다섯 항목 중 둘뿐이었다.

```text
§50 비교 항목                                   고치기 전
proxy overhead p50 / p95                        p95만
cold start breakdown (spawn/readiness/proxy)    queue · total만
daemon RSS                                      있음
CPU                                              없음
concurrency knee point                           없음
```

값이 없어서가 아니다. `StartupSpans`는 `spawn`과 `readiness`를 이미 갖고 있고,
CPU는 바로 앞 작업에서 추가했고, sweep은 모든 단계의 처리량을 갖고 있었다.
**리포트 표에만 있고 비교용 형식에는 넣지 않았다.** 그러면 Android 쪽이 markdown
표를 파싱해야 한다.

13개 키로 다섯 항목을 모두 담았다.

```text
proxy_overhead_p50_ms · proxy_overhead_p95_ms
cold_start_queue_ms · cold_start_spawn_ms · cold_start_readiness_ms · cold_start_total_ms
daemon_rss_mb
sustained_cpu_pct_of_wall
concurrency_knee · concurrency_knee_p95_ms · concurrency_knee_rps
```

CPU는 절대 시간이 아니라 **경과 대비 비율**로 담는다. 비교 대상이 CI runner와
저사양 기기여서 절대값은 나란히 놓기 어렵다.

### knee를 어떻게 정의했나

sweep 모듈의 doc은 "knee point 탐색"이라 적고 있었지만 **계산하지 않았다.**
표를 눈으로 보고 찾는 수밖에 없었다.

"최대 처리량을 낸 동시성"으로 정의하지 않았다. 포화된 뒤에는 어느 단계가 가장
빨랐는지가 runner 편차로 갈려 값이 튄다.

> **최대 처리량의 95%에 처음 도달한 동시성**

그 뒤로 동시성을 올리면 처리량은 거의 늘지 않고 지연만 늘어난다. 저사양 환경에서
이 지점이 얼마나 앞으로 당겨지는지가 §50이 보려는 것이다.

임계값 `0.95`는 근거가 아니라 **선택**이다. `stats::KNEE_FRACTION` 한 곳에 두고
주석에 그렇게 적었다.

순수 함수로 두어 테스트했다. 그중 하나는 **포화 뒤의 최대값이 knee를 뒤로
끌고 가지 않는지**를 본다 — 그것이 "최대 처리량"으로 정의하지 않은 이유다.

```text
stats::knee_tests  4개
```

---

## §44의 `deadlock == 0`을 실제로 강제한다

§44는 release gate의 차단 기준 다섯을 명시한다. 대조해 보니 넷은 검사가 있고
하나는 없었다.

```text
duplicate process == 0   storm의 unique_pids · crash_loop의 max_children · stress의 spawn count
orphan process == 0      stress의 survivors · release.yml의 "Assert nothing survived"
HTTP errors == 0         storm · stress · sustained · crash_loop의 실패 수
lost waiter == 0         startup_failure_rejects_every_waiter 등 3개 테스트
deadlock == 0            없었다
```

완료 체크리스트도 정직하게 적어 두었다 — 근거가 "전체 테스트가 기한 안에 끝난다"였다.

### 멈추면 60분을 태우고 아무것도 알려주지 않았다

기한이 **job 단위에만** 있었다. lifecycle deadlock이 생기면 `qualify`가 60분
뒤에 죽고, 어느 단계가 멈췄는지는 로그를 뒤져야 알 수 있다. private 저장소라
그 60분은 실제 비용이다.

긴 단계마다 `timeout-minutes`를 두었다. 11곳이다.

```text
release.yml   Build 20 · Tests 20 · Stress 25 · cross Build 30 · qemu smoke 10 ...
integration   Build 20 · End-to-end 20 · Performance baseline 20
ci            Test 15
```

실측 대비 넉넉하다(stress는 148초인데 25분). runner 편차를 감당하면서도 멈춘
것은 **이름과 함께** 빠르게 드러난다.

---

## release gate를 디스패치로 확인했다

`release.yml`을 세 번 건드렸다. 모두 태그에서 처음 실행될 수 있었다.

```text
#26  nimwake-bench 패키징
#27  태그 gate
#28  버전 추출을 태그 아닐 때도 실행
```

브랜치에서 디스패치해 확인한 것:

```text
Read the crate version                 success   (태그 아닌데도 실행됨)
Assert the tag matches the version      skipped   (의도대로)
artifact에 nimwake-bench                1,080,056 bytes
                                        ELF 64-bit, ARM aarch64, statically linked
BUILD_INFO                              target=aarch64-unknown-linux-musl
```

CPU 측정도 CI에서 동작한다. 그리고 **비교 대상으로서 이미 의미가 있다.**

```text
Sustained warm load   로컬 289% of wall  →  CI runner 104% of wall
Crash loop            로컬 150%          →  CI runner  59%
```

같은 workload를 다른 하드웨어에서 돌린 차이다. PRD §50이 Android 실기기로
한 번 더 넓히려는 축이 이것이다.

---

## daemon이 자기 버전을 틀리게 알렸다 (이슈 #24)

`v0.1.0-rc1` release의 `nimwaked`가 기동 로그에 `0.0.1`을 적었다.
같은 바이너리의 `BUILD_INFO`는 `ref=v0.1.0-rc1`이었다.

```text
nimwaked 0.0.1 — 1 application(s) validated
```

`[workspace.package] version`이 `0.0.1`로 남아 있었고 `nimwaked`가
`version.workspace = true`로 그 값을 쓴다. 운영 로그만으로는 어떤 release가
도는지 알 수 없었다.

**Android 실기기에서 먼저 발견됐다.** 이 세션에서도 `nimwaked 0.0.1` 출력을
여러 번 보았지만 문제로 읽지 않았다. 숫자가 눈앞에 있어도 기대값을 모르면
틀린 것을 알아채지 못한다.

### 버전을 올리고 gate를 두었다

```text
[workspace.package] version = "0.1.0"
```

`release.yml`의 qualify에 태그와 crate 버전이 다르면 실패하는 단계를 넣었다.
빌드보다 **먼저** 본다. 어긋났으면 5분을 쓰기 전에 멈추는 편이 낫다.

```text
v0.1.0        통과
v0.1.0-rc1    차단
v0.0.1        차단
```

**rc를 찍으려면 workspace 버전을 그 rc로 먼저 올려야 한다.** 매 release마다
버전 commit이 생기는데, 그것이 이슈가 요구한 규율이다.

### jq를 쓰지 않은 이유

처음에는 `cargo metadata | jq`로 썼다. 그런데 개발 머신(WSL)에 jq가 없어
**gate를 로컬에서 확인할 수 없었다.** 확인하지 못하는 gate는 다음 release에서
처음 실행된다. 이 세션에서 crash-loop 시나리오로 이미 겪은 일이다.

`cargo pkgid -p nimwaked | sed 's/.*[#@]//'`로 바꾸어 의존을 없애고 네 가지
태그로 실제 실행해 확인했다.

---

## PRD §50의 비교를 실제로 가능하게 만들었다

§50은 Android 검증이 별도 metric 체계를 만들지 않고 **CI Linux와 같은 workload로
실행해 비교하라**고 한다. 비교 항목은 다섯이다.

```text
proxy overhead p50 / p95                       nimwake-bench
cold start breakdown (spawn/readiness/proxy)   nimwake-bench
daemon RSS                                     nimwake-bench
CPU                                            없었다
concurrency knee point                         nimwake-bench
```

두 군데가 비어 있었다.

### `nimwake-bench`가 기기로 가지 않았다

aarch64 tarball에 담기는 것은 `nimwaked`와 fixture server들뿐이었다.
**지표를 만드는 도구가 빠져 있었다.** 기기에서 같은 workload를 돌릴 수 없으니
비교 자체가 불가능하다.

`release.yml`의 목록에 넣었다. bench는 `nimwaked` · `echo-server` ·
`crash-server`를 자기 옆에서 찾으므로 네 바이너리가 같은 디렉터리에 있어야 한다.

### CPU를 재지 않았다

`Resources`가 RSS와 FD만 담고 있었다. `/proc/<pid>/stat`의 utime·stime을 읽어
누적 CPU 시간을 추가했다. 리포트에는 절대 시간과 경과 대비 비율을 함께 적는다.
비교 대상이 CI runner와 저사양 기기이므로 절대 시간만으로는 읽기 어렵다.

```text
Sustained warm load (5s)    CPU 14.47s (289% of wall)
Crash loop (15s)            CPU 22.51s (150% of wall)
```

`/proc`의 시간 단위는 커널 `HZ`와 무관하게 `USER_HZ`(100)로 고정돼 있어 상수로 두었다.

---

## bind가 된다는 것과 connect가 거부된다는 것은 다르다

port 사전 검사(위)를 넣은 뒤 `process_runtime`이 15회 중 1~2회 깨졌다.

```text
`group` cannot start: 127.0.0.1:27137 is already in use by a process Nimwake does not own
```

`free_port`가 방금 `bind`에 성공한 port였다.

### 좁혀 간 과정

```text
단일 스레드 10회       실패 0      → 바이너리 내부 스레드 경쟁이다
ephemeral 범위 확인    32768~      → 테스트 범위(20000~30000)와 겹치지 않는다
50ms 간격 LISTEN 샘플  한 번도 없음 → 그런데 connect는 성공했다
```

**무엇이 응답했는지는 특정하지 못했다.** LISTEN 소켓은 없었다.

### 특정하지 못한 것을 추측하는 대신

`free_port`가 **쓰는 쪽과 같은 기준으로** 확인하게 했다. `bind` 성공만 보지 않고
`connect`가 거부되는지도 본다. `ProcessRuntime::start`가 그 기준을 쓰기 때문이다.

```text
고치기 전   process_runtime 15회 중 1~2회 실패
고친 후     process_runtime 15회 · 전체 4회 모두 통과 (134개)
```

이 할당기는 같은 종류로 **네 번째** 손을 봤다. 앞의 셋은 counter만 쓰던 때,
`HANDED_OUT`을 넣은 때, 예약 파일을 넣은 때다. 매번 "확인 방법이 쓰는 쪽의
기준과 달랐다"가 원인이었다.

---

## PRD §33의 "무한 spawn으로 이어지지 않는다"를 확인했다

§33은 crash-loop backoff를 Post-MVP로 미루면서 이렇게 적는다.

> MVP에서는 backoff 없이 재시도하되, 반복 실패가 무한 spawn으로 이어지지 않도록
> `startup_timeout_seconds`와 bounded waiter 상한에 의존한다.

backoff를 넣지 않았다. 넣는 것은 PRD를 다시 쓰는 일이다.
확인한 것은 **무엇을 근거로 안전하다고 말하는가**다.

`nimwake-bench`에 `crash_loop` 시나리오를 넣었다. `crash-server` 5개에 지속
요청을 보낸다.

```text
15초 · app 5개 · 연결 8개

requests (모두 502)        129,073
502가 아닌 응답                  0
동시 자식 최대 (<= 5)            2
정착 후 zombie                   0
부하 중 zombie 최대              2     판정하지 않는다
RSS 증가                    +1.8 MB
FD 증가                        -23
```

**누수는 없다.** 동시 자식이 application 수를 넘지 않고, 거두지 않은 process도
없고, RSS와 FD가 쌓이지 않는다. 결론만 보면 §33의 주장은 성립한다.

### 그런데 근거로 든 기제는 작동하지 않는다

spawn이 몇 번인지 따로 쟀다. `RUST_LOG=nimwake=debug`로 띄우면 spawn마다
`waiting for readiness`가 남는다.

```text
요청        10,414      구간 14.3초
spawn 시도   8,916      → 623 spawn/초
요청/spawn      1.2
```

`startup_timeout_seconds`는 **한 번도 관여하지 않는다.** `crash-server`는 1ms
안에 죽으므로 5초 상한 근처에도 가지 않는다.

bounded waiter 상한도 관여하지 않는다. 그것은 **대기 중인 요청 수**를 제한하지
spawn 속도를 제한하지 않는다.

기동 중복 제거(PRD §20)가 막아 줄 것으로 보았는데 그것도 아니다.
**요청/spawn이 1.2다.** 기동이 너무 빨리 실패해서 붙을 in-flight start가 거의
없다. 거의 모든 요청이 자기 spawn을 만든다.

실제로 spawn 속도를 정하는 것은 **들어오는 요청 속도**다. 그것은 경계가 아니다.

### 그래서 무엇이 문제인가

메모리가 아니라 **CPU와 배터리**다. 초당 수백 번의 process 생성은 Linux 서버에서는
묻히지만 Nimwake가 겨냥하는 기기에서는 다르다. 측정한 623 spawn/초는 client가
그만큼밖에 밀어넣지 못해서 나온 값이고, 더 빠른 client면 더 올라간다.

**이 수치가 crash-loop backoff를 Post-MVP에서 꺼낼 시점을 정하는 근거다.**
24h soak에서 앱이 OOM으로 죽기 시작하면 바로 이 경로를 탄다.

### zombie를 한 번 잘못 판정했다

처음에는 부하 중에 본 zombie의 최대값을 gate로 삼아 "2개 누수"로 release를
막았다. 자식이 종료한 뒤 부모가 `wait`로 거둘 때까지 짧게 zombie인 것이 정상이고,
20ms 간격으로 보면 그 창에 걸린다.

**판정은 부하가 멈추고 거둘 시간을 준 뒤에 한다.** 부하 중 최대값은 기록만 한다.

---

## readiness probe가 남의 listener를 자기 것으로 삼았다

동시 기동을 재려다 찾았다. 측정보다 이것이 중요하다.

```text
외부 listener pid=48466 (Nimwake가 띄우지 않은 echo-server)
gateway 요청   →  200, body=48466      남의 process가 응답
daemon의 자식  →  없음                  자기 process는 죽었다
자식의 stderr  →  "Address already in use"
readiness      →  173us에 통과
```

Nimwake가 **자기가 띄우지 않은 process로 트래픽을 보내고,** 자기 자식이 죽은
것을 알아채지 못했다.

### `biased` select로는 막히지 않는다

`ProcessRuntime::start`는 readiness와 조기 종료를 경쟁시키고 종료를 먼저 본다.
주석도 "process가 죽었다면 readiness는 성립할 수 없다"고 적고 있었다.
그 전제가 틀렸다.

```text
1. 자식을 spawn한다
2. 자식은 bind에 실패하지만 아직 살아 있다 (수 ms)
3. probe가 남의 listener에 붙어 성공한다     ← 정당한 순서다
4. app이 RUNNING이 되고 트래픽이 남의 process로 간다
```

죽기 전에 probe가 이기는 것이 경쟁의 올바른 결과다. 문제는 readiness가
**"port가 열렸나"만 묻고 "내 process가 열었나"를 묻지 않는 것**이다.

### spawn 전에 port가 비었는지 본다

고친 방식이다. 이미 열려 있으면 시작하지 않고 실패(502)로 끝낸다.

Nimwake는 process를 인수하지 않는다는 계약이 이미 있었다. (PRD §71)
코드가 그 계약을 지키지 않고 있었고, 이 검사가 둘을 맞춘다.

`a_port_held_by_a_foreign_process_is_not_adopted`로 고정했다. 사전 검사를
빼고 돌려 확인했다.

```text
고치기 전:  200, body="50245"   (외부 process의 pid)
고친 후:    502
```

### 이 상황은 실제로 일어난다

daemon을 SIGKILL하면 application이 남는다. destructor가 돌지 않으므로
`kill_on_drop`도 명시적 shutdown도 수행되지 않는다. 신호로 죽는 프로세스가
할 수 있는 일은 없다.

그 뒤 daemon을 다시 띄우면 port는 orphan의 것이다. 고치기 전에는 그 orphan에게
트래픽을 보냈다.

---

## 동시 기동 실측 (Next.js 8개)

```text
8개 동시 cold start  →  전부 200, 0.84~0.89초
앱당 RSS             →  96 MB  (합계 768 MB)
```

**직렬화되지 않는다.** 하나가 0.4초인데 8개가 0.87초다. 완전히 겹친다.
동시 기동을 제한하는 장치가 없다는 것이 수치로도 보인다.

2GB 기기면 이 무게로 10개 남짓에서 한계에 닿는다.

### 측정이 세 번 틀렸다

고쳐 가며 얻은 것이라 남긴다.

```text
1차   comm=="node"로 RSS를 셌다           Next.js는 comm을 바꾼다
2차   args에 standalone/server.js를 찾았다  args도 바뀐다
3차   awk 자신이 패턴에 걸렸다             daemon이 3개로 보였다
```

Next.js standalone은 `comm`과 `cmdline`을 **둘 다** `next-server (v15.5.27)`로
바꾼다. 원래 argv가 남지 않는다. 그래서 `pkill -x node`도 비켜 갔고 이전 실험의
앱 8개가 768MB를 쥔 채 살아남아 다음 측정을 오염시켰다.

세 번째는 `pgrep -f`에서 이미 겪은 것과 같다. **프로세스는 패턴이 아니라 pid로
확인한다.**

---

## soak은 Actions에서 돌리지 않는다

저장소가 private이므로 runner 시간이 과금된다. 비교하면

```text
release gate의 stress        148초
24시간 soak                  86400초   (583배)
```

GitHub-hosted runner의 job 상한이 6시간이라 PRD §65의 24h · 72h는 애초에
담기지도 않는다. 그래서 workflow로 두지 않고 [scripts/soak.sh](../../../scripts/soak.sh)에
두었다. 긴 구간은 수명이 긴 호스트의 일이고 Android 실기기 검증이 그 역할을 맡는다. (PRD §50)

release gate의 stress는 그대로 둔다. 148초이고 correctness gate의 본체다.
orphan · zombie · 누수를 보는 곳이 거기다. (PRD §44)

### 중단된 soak은 daemon을 남긴다

`nimwake-bench`는 SIGTERM에 destructor를 돌리지 않는다. 신호로 죽으면 그것이
띄운 daemon과 application이 남는다. 2시간 soak을 중단하면서 실제로 겪었다.

```text
bench를 kill        →  nimwaked와 echo-server가 남음
```

daemon 쪽 결함이 아니다. daemon은 부모가 정리해 주기를 기대하는 쪽이고
(`kill_on_drop`과 명시적 shutdown), 그 부모가 destructor 없이 죽은 것이다.

`scripts/soak.sh`는 bench를 `setsid`로 띄워 자기 process group의 leader로
만들고 trap에서 group 단위로 정리한다. `kill -- -$$`로는 안 된다. 비대화형
실행에서 스크립트는 group leader가 아니고, 그때 `-$$`는 존재하지 않는 pgid여서
kill이 조용히 실패한다.

### 검증하다 알게 된 것

배경 작업(`&`)으로 띄운 스크립트에는 SIGINT가 **무시로 설정된 채** 상속된다.
POSIX가 job control 없는 shell의 배경 process에 대해 그렇게 정한다.
진입 시 무시로 들어온 신호는 trap을 걸 수 없으므로 `trap cleanup INT`가
조용히 아무 일도 하지 않는다.

실제 사용(대화형 터미널에서 Ctrl-C)에서는 해당하지 않는다. 검증을 배경에서
돌리려 했던 쪽의 문제였고, trap 자체는 SIGTERM으로 확인했다.

---

## 실측: 프레임워크 앱 (Express · Next.js)

`hello-node`는 `http.createServer` 하나여서 런타임 비용만 보여 준다.
PRD §45의 "경량 API 2초 / Next.js 5초"가 현실적인지는 프레임워크로 재야 한다.

재현 절차: [scripts/measure_framework_cold_start.md](../../../scripts/measure_framework_cold_start.md)

```text
app          startup   cold TTFB      간극   warm TTFB       RSS
bare            52.7        56.9       4.2         0.9     47 MB
express        103.5       109.0       5.5         1.1     63 MB
next           205.8       396.0     190.2        15.4    104 → 118 MB
```

### PRD §45의 목표는 크게 여유가 있다

Next.js 기동이 **206ms**다. 목표 5초의 1/24이다.
`startup_timeout_seconds` 기본값 5초는 이 앱들에 충분하다.

**다만 이 숫자를 그대로 믿으면 안 된다.** 측정 대상은 한 페이지짜리 앱이고
`output: standalone`으로 빌드한 것이다. route가 많고 DB에 붙고 middleware가 있는
실제 서비스는 더 느리다. 여기서 나온 값은 **하한**이다.

### TCP readiness는 application readiness가 아니다

이쪽이 더 중요한 발견이다.

```text
next   기동(readiness) 206ms  →  첫 응답(TTFB) 396ms
       간극 190ms = 사용자가 겪는 cold start의 48%
```

bare와 express의 간극은 4~6ms로 무시할 수 있다. **Next.js만 다르다.**
port를 열고 나서도 첫 요청을 처리할 준비가 190ms 더 걸린다.

PRD §22는 readiness를 TCP connect 성공으로 정의한다. 그래서

| | |
| --- | --- |
| `readiness_latency` | 사용자가 겪는 cold start를 **과소 보고한다** |
| `startup_timeout_seconds` | readiness까지만 덮는다. 그 뒤로는 상한이 없다 |

후자가 실제 위험이다. port는 빨리 열고 응답은 느린 앱은 startup timeout에
걸리지 않는다. 그리고 **gateway에는 요청 단위 timeout이 없다.**
`forward`의 `connect` · `handshake` · `send_request` 어디에도 제한이 없고
hyper도 기본값을 두지 않는다.

처음에는 코드만 읽고 이렇게 적었다. 그 뒤 `mute-server` fixture로 재현했다.
`deaf-server`는 accept하고 **끊으므로** proxy가 실패해 502가 되지만, mute는
연결을 **붙잡는다.** 요청을 읽지도 응답하지도 끊지도 않는다.

```text
a_backend_that_never_answers_hangs_the_request
  → app은 RUNNING, 요청은 끝나지 않는다
```

port가 열려 있으므로 readiness probe도 startup timeout도 이것을 걸러내지
못한다. startup timeout이 readiness까지만 덮기 때문이다.

lease는 body가 drop될 때 회수되므로 daemon이 새지는 않는다. 기다리는 쪽이 문제다.

### 계약을 정했다 — PRD §75

처음에는 지금 동작을 고정만 해 두었다. PRD에 조항도 504도 없어서 새 계약을
코드에서 정하지 않았다. 그 뒤 사용자가 해소를 지시해 **PRD §75**를 신설했다.

상한은 **응답 header까지만** 적용한다.

```text
요청 전송 ──────── 응답 header 도착 ──────── body 전송 완료
└──────── 상한 적용 ────────┘└──── 적용하지 않는다 ────┘
```

세 안 중 이것을 고른 이유다. 전체 요청에 상한을 두면 어떤 기본값이든 정상
요청을 깨뜨린다. 큰 파일 내려받기와 천천히 흐르는 응답이 먼저 끊긴다.
막으려는 것은 **느린 응답이 아니라 오지 않는 응답**이다.

```text
[lifecycle] response_header_timeout_seconds = 60   기본값
504 Gateway Timeout                                응답이 오지 않음
502 Bad Gateway                                    backend를 쓸 수 없음
```

기본값을 60초로 넉넉히 잡았다. 긴 DB 조회나 무거운 SSR도 header가 늦다.

구현은 `proxy::forward`에서 connect · handshake · `send_request`를 한 번에
감싼다. `send_request`는 header가 도착하면 반환하고 body는 그 뒤에 흐르므로,
이 구간을 감싸는 것이 곧 "header까지"다.

테스트 둘로 고정했다.

```text
a_backend_that_never_answers_is_cut_off_with_504
a_slow_response_body_is_not_cut_off_by_the_header_timeout   ← 계약의 핵심
```

뒤쪽이 중요하다. `slow-body-server`는 header를 즉시 보내고 body를 2초에 걸쳐
흘린다. 상한을 1초로 두었으므로 body까지 상한에 들어간다면 응답이 잘린다.

앞쪽은 뮤테이션으로 확인했다. 상한을 무력화하면 31.8초에 실패한다
(client의 읽기 상한). 상한이 없던 동안의 실제 모습이다.

### 자원 경계가 `hello-node`로 본 것보다 가깝다

```text
hello-node    47 MB  →  5개 = 236 MB
Next.js      118 MB  →  5개 = 590 MB
```

2GB 기기에서 Next.js 앱은 10~14개 수준에서 한계에 닿는다.
그리고 동시 기동을 제한하는 장치가 없다 (자원 관리는 Post-MVP).

scale-to-zero가 이 문제에 답하려는 것은 맞지만, **동시에 깨어나는 수를 아무도
막지 않는다**는 사실은 그대로다.

---

## 실측: 실제 애플리케이션 (`examples/hello-node`)

`v0.0.5`까지의 모든 검증은 133줄짜리 Rust fixture 위에서 돌았다.
`examples/hello-node`는 저장소에 있었지만 **테스트·CI 어디에서도 실행되지 않았다.**

처음으로 실제 Node.js 앱을 Nimwake로 띄워 재 봤다.

### cold start

```text
              queue      spawn    readiness      total
hello-node    0.06ms     1.83ms    105.22ms    107.11ms
echo-server   0.04ms     0.31ms     52.26ms     52.60ms
```

**예상이 틀렸다.** "실제 앱이면 수 초"라고 봤지만 bare Node 앱은 107ms다.
node 런타임이 올라오는 비용은 fixture 대비 약 54ms뿐이다.

PRD §45의 "경량 API 2초 / Next.js 5초"는 **런타임이 아니라 프레임워크**의 비용이다.
`hello-node`는 `http.createServer` 하나여서 fixture와 같은 급이다.
프레임워크 앱은 여전히 측정되지 않았다.

### 메모리 — Nimwake는 작고, 앱이 크다

```text
daemon (앱 없음)        4.8 MB
daemon (5개 깨어 있음)   5.2 MB
hello-node 1개          48 MB
hello-node 5개         236 MB
```

PRD §45의 "idle daemon 100MB 이하" 목표에 20배 여유가 있다.
그리고 **자원의 거의 전부가 앱 쪽이다.** scale-to-zero가 답하려는 문제가 숫자로 확인된다.

동시에 공백도 드러난다. 앱 5개가 동시에 깨면 236MB다. 사소한 앱 5개가 그렇다.
2GB 기기에서 프레임워크 앱이라면 훨씬 적은 수에서 OOM이다.
**동시 기동을 제한하는 장치가 없다** (자원 관리는 Post-MVP).

### graceful shutdown

`hello-node`는 `SIGTERM` handler를 두고 스스로 나간다.
daemon 종료가 shutdown timeout(5초)을 쓰지 않고 **8ms**에 끝났고 잔존 프로세스는 0이었다.
fixture는 기본 signal 동작에 맡기므로 이 경로는 실제 앱으로만 확인된다.

### 남긴 것

E2E에 두 테스트를 넣었다. `node`가 없으면 건너뛰고, CI에는 `setup-node`를 넣어
반드시 돌게 했다. 예제가 다시 방치되지 않게 하려는 것이다.

---

## 구현 중 확정 (`v0.0.5`)

| #   | 결정                                             | 이유                                                                                                     | 반영 위치                                                |
| --- | ------------------------------------------------ | ---------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| D38 | idle timer는 app 하나에 하나만 둔다               | 요청마다 새 timer를 띄우면서 이전 것을 취소하지 않았다. 세대 검사는 **의미론적 안전**은 주지만 **메모리 상한**은 주지 않는다. 지속 부하 120초에 RSS가 57MB 늘었다. | [Invariant 9](../../prd/02-architecture/02-lifecycle-invariants.md), [§29](../../prd/02-architecture/07-scale-to-zero.md) |
| D39 | lifecycle stress를 여러 app에 나눠 돌린다          | 한 cycle의 길이는 idle timeout이고 그 하한은 1초다. 직렬이면 1000 cycle이 20분 가까이 걸린다. 반복 기동·종료의 누수를 보는 것이 목적이므로 나누어도 목적은 그대로다. | [§43](../../prd/03-delivery/04-stress-tests.md)          |
| D40 | aarch64는 `cross`로 빌드한다 (musl static)         | Docker만 있으면 되고 Rust 생태계의 표준 경로다. 네이티브 arm64 runner는 현재 요금제에서 쓸 수 있는지 불확실하고, `cargo-zigbuild`는 zig 버전을 따로 관리해야 한다. musl static이라 proot Ubuntu의 glibc 버전과 무관하다. | 이슈 #3, [§48](../../prd/04-platform/01-environments.md) |
| D41 | baseline은 `key=value` 한 줄씩                     | 이 파일을 읽는 쪽은 우리 자신뿐이다. 파서가 필요 없을 만큼 단순한 형태로 충분하다. 형식이 어긋난 줄은 건너뛴다 — 비교는 부가 정보이고, baseline 파일 하나 때문에 release가 멈추면 안 된다. | [§73](../../prd/03-delivery/07-performance-baseline.md)  |
| D42 | Android 검증을 `v0.1.0` 이후 → `v0.0.3` 이후 병렬로 당긴다 | proot 아래의 `/proc` · signal · process group, Termux background kill, Android sleep은 Linux CI로 재현되지 않는다. `v0.1.0` 이후에 발견하면 구조 변경이 되고, 그 전에 발견하면 수정이 된다. 검증은 `nimwake-android`가 하고, 결과는 Core `v0.1.0` 체크리스트로 들어온다. | [§50](../../prd/04-platform/02-android.md), [§65](../../prd/03-delivery/08-definition-of-done.md), 이슈 #4 |

### D38을 발견한 경위

sustained warm load를 처음 돌렸을 때 RSS가 30초에 4.8 → 19.3 MB로 올랐다.
직선이어서 평탄해지는지 보려고 120초로 늘렸더니 61.8 MB까지 계속 올랐다.

원인을 좁히는 데 세 번의 A/B가 필요했다.

| 가설 | 결과 |
| --- | --- |
| 요청 trace(`--trace-json`)가 원인 | 아니다. 끄니 오히려 더 늘었다 (28.5 vs 27.6 B/req) |
| glibc arena 보유 | 아니다. `MALLOC_ARENA_MAX=2`로도 그대로였다 |
| idle timer 누적 | **맞다.** idle timeout만 3600초 → 2초로 바꾸니 +29.8 MB → +3.8 MB |

마지막 A/B가 결정적이었다. **요청 수는 그대로 두고 timer의 수명만 바꿨다.**

### 왜 lifecycle stress는 이것을 못 잡았나

lifecycle stress는 idle timeout을 1초로 둔다. timer가 곧바로 만료되므로 쌓이지 않는다.
누수는 **기동 없이 지속되는 경로**에 있었고, 그것을 보는 것이 sustained warm load다.
PRD §43이 둘을 나눠 둔 이유가 여기서 드러났다.

### 코드 수준 주의사항 (추가)

- 테스트 port 할당에서 `seed + offset + attempt`에 나머지 연산을 한 번만 걸면 안 된다.
  `offset 0 · attempt 1`과 `offset 1 · attempt 0`이 **같은 port**를 낸다.
  첫 후보가 막힌 호출자가 옆 호출자의 port를 가져가고, `crash-server`의 readiness probe가
  다른 테스트의 fixture에 붙어 "기동에 실패해야 한다"는 단언이 간헐적으로 깨졌다.
  내준 port를 process 단위로 기록해 두어야 한다.

---

## Host interface (이슈 #5 · #6 · #7)

Core는 자기를 감독하는 host를 모른다. 그러나 supervisor는 `nimwaked`의 외부 동작에
사실상 의존한다. 암묵적 의존을 계약으로 바꾼 결정들이다. → [PRD §74](../../prd/04-platform/03-host-interface.md)

| #   | 결정                                           | 이유                                                                                          | 반영 위치                                                |
| --- | ---------------------------------------------- | ----------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| D34 | listen 주소를 설정 가능하게, 기본은 loopback    | `127.0.0.1` 고정 bind 때문에 같은 LAN의 다른 머신에서 접근할 수 없었다. Android A5(LAN lifecycle)가 여기서 막혔다. 열어 두는 것이 기본이 되어서는 안 되므로 **opt-in**으로 두고, 열렸을 때 경고를 남긴다. | [§74](../../prd/04-platform/03-host-interface.md), [§46](../../prd/04-platform/01-environments.md) |
| D35 | listen 주소는 이름 해석을 하지 않는다           | `host:port`만 받는다. DNS를 끌어들이면 daemon이 어디에 붙을지가 기동 시점의 네트워크에 의존하게 된다. | [§74](../../prd/04-platform/03-host-interface.md)        |
| D36 | 종료 코드로 실패 원인을 구분한다 (`sysexits`)   | supervisor는 재시도해도 나아지지 않는 실패에서 멈춰야 한다. 코드가 없으면 stderr 문자열 파싱이나 "기동 직후 연속 종료" 휴리스틱에 기대야 한다. `78` 설정 오류 / `69` bind 실패 / `1` 그 외. | [§74](../../prd/04-platform/03-host-interface.md)        |
| D37 | "미등록 Host 요청은 상태를 바꾸지 않는다"를 계약으로 | supervisor가 등록된 Host로 health probe하면 그 요청이 `acquire()`를 거쳐 app을 깨우고 idle timer를 리셋한다. **supervisor가 scale-to-zero를 무력화한다.** 전용 endpoint 대신 이미 성립하는 동작을 계약으로 고정하고 테스트로 못박았다. | [§74](../../prd/04-platform/03-host-interface.md), [Invariant 3](../../prd/02-architecture/02-lifecycle-invariants.md) |

### D37을 계약으로 택한 이유

이슈 #7은 두 가지를 제시했다.

```text
A. 미등록 Host 요청이 상태를 바꾸지 않음을 계약으로 고정
B. acquire()를 타지 않는 전용 경로 (/__nimwake/health)
```

A를 택했다. gateway는 `registry.resolve()`가 실패하면 `acquire()` 전에 응답하므로
**이미 성립하는 동작**이고, B는 예약 경로라는 새 표면을 만든다. Admin IPC와 전용
health endpoint는 Post-MVP 주제다. (PRD §60)

다만 "이미 성립한다"는 것만으로는 부족하다. 계약이라면 깨질 때 알아야 한다.
그래서 probe가 **잠든 app을 깨우지 않는 것**과 **깨어 있는 app의 idle timer를 리셋하지 않는 것**을
각각 E2E로 고정했다. 후자가 실제 위험이다 — 깨우지만 않으면 된다고 보면
supervisor가 app을 영원히 재우지 못하는 경로를 놓친다.

---

## 구현 중 확정 (`v0.0.4`)

| #   | 결정                                                       | 이유                                                                                                       | 반영 위치                                                    |
| --- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| D29 | 계측 값을 `u64`로 내보낸다                                  | `u128`을 쓰면 `tracing`이 JSON에 **문자열**로 적는다(`"queue_us":"142"`). 읽는 쪽이 숫자로 다룰 수 없다. 마이크로초 단위 `i64`는 29만 년을 담으므로 정밀도 손실이 없다. | [§72](../../prd/03-delivery/05-instrumentation.md)           |
| D30 | E2E를 `nimwaked` 패키지에 두고 fixture를 형제 경로에서 찾는다 | `CARGO_BIN_EXE_*`는 같은 패키지의 bin만 노출한다. daemon과 fixture를 동시에 쓰려면 한쪽은 경로로 찾아야 한다. workspace 바이너리는 모두 같은 디렉터리에 놓인다. | [§41](../../prd/03-delivery/02-testing-strategy.md)          |
| D31 | 부하 도구를 직접 만든다                                     | k6를 쓰면 CI에 외부 의존이 생기고, cold-start breakdown을 위해 daemon의 trace 출력과 맞물려야 한다. 그 결합을 외부 도구로 하면 더 복잡해진다. `v0.0.5` storm 테스트에서도 재사용한다. | [§42](../../prd/03-delivery/03-load-tests.md)                |
| D33 | 요청 구간 계측은 `debug`, 기동 구간은 `info` | 요청 event는 요청마다 나간다. `info`로 두면 운영 daemon이 요청당 로그 한 줄을 남기고, subscriber가 event마다 mutex를 잡는다. 측정값: 켜져 있으면 요청당 ~2.5µs, 꺼져 있으면 ~0.3ns. 기동은 드물고 운영자가 보고 싶어 하므로 `info`로 남긴다. | [§72](../../prd/03-delivery/05-instrumentation.md) |
| D32 | daemon CLI를 직접 파싱한다                                  | 옵션이 셋뿐이다. `clap`은 `nimwake` CLI가 생길 때 함께 들인다. (PRD §35의 스택 선택은 CLI 도구를 가리킨다)     | [§35](../../prd/02-architecture/09-repository-and-stack.md)  |

### 계측의 실제 비용

"로깅 수준이니 사소하다"고 넘기지 않고 쟀다. 요청 경로에 더해진 것은 두 가지다.

| 요청당 추가된 작업 | 비용 |
| --- | ---: |
| `Instant::now()` × 5 | ~0.1 µs |
| `tracing` event (subscriber 켜짐) | ~2.5 µs |
| `tracing` event (필터로 꺼짐) | ~0.3 ns |

warm path p50이 약 400µs이므로 event가 켜져 있어도 0.6% 수준이다.
그럼에도 기본에서 끈 이유는 성능보다 **요청마다 로그 한 줄이 쌓이는 것**과,
`tracing_subscriber::fmt`가 event마다 mutex를 잡아 동시성이 올라갈 때 경합 지점이 되는 것 때문이다.

end-to-end RPS로는 이 차이를 확인할 수 없었다. 측정 환경의 편차(22k~34k)가 차이보다 커서
추가된 작업만 격리해 재야 했다. **부하 테스트가 답할 수 없는 질문이 있다**는 점이 남길 만하다.

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
