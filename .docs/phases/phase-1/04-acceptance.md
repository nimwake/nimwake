# 완료 체크리스트

> **Phase 1 — Core MVP** › 완료 판단
>
> 판정 기준 원장: [PRD §65 Core MVP Definition of Done](../../prd/03-delivery/08-definition-of-done.md)

---

이 문서는 PRD §65의 항목을 **추적용 체크리스트**로 옮긴 것이다.

항목을 추가하거나 바꿔야 한다면 PRD §65를 먼저 고치고 여기를 맞춘다.

---

## 읽는 방법

각 항목에 **무엇이 그것을 보장하는지**를 함께 적는다.
체크만 있으면 "누가 언제 무슨 근거로 체크했는가"가 남지 않고,
남지 않으면 회귀가 생겼을 때 어디를 보아야 하는지 알 수 없다.

근거는 셋 중 하나다.

| 표기 | 의미 |
| --- | --- |
| `test` | 자동 테스트. 이름으로 찾을 수 있다 |
| `gate` | CI workflow 단계 |
| `report` | release artifact에 기록되는 측정값 |

> 수동 테스트에서 동작하는 것은 `v0.1.0`의 근거가 되지 않는다.
> → [v0.1.0 마일스톤](03-milestones/06-v0.1.0-core-mvp.md)

---

## 런타임

- [x] Rust daemon 실행 — `test` `the_primary_regression_scenario` (실제 바이너리를 띄운다)
- [x] manifest parsing — `test` `registry::tests::*`, `spec::tests::*`
- [x] manifest validation (unique host / unique port / `port != gateway port`) — `test` `registry_rejects_conflicting_manifests`, `a_port_conflict_refuses_to_start`
- [x] Runtime abstraction — `test` 전이 테스트 26개가 `Runtime` 없이 돈다. adapter는 `FakeRuntime` / `ProcessRuntime` 둘이고 lifecycle semantics는 같다
- [x] real child process spawn — `test` `start_waits_for_readiness_and_stop_removes_the_process`
- [x] `Child` 소유권이 `ProcessController` 한 곳에 있음 — `gate` `ci.yml`의 "Forbid Child ownership outside ProcessController"
- [x] stdout/stderr를 inherit하거나 drain (방치된 pipe 없음) — `test` `a_noisy_process_keeps_serving`, `a_noisy_app_keeps_serving`
- [x] process monitoring — `test` `a_process_that_dies_while_running_fails_then_restarts`, `wait_reports_the_exit_status_of_a_process_that_dies_on_its_own`
- [x] TCP readiness — `test` `start_fails_when_the_process_exits_before_readiness` (readiness가 성립하지 않으면 기동이 실패한다)
- [x] startup timeout — `test` `startup_timeout_moves_to_failed_and_leaves_no_process`, `an_app_that_never_becomes_ready_returns_502`

---

## 게이트웨이

- [x] Host routing — `test` `routing_is_driven_by_the_host_header`, `an_unknown_host_wakes_nothing`
- [x] HTTP reverse proxy — `test` `a_post_body_survives_a_cold_start` (body가 왕복한다)
- [x] cold start — `test` `request_wakes_the_app_serves_it_sleeps_and_wakes_again`
- [x] concurrent cold-start deduplication — `test` `thousand_concurrent_acquires_start_exactly_one_process`, `the_primary_regression_scenario` (100 동시 → unique pid 1)
- [x] `acquire()` 단일 API — backend 확보와 request 예약이 원자적 — `test` `sleeping_plus_acquire_enters_starting`, `running_plus_acquire_reuses_the_process` (한 전이 안에서 일어난다)
- [x] request tracking이 lease 발급/반납으로만 변경됨 — `test` `a_duplicate_request_finished_does_not_change_the_counter`, `a_lease_outlives_the_response_body`
- [x] mailbox / pending waiter 상한 초과 시 `503` — `test` `the_pending_waiter_limit_rejects_with_overloaded`, `requests_beyond_the_pending_limit_get_503`

---

## Scale-to-Zero

