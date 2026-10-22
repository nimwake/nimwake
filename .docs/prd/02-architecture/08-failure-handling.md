# Failure Handling

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §31–§33, §71, §77

---

## 31. Request During STOPPING

STOPPING 상태에서 새 `acquire`가 발생할 수 있다.

MVP에서는 **shutdown을 취소하지 않는다.**

```text
STOPPING
   │
   │ acquire
   ▼
waiter로 등록
   │
   ▼
stop complete
   │
   ▼
STARTING
   │
   ▼
RUNNING
   │
   ▼
lease 발급
```

이미 SIGTERM을 받은 process를 다시 정상 상태로 되돌리려 하지 않는다.

이 정책은 process semantics를 결정적으로 유지하기 위한 것이다.
SIGTERM 이후의 취소 의미는 application마다 달라지므로, 되돌리기를 허용하면 상태 머신이 크게 복잡해진다.

`Stopping` state는 waiter를 직접 소유하므로, stop 완료 시 waiter를 그대로 다음 `Starting`으로 넘긴다.
→ [§11 상태 모델](01-core-architecture.md)

---

## 32. Process Crash

application process는 예상치 못하게 종료될 수 있다.

`ProcessController`가 child의 exit를 감지해 event로 보고한다.

```text
Application Process
       │
       ▼
 ProcessController
       │
       ▼
 ProcessExited { process_id, status }
       │
       ▼
  AppManager
```

`ProcessExited`는 반드시 `process_id`를 동반한다.

```rust
if running.process_id != process_id {
    // 이미 교체된 process의 exit
    return;
}
```

identity가 일치하는 RUNNING process가 비정상 종료하면:

```text
RUNNING
   ↓
FAILED
```

로 전환하고, 남아 있는 lease는 모두 무효화한다.

STOPPING 중의 `ProcessExited`는 정상 종료 경로이며 `SLEEPING`으로 전이한다.

---

## 33. Failure Recovery

FAILED는 permanent terminal state가 아니다.

다음 `acquire`에서 다시 startup을 시도할 수 있다.

```text
FAILED
   │
acquire
   ▼
STARTING
```

