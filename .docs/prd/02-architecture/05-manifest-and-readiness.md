# Application Manifest와 Readiness

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §21–§22, §76

---

## 21. Application Manifest

application은 `nimwake.toml`을 통해 정의한다.

MVP 예시:

```toml
name = "hello"

[server]
host = "hello.localhost"
aliases = ["www.hello.localhost"]   # 선택. 같은 app에 닿는 추가 host (§87)
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

## 76. Manifest Reload

daemon을 재시작하면 모든 application이 내려간다. application 하나를 추가하려고
나머지 전부를 중단시키는 것은 실사용에서 가장 자주 부딪히는 제약이다.

`SIGHUP`으로 manifest를 다시 읽는다.

```text
SIGHUP
   │
   ▼
manifest 디렉터리를 다시 읽는다
   │
   ▼
전체 검증 (§22)
   │
   ├── 실패 → 아무것도 바꾸지 않는다. 기존 상태로 계속 서빙한다
   │
   └── 성공 → 차이만 적용한다
```

### 왜 `SIGHUP`인가

```text
nimwake reload CLI   §56 Post-MVP다
파일 감시            "편집이 끝났는가"를 알 수 없다
SIGHUP               재적재 시점을 운영자가 정한다
```

파일 감시를 쓰지 않는 이유가 중요하다. 에디터는 임시 파일을 쓰고 바꾸거나 여러
파일을 차례로 저장한다. 그 중간 상태를 읽으면 **운영자가 의도하지 않은 조합**으로
재적재한다. 종료 계약이 이미 POSIX 신호 위에 서 있으므로(§30, §48) 신호를 쓴다.

### 적용 규칙

```text
spec이 같은 application     건드리지 않는다
없어진 application          정지시키고 routing에서 뺀다
추가된 application          SLEEPING으로 등록한다
바뀐 application            정지시킨다. 다음 요청이 새 spec으로 cold start한다
```

"바뀐"의 기준은 **`AppSpec` 전체의 동등성**이다. `[lifecycle]` timeout만 바뀐
경우도 정지시킨다. 무엇이 바뀌었는지에 따라 다르게 처리하면 규칙이 늘어나고,
정지는 scale-to-zero에서 정상 경로다.

**바뀌지 않은 application을 건드리지 않는 것이 이 기능의 전부다.** 그것이
안 되면 daemon 재시작과 다를 바 없다.

### 검증은 전부-또는-전무다

§22의 검증은 **집합 전체**를 본다 — unique host, unique port, gateway port 충돌.
일부만 적용하면 어떤 manifest 파일과도 일치하지 않는 상태가 남는다.

재적재가 실패하면 로그에 이유를 남기고 기존 상태로 계속 서빙한다. daemon은
종료하지 않는다. 기동 시점의 검증 실패는 종료 코드 `78`이지만(§74), 재적재
실패는 이미 서빙 중인 application을 내릴 이유가 되지 않는다.

### manifest가 하나도 없으면 거부한다

"전부 지웠다"와 "디렉터리를 잘못 건드렸다"를 구분할 수 없다. 후자일 때 전부
내리면 복구가 어렵다. 기동 시점에는 manifest 없음이 설정 오류(`78`)이고,
재적재에서는 **현재 집합을 유지한다.**

### 정지는 §30을 그대로 쓴다

정지 대상에게 `SIGTERM`을 보내고 `shutdown_timeout_seconds`를 기다린다.
application이 스스로 마무리할 기회를 갖는다.

다만 **gateway가 lease 반납을 기다리지는 않는다.** 응답 중이던 요청은 backend가
닫히면서 끊긴다. `SIGHUP`은 운영자의 의도된 행위이므로 그 비용을 받아들인다.
요청을 모두 흘려보낸 뒤 교체하는 것(drain)은 이후 정제 대상이다.

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

---

## 87. Host Aliases

한 application이 **여러 `Host`로 닿을 수 있어야 한다.**

```toml
[server]
host = "blog.example.com"
aliases = ["www.blog.example.com", "blog.trycloudflare.com"]
port = 3101
```

### 없을 때 치르는 값

`host`는 하나뿐이고 `port` 중복은 거부된다(§21의 전체 검증). 그래서 두 호스트명을
서빙하려면 **manifest를 둘로 나눠야 하고, 그러면 같은 application이 두 번 뜬다.**

```text
blog.example.com      → app "blog"   :3101   process 1
www.blog.example.com  → app "blog2"  :3102   process 2   ← 같은 코드, 두 번째 사본
```

치르는 값:

| | |
| --- | --- |
| 메모리 | application이 두 벌 뜬다. Next.js 한 벌이 ~96 MB였다 |
| cold start | 호스트명마다 따로 깨어난다. 한쪽을 깨워도 다른 쪽은 잠들어 있다 |
| idle timer | 둘이 독립이다. 번갈아 들어오면 **둘 다 깨어 있는다** |
| 배포 | 같은 artifact를 두 디렉터리에 둬야 한다 |

작은 기계를 겨냥하는 엔진에서 **apex와 `www`를 쓰는 것만으로 비용이 두 배가 되는
것은 전제와 어긋난다.**

### 왜 `host`를 목록으로 바꾸지 않는가

```toml
host = ["a", "b"]       # 이렇게 하지 않는다
aliases = ["b"]         # 이렇게 한다
```

두 가지 이유다.

**기존 manifest가 그대로 동작해야 한다.** `host`를 "문자열 또는 목록"으로 받으면
deserializer를 직접 써야 하고, `deny_unknown_fields`와 함께 쓸 때 오류 메시지가
나빠진다. `aliases`는 `#[serde(default)]`로 끝난다.