- [x] idle shutdown — `test` `an_idle_application_stops_its_process`, `idle_with_no_active_lease_enters_stopping`
- [x] stale timer protection — `test` `a_stale_idle_timer_is_ignored`, `an_early_idle_timer_does_not_stop_the_app`, `a_request_before_the_idle_deadline_keeps_the_app_alive`
- [x] graceful shutdown — `test` `graceful_shutdown_waits_for_the_shutdown_timeout`, `the_node_example_shuts_down_gracefully` (실제 앱이 `SIGTERM`에 스스로 나간다)
- [x] forced shutdown — `test` `sigterm_is_escalated_to_sigkill`, `stop_terminates_the_whole_process_group`
- [x] process crash detection — `test` `running_plus_process_exit_enters_failed`, `a_process_that_dies_while_running_fails_then_restarts`
- [x] retry after failure — `test` `failed_plus_acquire_enters_starting`, `an_app_that_crashes_is_restarted_by_the_next_request`

---

## PRD 조항별 근거

PRD를 코드와 한 절씩 대조한 결과다. **같은 일을 다시 하지 않기 위해 남긴다.**

`test`는 자동 테스트, `guard`는 `ci.yml`의 구조 검사, `gate`는 release를 막는
검사, `report`는 판정하지 않고 기록하는 수치, `구조`는 타입과 모듈 경계가
보장하는 것이다.

| 절 | 무엇으로 받쳐지나 |
| --- | --- |
| §9–§10 Core Architecture | `guard` gateway가 runtime에 닿지 못한다 (Invariant 3) |
| §11 상태 모델 | `test` 전이 테스트 41개 · 집합 의미를 쓰는 두 이유 모두 |
| §12 Invariant 1–10 | 아래 표 참조 |
| §18 Runtime Abstraction | `구조` lifecycle 판단이 `transition.rs` 한 곳에 있다 (아래) |
| §19 Process Runtime | `test` `a_noisy_process_keeps_serving` — drain을 빼면 실패한다 |
| §20 Process Isolation | `test` `the_child_gets_its_own_directory_and_an_overlaid_environment` |
| §21–§22 Manifest · Readiness | `test` 검증 항목 여덟 전부 · timeout 네 개를 하나씩 |
| §23 Gateway Routing | `test` Host 정규화(포트·대소문자) 세 곳 |
| §24 HTTP Contract | `test` `every_contract_method_passes_through` |
| §25 Request Tracking | `guard` + `구조` counter는 lease로만 움직인다 |
| §26 Acquire / Lease | `test` lease 수명과 body 수명이 묶여 있다 |
| §27–§28 Scale-to-Zero · Idle Timer | `test` 두 조건을 각각 지워 확인했다 |
| §29 Stale Timer | `test` · 세대 비교는 **주된 보호가 아니다** (설계 기록 참조) |
| §30 Graceful Shutdown | `test` SIGTERM → timeout → SIGKILL · process group |
| §31–§32 STOPPING 중 요청 · Crash | `test` waiter 이월 · 교체된 process의 event 무시 |
| §33 Failure Recovery | `test` `FAILED`에서 다음 `acquire`가 재시도한다 → §77 |
| §43 Stress | `gate` lifecycle stress · sustained · crash loop |
| §44 CI Strategy | `gate` 차단 기준 다섯 전부 (deadlock은 단계별 기한으로) |
| §50 Android 비교 | `report` `baseline.txt`가 비교 항목 다섯을 담는다 |
| §69 Backpressure | `guard` unbounded channel 금지 · `test` 상한 초과는 503 |
| §70 Process Ownership | `guard` `tokio::process`는 `controller.rs` 밖에서 금지 |
| §71 Daemon–Child Lifetime | `test` + `gate` daemon 종료 후 잔존 0 |
| §72 Latency Instrumentation | `test` 구간이 연속 부분구간인지 숫자로 단언한다 |
| §73 Performance Report | `report` release마다 baseline 누적 · 직전 대비 |
| §74 Host Interface | `test` listen 주소 · 종료 코드 · health probe |
| §75 Response Header Timeout | `test` 504로 끊고 **느린 body는 끊지 않는다** |
| §76 Manifest Reload | `test` + `gate` `SIGHUP` 후 app pid가 그대로다 |
| §77 Crash-Loop Backoff | `test` + `gate` 요청 8회에 기동 시도 3회. 창·지수·상한을 뮤테이션으로 확인 |
| §78 Memory Admission Control | `test` + `gate` 부족하면 `503`·spawn 0회. 꺼져 있으면 `/proc` 판독 0회 |
| §79 Status Snapshot | `test` + `gate` `SIGUSR1` → 버전과 app별 상태. OS pid가 app이 알린 것과 일치 |

