# Gateway Routing과 Request Lease

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §23–§26, §69, §75

---

## 23. Gateway Routing

`nimwaked`의 초기 listen 주소는 다음과 같다.

```text
127.0.0.1:8080
```

Gateway는 `Host` header를 기준으로 application을 결정한다.

```text
GET /
Host: blog.example.com
        │
        ▼
    Nimwake
        │
        ▼
  AppRegistry
        │
        ▼
      blog
```

routing chain:

```text
Host
 │
 ▼
AppRegistry
 │
 ▼
AppHandle
 │
 ▼
acquire()
 │
 ▼
RequestLease
 │
 ▼
Reverse Proxy
```

Gateway는 application의 현재 상태를 검사하지 않는다.
RUNNING이든 SLEEPING이든 동일하게 `acquire()`를 호출하고, wake 여부는 AppManager가 판단한다.

---

## 24. HTTP Contract

Core MVP가 보장하는 범위를 명시한다.

```text
Supported
────────────────────────────
HTTP/1.1 일반 요청
GET / HEAD / POST / PUT / DELETE / PATCH
일반적인 크기의 request / response body


Not guaranteed (v0.1.0)
────────────────────────────
WebSocket upgrade
SSE
매우 큰 streaming upload
임의로 오래 유지되는 long-lived request
```

POST 계열을 금지하지 않는다. body는 streaming abstraction으로 전달하되,
cold start 동안은 header 수신 후 lease 확보까지 대기한다.

```text
headers received
      ↓
acquire()   (필요하면 cold start)
      ↓
backend ready
      ↓
request / body 전달
```

request body 크기 상한과 long-lived connection 정책은 Post-MVP에서 구체화한다.

---

## 25. Request Tracking

Running application은 최소 다음 상태를 추적한다.

```text
active_request_count
last_request_at
idle_generation
```

증감은 **lease 발급과 반납으로만** 이루어진다.

```text
acquire()  → active_request_count += 1
lease drop → active_request_count -= 1, last_request_at = now
```

Gateway가 counter를 직접 조작하는 경로는 존재하지 않는다.

다음 조건에서는 shutdown하지 않는다.

```text
active_request_count > 0
```

---

## 26. Acquire와 RequestLease

Gateway가 사용하는 단 하나의 lifecycle API는 `acquire()`다.

```rust
let lease = app.acquire().await?;

proxy(request, lease.backend()).await
```

`acquire()`는 AppManager의 **한 transition 안에서** 다음 두 가지를 동시에 수행한다.

```text
1. application이 RUNNING임을 보장
2. active request 예약 (active_requests += 1)
```

반환값:

```rust
struct RequestLease {
    lease_id: LeaseId,
    backend: Backend,
    app: AppHandle,
}
```

lease가 drop되면 자동으로 `RequestFinished { lease_id }`를 전달한다.

이 RAII 구조를 통해 다음 상황에서도 counter가 누락되지 않는다.

- proxy error
- early return
- cancelled request
- future refactor

---

### 왜 `ensure_running()`이 아닌가

`ensure_running()` + 별도의 `RequestStarted`로 나누면 두 연산 사이에 idle shutdown이 끼어들 수 있다.

```text
ensure_running()
      ↓
   RUNNING
             ← idle timer 발화
   STOPPING
      ↓
RequestStarted   ← 종료 중인 process로 proxy
```

`acquire()`는 이 구간을 하나의 transition으로 만들어 race를 **구조적으로 제거한다.**

→ [Invariant 7 — Acquire Atomicity](02-lifecycle-invariants.md)

---

### STOPPING 중의 acquire

`STOPPING` 상태에서 도착한 `acquire`는 실패시키지 않고 waiter로 등록한다.

```text
STOPPING
   │
   └── waiters: [...]
          │
     stop complete
          │
          ▼
      STARTING
          │
       ready
          │
          ▼
   lease 발급
```

→ [§31 Request During STOPPING](08-failure-handling.md)

---

## 75. Response Header Timeout

application이 연결을 받아 두고 응답하지 않을 수 있다.

```text
accept는 된다
   │
요청을 읽지 않는다
   │
응답을 쓰지 않는다
   │
끊지도 않는다
```

