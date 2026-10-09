# Application Manifest와 Readiness

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §21–§22

---

## 21. Application Manifest

application은 `nimwake.toml`을 통해 정의한다.

MVP 예시:

```toml
name = "hello"

[server]
host = "hello.localhost"
port = 3101

[runtime]
command = ["node", "server.js"]
working_dir = "./examples/hello-node"

# 선택. daemon의 환경 위에 덮어쓴다. (§20)
[runtime.env]
NODE_ENV = "production"

[lifecycle]
idle_timeout_seconds = 60
startup_timeout_seconds = 5
shutdown_timeout_seconds = 5
response_header_timeout_seconds = 60   # §75
max_pending_requests = 256
```

대응하는 domain model:

```rust
struct AppSpec {
    name: String,
    server: ServerSpec,
    runtime: RuntimeSpec,
    lifecycle: LifecycleSpec,
}
```

manifest는 특정 runtime이 아니라 동작을 기술한다.

따라서 다음은 모두 동등하게 취급한다.

```text
node server.js
uvicorn main:app
./go-server
./rust-server
bun server.ts
```

manifest의 계약은 다음 한 문장이다.

> 이 command를 실행하면 이 port에서 HTTP application이 listen한다.

manifest schema는 `v0.1.0` 이전까지 변경될 수 있다.

---

### Port 할당 정책

MVP는 manifest에 명시된 **static port**를 사용한다.

동적 port allocation은 Core MVP 범위 밖이다. → [§63 Post-MVP 후보](../05-beyond-mvp/03-long-term-roadmap.md)

static port를 쓰는 대신 **registry load 시점의 검증이 필수**다.

---

### Manifest Validation

registry를 구성할 때 다음을 검증하고, 하나라도 위반하면 **기동을 거부한다.**

```text
unique name
unique host
unique port
port != gateway port
valid command (non-empty)
valid working_dir (존재하는 경로)
idle/startup/shutdown timeout > 0
max_pending_requests > 0
```

충돌 예:

```text
App A → 3101
App B → 3101
        ↓
   load 실패
```

검증 실패를 런타임까지 미루면 cold start 시점에야 port 충돌이 드러난다.
manifest validation은 daemon 기동 단계에서 수행한다.

---

## 22. Readiness

process spawn 성공은 application readiness를 의미하지 않는다.

기본 readiness 방식은 TCP connection이다.

```text
spawn
  ↓
127.0.0.1:<port>
  ↓
TCP connect
  │
  ├── fail → retry
  │
  └── success
          ↓
        READY
```

retry interval은 초기에는 작게 잡는다. 예: 약 50ms.

startup은 반드시 timeout을 가진다.

```text
startup_timeout_seconds
```

timeout 발생 시:

```text
STARTING
   ↓
FAILED
```

로 전환한다.

HTTP health check는 이후 확장할 수 있다.

---

### Readiness와 Process Exit의 경쟁

readiness 성공과 process 종료는 동시에 일어날 수 있다.

```text
spawn
 ↓
port open
 ↓
readiness success      ┐
                       ├── 도착 순서는 scheduler에 따라 달라진다
process exits          ┘
```

따라서 두 event 모두 `identity`를 동반해야 하며, 다음 규칙으로 처리한다.

```text
StartCompleted(operation_id)   → operation_id 불일치 시 무시
ProcessExited(process_id)      → process_id 불일치 시 무시
```

`StartCompleted(success)`가 도착했더라도 해당 process가 이미 종료되었다면
`ProcessExited`에 의해 `FAILED`로 전이된다.

→ [§68 Lifecycle Identity](02-lifecycle-invariants.md)

---

← [Runtime Abstraction과 Process Ownership](../02-architecture/04-runtime.md) · [문서 인덱스](../README.md) · [Gateway Routing과 Request Lease](../02-architecture/06-gateway-and-requests.md) →