### §18은 테스트가 아니라 구조가 보장한다

"adapter가 바뀌어도 lifecycle semantics는 변경되지 않아야 한다"를 두 runtime의
테스트를 나란히 두는 방식으로 증명하지 않는다. 그렇게 하면 두 벌의 테스트가
비슷해 보이는지만 사람이 눈으로 비교하게 된다.

**lifecycle 판단이 `transition.rs` 한 곳에 있다.** 그 함수는 runtime을 모른다.
`Runtime`이 돌려줄 수 있는 것은 성공·실패·종료 상태뿐이고, 그것들은 모두 같은
전이 함수로 들어간다. adapter가 semantics를 바꿀 방법이 없다.

Invariant 2 guard가 이것을 지킨다 — 전이를 두 번째 장소에서 적용하면 CI가 막는다.

## 동시성 불변식

### PRD §12 Invariant 1–10

DoD 목록(아래)은 §12의 불변식과 1:1이 아니다. 어느 불변식이 무엇으로 지켜지는지
따로 적는다. `guard`는 `ci.yml`의 구조 검사다 — 테스트로 잡히지 않는 종류를
막는다.

| | | |
| --- | --- | --- |
| 1 | Single Process | `test` `thousand_concurrent_acquires_start_exactly_one_process`, `running_plus_acquire_reuses_the_process`, `concurrent_acquires_on_a_running_app_reuse_the_process` · `report` cold-start storm의 `unique_pids` |
| 2 | Single Lifecycle Authority | `guard` Forbid a second lifecycle authority — `transition::`을 `manager.rs` 밖에서 적용하면 실패한다. `AppState`는 읽기 접근자만 노출한다 |
| 3 | Gateway Isolation | `guard` Forbid the gateway from reaching into the lifecycle — gateway가 runtime이나 `AppManager`를 import하면 실패한다 |
| 4 | Cold Start Deduplication | `test` `thousand_concurrent_acquires_start_exactly_one_process` · `gate` storm이 2개 이상 띄우면 release를 막는다 |
| 5 | Active Request Protection | `test` `active_lease_prevents_idle_shutdown`, `idle_with_an_active_lease_stays_running`, `a_lease_outlives_the_response_body` |
| 6 | Non-Blocking Actor | `guard` Keep the decision path synchronous — `transition.rs`에 `async fn`이나 `.await`가 들어오면 실패한다. 동기 순수 함수는 기다릴 수 없고, 기동·종료·monitor는 `Effect`로 나간다 |
| 7 | Acquire Atomicity | `test` `sleeping_plus_acquire_enters_starting`, `returning_the_last_lease_arms_the_idle_timer`, `returning_one_of_many_leases_does_not_arm_the_timer` — 상태 전이와 lease 발급이 한 step에서 나온다 |
| 8 | Stale Event Rejection | `test` `a_stale_start_completed_is_ignored`, `a_stale_stop_completed_is_ignored`, `a_stale_process_exit_is_ignored`, `a_stale_start_completed_stops_the_orphan_process` |
| 9 | Bounded Queues | `guard` Forbid unbounded queues · `test` `with_pending_limit`을 쓰는 overload 테스트 |
| 10 | Daemon-Child Lifetime | `test` `no_child_survives_the_daemon`, `registry_shutdown_leaves_no_managed_child` · `gate` `release.yml`의 "Assert nothing survived" |

### Core MVP DoD 항목