**대표 host가 하나 있어야 한다.** §79의 상태 보고와 §86의 표가 app마다 host 하나를
보여 준다. 목록이면 "무엇을 보여 줄까"를 또 정해야 한다. `host`가 대표이고
`aliases`는 추가다.

### 검증

§21의 전체 검증에 들어간다. **하나라도 틀리면 daemon이 뜨지 않는다.**

```text
alias가 다른 app의 host와 겹친다      거부. 어느 app으로 갈지 정해지지 않는다
alias가 다른 app의 alias와 겹친다      거부. 같은 이유
alias가 자기 host와 같다              거부. 적어도 하나는 뜻이 없다
alias가 비어 있다                     거부
```

`host`와 같은 규칙으로 정규화한다 — 앞뒤 공백, port suffix, 대소문자를 무시한다.
`Blog.Example.COM:8080`과 `blog.example.com`은 같은 것이다.

중복을 **조용히 덮지 않는다.** 나중에 읽은 것이 앞의 것을 밀어내면 manifest 두
개를 둔 사람은 한쪽이 사라진 것을 모른다.

### 바꾸지 않는 것

| | |
| --- | --- |
| §82의 `allow` | app의 성질이므로 **모든 host에 같이 걸린다**. host마다 다른 대역을 두려면 app을 나눠야 한다 |
| §79 로그 · §86 표 | 대표 `host`만 보여 준다. alias는 manifest에 있다 |
| `X-Forwarded-For` | §81 그대로. 받은 `Host`를 고쳐 보내지 않는다 |
| 와일드카드 | **하지 않는다.** `*.example.com`은 "어느 app인가"를 모호하게 만들고, 모르는 호스트명이 app을 깨우는 길이 된다(§74의 health probe가 깨진다) |

와일드카드를 하지 않는 이유가 특히 중요하다. §74는 **등록되지 않은 `Host`가 `404`를
받고 아무것도 깨우지 않는다**는 계약으로 supervisor의 probe를 받는다. 와일드카드가
있으면 그 계약이 "등록되지 않은"의 뜻을 잃는다.

→ [§21 Application Manifest](#21-application-manifest) ·
[§23 Gateway Routing](06-gateway-and-requests.md#23-gateway-routing) ·
[§74 Host Interface](../04-platform/03-host-interface.md#74-host-interface) ·
[§82 Peer Allowlist](06-gateway-and-requests.md#82-per-application-peer-allowlist)
