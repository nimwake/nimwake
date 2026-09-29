# Failure Handling

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §31–§33, §71

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

crash-loop backoff는 Post-MVP에서 추가한다.

예:

```text
3 failures / 60 sec
        ↓
     BACKOFF
```

MVP에서는 backoff 없이 재시도하되, 반복 실패가 무한 spawn으로 이어지지 않도록
`startup_timeout_seconds`와 bounded waiter 상한에 의존한다.

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