- [x] no duplicate processes — `test` `thousand_concurrent_acquires_start_exactly_one_process` · `report` cold-start storm의 `spawn count`
- [x] no lost startup waiter — `test` `startup_failure_rejects_every_waiter`, `stopping_plus_acquire_restarts_after_the_stop_completes`, `teardown_rejects_pending_waiters`
- [x] no lifecycle deadlock — `gate` 긴 단계마다 `timeout-minutes`를 두어 멈추면 **어느 단계인지와 함께** 실패한다 (release gate: Build 20 · Tests 20 · Stress 25분) · `report` lifecycle stress의 `failed requests`
- [x] no orphan process after stress test — `report` lifecycle stress의 `orphan processes` / `zombie processes` (release 실측: 둘 다 0)
- [x] stale `StartCompleted` / `StopCompleted`가 state를 변경하지 않음 — `test` `a_stale_start_completed_is_ignored`, `a_stale_stop_completed_is_ignored`, `a_stale_start_completed_stops_the_orphan_process`
- [x] 교체된 process의 `ProcessExited`가 state를 변경하지 않음 — `test` `a_stale_process_exit_is_ignored`
- [x] daemon 종료 후 managed child process 잔존 0 — `test` `no_child_survives_the_daemon`, `registry_shutdown_leaves_no_managed_child` · `gate` `release.yml`의 "Assert nothing survived"

---

## 테스트

- [x] unit tests pass — `gate` `ci.yml` / `release.yml`의 `cargo test --workspace`
- [x] concurrency tests pass — `gate` 위와 같음 (`tests/concurrency.rs`)
- [x] E2E tests pass — `gate` 위와 같음 (`crates/nimwaked/tests/e2e.rs`, 실제 바이너리)
- [x] cold-start storm passes — `gate` `release.yml`의 "Stress and leak checks" · `report` `spawn count == 1`
- [x] lifecycle stress passes — `gate` 위와 같음 · `report` 1000 cycles / orphan 0 / zombie 0
- [x] release CI passes — `gate` `v0.1.0-rc1` 태그에서 세 job이 모두 통과했다
      (`qualify` / `aarch64` / `publish`). `publish`는 이 rc에서 처음 실행됐다

---

## 계측

- [x] cold start가 구간별 breakdown으로 기록됨 — `report` actor queue / process spawn / app readiness / total · `test` `benchmark_mode_emits_span_measurements`
- [x] warm path proxy overhead가 direct 대비로 기록됨 — `report` Warm path 표의 `overhead` 열
- [x] concurrency sweep 결과가 기록됨 — `report` Concurrency sweep 표 + knee point
- [x] performance report artifact가 release마다 생성됨 — `gate` `release.yml`의 "Upload the report" (`performance-report.md`, `baseline.txt`)

---

## Android Device