crash-loop backoff의 계약은 [§77](#77-crash-loop-backoff)에 있다.

예:

```text
3 failures / 60 sec
        ↓
     BACKOFF
```

`startup_timeout_seconds`와 bounded waiter 상한은 한 번의 기동에만 걸린다.
즉시 실패하는 명령에는 상한이 되지 않으므로 §77이 창과 지연을 따로 정한다.

---

## 77. Crash-Loop Backoff

§33은 backoff를 Post-MVP로 미루면서 모양만 예로 보였다. 이 절이 그 계약이다.

### 무엇이 문제였나

backoff가 없으면 **계속 실패하는 application은 요청마다 기동을 재시도한다.**

`startup_timeout_seconds`는 **한 번의 기동에만** 걸리므로 즉시 실패하는 명령
(`command`가 틀린 경우)에는 아무 상한이 되지 않는다.

### 얼마나 빠른가 — 전에 적은 "상한"은 상한이 아니었다

이 절은 처음에 "측정된 상한은 초당 623회 spawn"이라고 적었다. **그 수치의 출처가
그것을 상한이라고 하지 않았다.** phase-1의 측정 기록은 이렇게 적어 두었다.

```text
측정한 623 spawn/초는 client가 그만큼밖에 밀어넣지 못해서 나온 값이고,
더 빠른 client면 더 올라간다.
```

하한을 상한으로 옮겨 적은 것이다. 다시 쟀다 — backoff를 끈 빌드로, `command`가
없는 파일을 가리키는 app에 3초씩 요청을 퍼부었다.

```text
개발 기계(WSL, x86_64)      spawn/초          요청/spawn
  동시 1                 1,550 ~ 1,890  (2회)     1.0
  동시 8                 1,590 ~ 1,644  (3회)     1.5
  동시 32                1,507 ~ 1,699  (3회)     1.6
```

**동시 1에서는 요청 수와 spawn 수가 정확히 같다** — 그때 재는 것은 Core가 아니라
클라이언트다. 623도 그런 값이었다.

동시 연결을 8배로 늘리면 요청은 늘어나는데 **spawn은 늘지 않는다.** 남는 요청이
같은 기동 하나에 합쳐진다(§20의 기동 중복 제거). 그러니 개발 기계에서 spawn
속도는 초당 1,500~1,900 어딘가에서 평평해진다.

재는 방법은 `scripts/measure-spawn-rate.py`에 두었다. backoff를 끈 빌드가
필요하다는 것까지 적어 두었으니 기기에서 그대로 돌릴 수 있다.

이것이 확정된 상한이라고는 하지 않는다 — 더 빠른 클라이언트로 더 밀어야 한다.
**분명한 것은 자릿수다: 네 자리다.** 기기(전화기)에서는 훨씬 낮을 것이고 거기서
다시 재야 한다. 그래도 초당 수천 번의 process 생성은 전화기의 배터리와 CPU를
태우고 로그를 채운다.

### 언제 crash loop로 판정하나

```text
60초 안에 실패 3회  →  backoff
```

창(window)으로 본다. **연속 실패 횟수로 보지 않는다.** 하루를 정상으로 돌다가
한 번 죽은 application은 crash loop가 아니다. 창은 그 둘을 구분한다.

창을 쓰면 초기화 조건을 따로 정하지 않아도 된다. 실패가 드물어지면 창에서
저절로 빠진다. 1분에 한 번 실패하는 application의 spawn 속도는 이미 무해하다.

### 얼마나 기다리나

```text
실패 1, 2회     즉시 재시도한다
실패 3회        1초
실패 4회        2초
실패 5회        4초
...             2배씩
상한            30초
```

1, 2회를 즉시 재시도하는 것이 중요하다. port가 잠깐 잡혀 있던 것 같은 일시적
실패까지 기다리게 만들면 **다음 요청이 앱을 깨운다는 약속이 깨진다.**

상한이 30초인 이유는 창이 60초이기 때문이다. 30초 간격이면 최근 실패 3회가
60초 안에 들어오므로 backoff가 유지된다. 상한을 창보다 크게 두면 backoff에서
빠졌다 들어오기를 반복한다.

최악의 spawn 속도는 **30초당 1회**다. 위에서 잰 초당 수천 회에서 내려온다 —
네 자리에서 한 자리로 간다.

### 설정 항목을 만들지 않는다

네 값은 코드 상수다. `[lifecycle]`에 넣지 않는다. 조정이 필요하다는 근거가
아직 없고, manifest 항목은 한 번 내보내면 계약이 된다.

### 실패 이력은 어디에 있나

```text
STARTING    싣는다
RUNNING     싣는다
FAILED      싣는다
SLEEPING    싣지 않는다 — 초기화된다
STOPPING    싣지 않는다
```

`RUNNING`이 싣는 이유가 있다. 기동은 성공하고 첫 요청에 죽는 application도
crash loop다. §32의 비정상 종료 경로도 실패로 센다.

`SLEEPING`에 도달하려면 **정상으로 돌다가 idle로 멈춰야 한다.** 그것은
application이 동작한다는 가장 분명한 증거이므로 거기서 초기화하는 것은
정보 손실이 아니라 의도다.

### backoff 중의 요청

```text
503 Service Unavailable
Retry-After: <남은 초>
```

`502`가 아니다. `502`는 기동을 시도했고 실패했다는 뜻이다. backoff 중에는
**시도하지 않는다.**

overload도 `503`이지만(§69) `Retry-After`를 붙이지 않는다. 그것이 둘을 가르는
신호다. overload는 바로 다시 보내면 될 수 있고, backoff는 남은 시간 동안
보내도 소용이 없다.

### 재적재는 backoff를 지운다

§76이 spec이 바뀐 application의 actor를 교체하므로 이력도 함께 사라진다.
**그것이 의도다.** 실사용에서 가장 흔한 순서다 — 앱이 crash loop에 빠지고,
운영자가 원인을 고치고, `SIGHUP`을 보낸다. 거기서 30초를 더 기다리게 만들면
고친 보람이 없다.

### backoff는 상태가 아니다

`FAILED`에 머문다. 여섯 번째 상태를 만들지 않는다. backoff는 "`FAILED`에서
언제 재시도할 수 있는가"일 뿐이고, §11의 상태 모델과 `is_settled`(Invariant 10)는
그대로 둔다.

→ [§33 Failure Recovery](#33-failure-recovery) · [§32 Process Crash](#32-process-crash) ·
[§69 Backpressure](06-gateway-and-requests.md)

---

## 71. Daemon–Child Lifetime

`nimwaked`는 상태를 디스크에 저장하지 않는다. (SQLite persistence는 Post-MVP)

따라서 daemon이 재시작되면 in-memory state는 전부 `SLEEPING`에서 다시 시작한다.

문제는 **이전 daemon이 남긴 child process가 살아 있을 수 있다는 것**이다.

```text
nimwaked
   │
   └── node server.js

nimwaked crash

node server.js 생존

nimwaked restart
   ↓
state = SLEEPING
   ↓
new acquire
   ↓
new node process
   ↓
port already occupied     ← 기동 실패
```

### 정책

MVP는 process adoption(기존 process 재발견·인수)을 구현하지 않는다.

대신 다음을 요구사항으로 고정한다.

> **Nimwake가 생성한 child process의 수명은 daemon의 수명에 종속된다.**

구현 방향:

```text
spawn
  ↓
전용 process group 생성
  ↓
daemon graceful shutdown
  ↓
process group 전체 정리
```

- Linux에서는 process group을 사용하고, daemon 종료 시 group 단위로 종료 신호를 보낸다.
- 비정상 종료(SIGKILL 등)까지 완벽히 막을 수는 없으므로, parent-death 계열 수단을 가능한 범위에서 함께 적용한다.
- Android / proot 환경의 실제 semantics는 `v0.1.0` 이후 E2E로 확인한다.
  → [§50 Android Validation](../04-platform/02-android.md)

### v0.1.0 명시 사항

```text
daemon restart 후 기존 application process를 재발견하거나 adopt하지 않는다.
```

이 제약은 문서화된 계약이며, 상태 지속성은 Phase 2의 SQLite 도입과 함께 재검토한다.
→ [§59 Database Strategy](../05-beyond-mvp/02-platform-features.md)

---

← [Scale-to-Zero와 Graceful Shutdown](../02-architecture/07-scale-to-zero.md) · [문서 인덱스](../README.md) · [Repository Architecture와 Technology Stack](../02-architecture/09-repository-and-stack.md) →