readiness probe는 이것을 걸러내지 못한다. port는 열려 있다.
`startup_timeout_seconds`도 걸러내지 못한다. readiness까지만 덮기 때문이다.
→ [§22 Manifest와 Readiness](05-manifest-and-readiness.md)

상한이 없으면 그 요청은 client가 끊을 때까지 끝나지 않는다.

### 상한은 응답 header까지만 적용한다

```toml
[lifecycle]
response_header_timeout_seconds = 60
```

```text
요청 전송 ──────── 응답 header 도착 ──────── body 전송 완료
└──────── 상한 적용 ────────┘└──── 적용하지 않는다 ────┘
```

**body 전송에는 적용하지 않는다.** 큰 파일 내려받기나 천천히 흐르는 응답을
끊지 않기 위한 것이다.

전체 요청에 상한을 두면 어떤 기본값이든 정상 요청을 깨뜨린다. 막으려는 것은
**느린 응답이 아니라 오지 않는 응답**이다.

### 상태 코드

```text
응답이 오지 않음        504 Gateway Timeout
backend를 쓸 수 없음    502 Bad Gateway
```

둘은 다르다. 앞의 것은 application이 살아 있으나 답하지 않는 상태다.
상한을 넘긴 뒤에도 application은 `RUNNING`이다. 끊는 것은 요청 하나다.
→ [§26 Acquire와 RequestLease](#26-acquire와-requestlease)

### 기본값

60초다. 넉넉하게 잡는다. 긴 DB 조회나 무거운 SSR도 header가 늦으므로 짧게
잡으면 멀쩡한 요청을 끊는다. app별로 manifest에서 재정의한다.

crash-loop backoff와 달리 이것은 Post-MVP가 아니다. backoff가 없으면 자원이
더 쓰이는 데 그치지만, 이 상한이 없으면 **요청이 영원히 끝나지 않는다.**

---

## 69. Backpressure와 Overload

Nimwake는 메모리가 제한된 장비에서 동작한다.
따라서 **대기열은 무한히 늘어나면 안 된다.** → [Invariant 9](02-lifecycle-invariants.md)

상한은 두 곳에 필요하다.

| 지점                     | 상한                   | 초과 시           |
| ------------------------ | ---------------------- | ----------------- |
| AppManager mailbox       | bounded channel        | `503`             |
| pending startup waiters  | `max_pending_requests` | `503`             |

mailbox만 제한하면 충분하지 않다.
AppManager가 command를 빠르게 소비하면서 waiter를 계속 누적시킬 수 있기 때문이다.

```text
10,000 cold-start requests

mailbox capacity = 128

AppManager가 계속 consume

        ↓

waiters = 10,000      ← mailbox 제한만으로는 막지 못한다
```

정책:

```text
pending < max_pending_requests
    ↓
  wait

pending >= max_pending_requests
    ↓
503 Service Unavailable
```

mailbox가 가득 찬 경우도 동일하게 처리한다.

```text
AppManager mailbox full
        ↓
짧은 시간 대기 후 재시도
        ↓
여전히 full
        ↓
503 Service Unavailable
```

### lease 반납은 예외다

mailbox가 가득 찼다고 해서 **lease 반납(`RequestFinished`)을 버려서는 안 된다.**

버리는 순간 `active_request_count`가 0으로 내려가지 못해 해당 application은 영원히 잠들지 못한다.
새 요청은 거절(`503`)하되, 이미 발급된 lease의 반납은 반드시 전달되어야 한다.

```text
새 요청        상한 초과 → 거절 (503)
lease 반납     상한과 무관하게 반드시 전달
```

MVP는 adaptive queue나 우선순위 큐를 만들지 않는다.
**"모든 대기열은 bounded"라는 invariant만 처음부터 지키면 된다.**

기본값은 상수로 두고, app별로 `max_pending_requests`를 재정의할 수 있게 한다.

---

← [Application Manifest와 Readiness](../02-architecture/05-manifest-and-readiness.md) · [문서 인덱스](../README.md) · [Scale-to-Zero와 Graceful Shutdown](../02-architecture/07-scale-to-zero.md) →