[`nimwake-android` Phase 1](https://github.com/nimwake/nimwake-android/tree/main/.docs/phases/phase-1)의 검증 리포트로 판정한다.

```text
Install ✓   Boot recovery ✓   Cold start ✓   Scale-to-zero ✓
Daemon recovery ✓   Network recovery ✓   24h soak ✓   72h soak ✓
```

**이 절은 Core가 체크할 수 없다.** 입력이 다른 저장소의 리포트다.
Core가 할 일은 aarch64 바이너리를 공급하는 것까지이고, 그것은 `release.yml`에 있다.

- [ ] Install
- [ ] Boot recovery
- [ ] Cold start
- [ ] Scale-to-zero
- [ ] Daemon recovery
- [ ] Network recovery
- [ ] 24h soak
- [ ] 72h soak

---

## 남은 것

```text
Android        8항목        nimwake-android 리포트 대기
```

**Core 쪽은 전부 끝났다.** 구현도, release gate도 실제 태그로 검증됐다.

`v0.1.0-rc1`에서 확인한 것:

```text
release        prerelease로 표시됨. /releases/latest가 404 →
               다음 정식 release의 baseline 비교가 rc를 집어 가지 않는다
assets         nimwake-aarch64-linux-musl.tar.gz (2.67 MB)
               performance-report.md · baseline.txt
release notes  제약이 먼저(1~56줄), 성능 리포트가 뒤(57줄부터)
baseline       label=v0.1.0-rc1 + 지표 6개. 다음 release가 이것과 비교한다
```

남은 것은 다른 저장소의 입력이다. `v0.1.0` 태깅은 그 리포트를 기다린다.

---

## §50 CI Linux ↔ Android 실기기 비교 (v0.1.0-rc5)

같은 `nimwake-bench`를 같은 workload로 CI runner와 실기기에서 돌려 비교한다.
**다섯 항목을 모두 채웠다.**

| 지표 | CI Linux | Android 기기 | 배수 |
| --- | ---: | ---: | ---: |
| proxy overhead p50 | 0.938 ms | 5.907 ms | 6.3× |
| proxy overhead p95 | 1.974 ms | 13.277 ms | 6.7× |
| cold start queue | 0.037 ms | 0.737 ms | 19.9× |
| cold start spawn | 0.169 ms | **5.178 ms** | **30.6×** |
| cold start readiness | 0.054 ms | 0.863 ms | 16.0× |
| cold start total | 1.129 ms | 8.898 ms | 7.9× |
| daemon RSS | 5.60 MB | **2.11 MB** | 0.4× |
| CPU (wall 대비) | 100.1 % | **8.7 %** | 0.1× |
| concurrency knee | 50 (7,188 rps) | **1 (129 rps)** | — |
| knee p95 | 8.75 ms | 11.39 ms | 1.3× |

cold start 분해는 표본 1,000개(CI) · 993개(기기)다.

### overhead는 농도가 다르다

```text
CI     8 동시 연결 (무릎 50 아래)
기기   1 동시 연결 (무릎이 1이다)
```

**같은 농도로 잴 수 없다.** 기기의 무릎이 1이므로 그 위에서 재면 `echo-server`
fixture가 먼저 막혀 비교 기준이 병목이 된다 — rc4에서 8로 재니 overhead가
`-13.4ms`로 나왔다. 두 수치는 각자의 농도에서 유효하고 배수는 참고값이다.

### 먼저 무너지는 것은 process spawn이다

```text
         queue    spawn     readiness   나머지
CI        3.3%    15.0%      4.8%       77.0%
기기      8.3%    58.2%      9.7%       23.8%
```

기기에서 cold start의 **58%가 process spawn**이다. CI에서는 15%다.
proot(ptrace)이 fork/exec를 감싸는 비용이 여기 드러난다.

절대 배수로는 queue가 19.9배, spawn이 30.6배다. **spawn은 배수도 비중도 가장 크다.**

### daemon은 기기에서 병목이 아니다

RSS가 CI의 0.4배, CPU가 0.1배다. **적게 쓰는 것이 좋은 소식이 아니다** — 처리량이
다른 곳에서 막혀 daemon이 할 일이 없다는 뜻이다. 무릎이 동시성 1이고 129 rps다.

막는 것은 proot(ptrace) 비용과 커널 `somaxconn 128`이다. Nimwake 고유 구간이
아니다. → `nimwake-android` `.docs/validation/08`

### lifecycle stress는 기기에서 깨끗하다

rc4에서 1000 사이클 × 10 app을 218초에 돌렸다. spawn 1000 · 실패 0 · 고아 0 ·
좀비 0 · 열린 port 0 · FD 12 → 12. **깨우고 재우기를 1000번 반복해도 새는 것이
없다.** rc5는 런타임이 같다.

### proxy connect는 중첩된 다른 표에 있다

§50이 cold start 분해를 "spawn / readiness / **proxy connect**"로 적었다.
앞의 둘은 `nimwake::startup`에 있고 proxy connect는 `nimwake::request`에 있다.

**한 표에 섞으면 거짓 분해가 된다.** 두 trace는 중첩 관계다.

```text
ttfb
├── routing          gateway 자체 처리
├── acquire          깨우기 — cold start면 기동 분해가 이 안에 있다
├── proxy connect    backend와 TCP 연결 수립까지
└── backend          app이 응답 header를 만든 시간
```

bench가 이제 요청 trace도 모아 **따로 표를 낸다.** 중첩을 수치로 확인했다 —
로컬에서 `acquire` p50 1.09ms가 기동 `total` 1.02ms를 담고, 차이 0.07ms가 actor가
기동을 결정하기 전의 mailbox 대기다.

```text
routing          0.00 ms
acquire          1.09 ms   ⊇ 기동 total 1.02 ms
proxy connect    0.03 ms
backend          0.08 ms
ttfb             1.24 ms
```

구간마다 따로 낸 백분위이므로 **더해서 ttfb가 되지 않는다.** 같은 요청의 값이 아니다.

baseline에 `request_routing_ms` · `request_acquire_ms` ·
`request_proxy_connect_ms` · `request_backend_ms` · `request_ttfb_ms`를 남긴다.

CI ↔ 기기 비교는 **다음 릴리스에서** 채운다. 기기는 rc5로 72h soak 중이고 bench를
다시 돌릴 수 없다.

---

## Android 실기기 장시간 측정 (참고)

판정은 `nimwake-android`의 리포트가 한다. 여기에는 **Core가 그 수치를 받아
읽을 수 있도록** 기록만 남긴다. PRD §51의 관찰 항목에 대응한다.

```text
기기        Samsung Galaxy A51 5G · Android 13 · Termux + proot Ubuntu 26.04
Core        d7c5590 (`#28` 시점의 main, aarch64-unknown-linux-musl)
            **rc3보다 오래된 build다.** 이후 main에 §76·§77·§78이 들어왔다
workload    5분마다 요청 하나. 매번 잠든 앱을 깨운다
구간        2026-10-09 13:44:06 ~ 10-12 13:42:23 (**72.0시간, 표본 862개, 완주**)
```

| §51 관찰 항목 | 결과 |
| --- | --- |
| daemon survival | **pid 12821 하나.** 862 표본 전부 같은 pid — 재시작 0회 |
| Android background killing | 관측되지 않았다. **최대 표본 간격 302초**(주기 300초)이므로 샘플러도 Termux도 멈춘 적이 없다 |
| memory usage | RSS 2104 → 1308 KB (max 2332). **줄었다.** 기울기 -9.07 KB/h |
| battery usage | 100% → 79%. 시간당 0.29% |
| temperature | 26.3~30.0 °C. 추세 없음 |
| wake reliability | 요청 실패 **0건 / 862건.** `apps_before`가 862회 모두 0이므로 매번 진짜 cold start다 |
| network recovery | 이 구간에서 끊김이 관측되지 않았다 (별도 시나리오는 A5) |
| orphan process | 요청 직전 깨어 있는 app **0 표본.** 쌓이지 않았다 |
| storage growth | free 99608 → 98404 MB. 72시간에 1.204GB. **nimwake 몫은 1.3MB뿐이다** — `.nimwake-host/logs`가 1356KB이고 `nimwaked.log`가 1.19MB다. 나머지는 OS와 다른 앱이다 |
| repeated lifecycle stability | cold start p50 47 · p95 56 · max 133 ms |

처음 24시간만 따로 보아도 같다 — 표본 288개, 실패 0, 재시작 0, RSS 2104 → 1240 KB.

### 로그의 39.3%가 ANSI escape였다

이 soak이 드러낸 결함이다. `nimwaked.log` 1.19MB 중 **492KB가 색상 코드**였다.
`tracing_subscriber::fmt()`의 기본값이 색 켬이고, supervisor가 stdout을 파일로
리다이렉트하는데 터미널 여부를 보지 않았다.

같은 workload를 20사이클 돌려 쟀다.

```text
고치기 전       23,542 bytes
NO_COLOR=1      14,295 bytes
고친 뒤         14,299 bytes   ← is_terminal() 판정
```

`with_ansi(std::io::stdout().is_terminal())`로 고쳤고, e2e가 pipe로 나가는 로그에
escape가 없음을 단언한다. **그 테스트가 있었다면 72시간을 기다리지 않고 잡혔다.**

### RSS가 줄었다

누수를 찾으러 본 지표인데 반대로 나왔다.

```text
 0–6h    평균 2211 KB
 6–12h   평균 1659 KB
12–18h   평균 1239 KB
18–50h   평균 1234~1247 KB   (평평하다)
```

12시간쯤에 자리를 잡고 38시간 동안 변하지 않는다. allocator arena가 정리되거나
Android의 page reclaim이 작용한 것으로 보이며, 어느 쪽이든 **증가가 없다**는 것이
이 측정의 결론이다.

### soak 수치를 §50 비교에 쓰지 않는다

soak CSV에는 **구간 분해가 없다.** `seconds`는 HTTP 응답 시간이고 CI의
`cold_start_total_ms`는 기동 구간(queue+spawn+readiness)이다. **다른 것을 재는
값이므로 나란히 놓으면 안 된다.**

실제로 soak의 cold start p50은 47ms이고 rc4 bench의 기동 구간 total은 10.7ms다.
6.7배 차이가 "기기가 빠르다"는 뜻이 아니라 **재는 대상이 다르다**는 뜻이다.
나머지 36ms는 curl 기동 · TCP connect · proxy connect · app 응답이다.

§50 비교는 위의 rc4 절에 있다.

---

## 이 체크리스트가 보장하지 않는 것

전 항목이 체크되어도 아래는 **측정되지 않은 상태**다.
`v0.1.0`은 "Core lifecycle이 자동 테스트를 통과한 상태"이고 제품의 가용성이 아니다.

| 공백 | 지금 아는 것 |
| --- | --- |
| 실제 서비스의 cold start | Next.js **기동 206ms**, 첫 응답 396ms. 목표 5초에 여유가 크다. 다만 한 페이지짜리 앱이므로 하한이다 |
| readiness의 의미 | TCP connect는 application readiness가 아니다. Next.js는 port를 열고도 190ms 더 걸린다 |
| 동시 기동의 자원 한계 | Next.js 앱 8개를 동시에 깨우면 직렬화 없이 전부 0.87초에 뜬다. 앱당 96MB로 합계 768MB. **입장 제어가 생겼다**(PRD §78, `--min-free-memory-mb`) — 하나씩 쌓이는 경우는 막는다. **동시에 깨어나는 경우는 막지 못한다**: app의 메모리 비용을 기동 전에 알 수 없다. eviction(§54)은 여전히 Post-MVP |
| 장시간 운용 | CI에서의 최장 측정은 105초(stress) / 120초(sustained)다. **실기기에서 72.0시간을 완주했다** — daemon 재시작 0회, 요청 실패 0/862, RSS 2104→1308KB(감소), 최대 표본 간격 302초(주기 300초). **단 build가 d7c5590이고 rc3보다 오래됐다** — 이후 §76·§77·§78이 main에 들어왔고 그 코드는 기기에서 돌지 않았다. 판정은 `nimwake-android` 소관 |
| crash-loop | backoff가 있다 (PRD §77). 같은 기계에서 before/after를 쟀다 — 20초 부하에 **기동 시도 3,653·4,728회 → 14·14회**(초당 183~236 → 0.7). 14는 설계가 예측한 값과 같다(앱당 즉시 3회 + 1·2·4·8·16초 지연 = 7회, 앱 2개). 누수는 전부터 없었다 — 동시 자식 ≤ app 수, 정착 후 zombie 0, FD 증가 없음 |
| manifest 재적재 | `SIGHUP`으로 된다 (PRD §76). 바뀌지 않은 application은 건드리지 않는다. 검증 실패와 빈 집합은 거부하고 기존 상태를 유지한다 |
| 운영 도구 | `SIGUSR1`로 상태를 로그에 찍을 수 있다 (PRD §79) — 버전·state·OS pid·active·pending. `nimwake status` CLI와 admin IPC는 §56 Post-MVP다 |

실측 근거는 [05. 설계 결정 기록](05-design-decisions.md)의 "실측: 실제 애플리케이션"에 있다.

---

전 항목이 충족되면 다음으로 태깅한다.

```text
Nimwake v0.1.0 — Core MVP
```

---

← [v0.1.0 Core MVP](03-milestones/06-v0.1.0-core-mvp.md) · [페이즈 개요](README.md) · [설계 결정 기록](05-design-decisions.md) →
