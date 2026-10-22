# Gateway Routing과 Request Lease

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §23–§26, §69, §75, §78, §80, §81

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
WebSocket upgrade        → 501로 거절한다. 조용히 내려가지 않는다 (§84)
SSE                      → 응답 body는 흘러간다. 다만 연결이 열려 있는 동안
                           lease를 쥐므로 app이 잠들지 않는다
매우 큰 streaming upload  → 상한을 켤 수 있다 (§83). 기본은 꺼짐
임의로 오래 유지되는 long-lived request
```

**"보장하지 않음"이 "아무 말 없이 실패한다"는 뜻은 아니다.** WebSocket은 §84가
`501`로 거절하고 로그를 남긴다 — 지원하지 않는다는 사실이 보여야 한다.

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

## 78. Memory Admission Control

§53이 모양을 예로 보였을 뿐 계약이 없었다. 이 절이 그 계약이다.

§54 Eviction Policy는 **여전히 Post-MVP다.** 이 절은 메모리가 부족할 때
**새로 깨우지 않는 것**까지만 정한다. 돌고 있는 application을 내리지 않는다.

### 무엇이 문제였나

제한 장치가 없었다. Next.js 앱 하나가 96~118MB이고, 요청이 올 때마다 하나씩
깨어난다. 메모리가 바닥나면 **OOM killer가 무엇을 죽일지 우리가 정하지 못한다**
— 돌고 있던 application일 수도, `nimwaked` 자신일 수도 있다.

요청 하나를 거절하는 것이 돌고 있는 것을 잃는 것보다 낫다.

### 임계값

```text
nimwaked --min-free-memory-mb <N>
```

`0`이 기본이고 그때는 검사하지 않는다. **기본으로 켜지 않는 이유**: 안전한
값은 기기마다 다르고, 켜져 있으면 지금까지 되던 기동이 거절될 수 있다.
scale-to-zero 엔진이 기본값 때문에 app을 깨우지 않는 것은 놀랄 일이다.

manifest가 아니라 CLI다. 임계값은 **시스템 전체의 성질**이고 application마다
다른 값을 갖는 것이 아니다. daemon 설정은 모두 CLI 인자다. (§74)

### 어디에 적용하나

```text
적용      잠든 application을 깨우는 요청 (SLEEPING · FAILED에서의 acquire)
적용 안 함 이미 RUNNING인 application으로 가는 요청
적용 안 함 STARTING에 합류하는 요청
적용 안 함 stop 직후의 재기동
```

**돌고 있는 것은 건드리지 않는다.** 이 절의 목적이 그것을 지키는 것이다.

stop 직후의 재기동을 검사하지 않는 이유: 방금 그 application이 쓰던 메모리가
풀렸고, stop을 기다린 요청을 거절하는 것이 가장 나쁜 결과다. (§31)

### 거절하는 방식

```text
503 Service Unavailable
Retry-After 없음
```

`Retry-After`를 붙이지 않는다. 메모리가 언제 풀릴지 **모른다.** §77의 backoff는
남은 시간을 알기 때문에 붙인다. 모르는 것을 숫자로 적으면 거짓말이 된다.

overload(§69)도 `503`에 `Retry-After`가 없다. 둘은 본문과 로그로 구분한다.
클라이언트가 할 일은 어느 쪽이든 같다 — 다시 보내면 될 수도 있다.

### 막지 못하는 것

**동시에 깨어나는 경우는 막지 못한다.**

application의 메모리 비용을 기동 전에 알 수 없다. 앱 8개가 같은 순간에 요청을
받으면 여덟 결정이 모두 같은 가용량을 보고 모두 통과한다.

막는 것은 **하나씩 쌓이는 경우**다. 전화기에서 몇 시간에 걸쳐 앱이 하나씩
깨어나는 것이 실제 양상이고, 매 cold start가 그 시점의 가용량을 본다.

예약(reservation)으로 동시성까지 막으려면 actor들이 공유하는 메모리 예산이
필요하고, 그것은 application마다의 예상 RSS를 먼저 알아야 한다. 측정 기반이
생긴 뒤에 다룬다.

### 가용량의 정의

`/proc/meminfo`의 `MemAvailable`을 쓴다. `MemFree`가 아니다.

`MemFree`는 page cache를 쓰지 않은 양만 센다. Linux는 남는 메모리를 cache로
쓰므로 `MemFree`는 정상 상태에서도 작고, 그것으로 판정하면 **멀쩡한 기기에서
계속 거절한다.** `MemAvailable`은 커널이 "회수 가능"으로 계산한 값이다.

읽을 수 없으면 **통과시킨다.** 판독 실패로 서비스를 멈추지 않는다.

→ [§53 Memory Pressure](../05-beyond-mvp/01-resource-management.md) ·
[§54 Eviction Policy](../05-beyond-mvp/01-resource-management.md) ·
[§74 Host Interface](../04-platform/03-host-interface.md)

## 80. Idle Eviction

§78은 메모리가 부족할 때 **새로 깨우지 않는 것**까지였다. 이 절은 그 앞에
**쓰지 않는 것을 먼저 내리는 것**을 더한다.

§54 Eviction Policy의 우선순위 다섯 단계 중 **두 개만** 쓴다.
`priority`와 `always_on`은 여전히 Post-MVP다.

### 무엇이 문제였나

§78만 있으면 **아무도 쓰지 않는 메모리를 쥔 채 요청을 거절한다.**

```text
app A·B·C가 깨어 118MB씩 쥐고 200초째 유휴    idle timeout은 300초
app E로 요청 → 503
```

A·B·C는 활성 요청이 없고 100초 뒤에는 어차피 스스로 잠든다. **그것을 기다리는
대신 지금 내리면 E가 서빙된다.** idle timeout은 시간 기반 eviction이고, 이 절은
거기에 메모리 기반을 더한다.

### 누구를 내리나

```text
1. state == RUNNING
2. active_requests == 0
3. 그중 가장 오래 유휴인 것        last_request_at이 가장 오래된 것
```

**활성 요청이 있는 application은 절대 내리지 않는다.** 그것을 내리면 lease를
쥔 요청이 깨지고, §78이 지키려던 것을 스스로 깨뜨린다.

후보가 없으면 아무것도 하지 않는다. 전부 바쁘면 내릴 것이 없고, 그때는 §78의
거절이 맞는 답이다.

### 한 번에 하나만

정지는 메모리를 **즉시** 돌려주지 않는다. process가 빠져나갈 시간이 필요하다.
한 tick에 여럿을 내리면 첫 정지의 효과를 보기 전에 과하게 내린다.

```text
10초마다  가용 메모리 확인 → 부족하면 하나 정지 → 다음 tick에서 다시 본다
```

### 언제 도나

`--min-free-memory-mb`가 `0`(기본)이면 **돌지 않는다.** §78과 같은 임계값을 쓰고,
꺼져 있으면 `/proc/meminfo`를 읽지도 않는다.

### 왜 주기적인가 — 거절 지점이 아니라

§78의 검사는 각 application의 actor 안에 있고 **다른 application을 모른다.**
거절 지점에서 eviction을 하려면 요청 경로가 application들을 가로질러 정책을
결정해야 한다.

주기적 reaper는 요청 경로를 건드리지 않는다. 새 권한도 만들지 않는다 — 기존
handle로 `Stop`을 보내고, 그것은 §76의 재적재가 이미 하는 일이다.

### 정지 중에 요청이 오면

§30의 graceful shutdown을 그대로 쓴다. 정지 중에 온 요청은 §31대로 큐에 들어가
정지가 끝난 뒤 다시 기동한다. **요청은 잃지 않는다.**

그 경우 내린 효과가 없어지고 지연만 늘지만, idle timeout이 요청과 경쟁할 때와
같은 상황이고 §29의 generation 비교가 다루는 것과 같은 종류다. 드물고 스스로
교정된다.

### 판단은 한 걸음이다

`sweep` 하나가 "메모리를 보고, 부족하면 하나 내린다"를 한다. daemon은 ticker로
그것을 부를 뿐이다.

처음에는 loop 전체를 daemon에 두었다. **그러면 규칙을 검증할 길이 없다** —
테스트가 실제 메모리 압박을 만들어야 하는데 `echo-server`는 2MB라 개발 기계에서
임계값을 넘길 수 없고, 임계값을 도달 불가능하게 높이면 §78이 모든 기동을 막아
깨어 있는 app이 없다.

한 걸음을 core로 옮기니 `FakeRuntime`으로 가용 메모리를 정해 결정적으로
확인할 수 있다.

### 로그에 이유를 남긴다

`StopReason::MemoryPressure`로 내린다. `Idle`과 구분해야 운영자가 "스스로 잠든
것"과 "메모리 때문에 내린 것"을 가를 수 있다.

→ [§78 Memory Admission Control](#78-memory-admission-control) ·
[§54 Eviction Policy](../05-beyond-mvp/01-resource-management.md) ·
[§29 Idle 판정](07-scale-to-zero.md) · [§30 Graceful Shutdown](07-scale-to-zero.md)

## 81. Forwarded Client Address

Gateway는 backend로 보내는 요청에 **자기가 관측한 peer 주소**를 `X-Forwarded-For`에
덧붙인다.

### 무엇이 문제였나

지금까지 gateway는 `X-Forwarded-For`를 건드리지 않았다. hop-by-hop header만 지우고
나머지는 그대로 넘겼다. 결과가 둘로 갈린다.

```text
터널·프록시 뒤   프록시가 XFF를 넣고 gateway가 통과시킨다 → application이 본다
직접 노출        XFF가 없다. application은 127.0.0.1만 본다
```

**직접 노출이 "폰을 서버로 쓴다"의 기본 형태다.** `--listen 0.0.0.0`으로 LAN에
열면 application이 접속자를 알 수 없고, 접근 로그·속도 제한·차단이 불가능하다.

### 무엇을 하나

```text
기존 X-Forwarded-For 있음   그 뒤에 관측한 peer를 덧붙인다
없음                        peer 하나로 만든다
```

표준 reverse proxy의 동작이다. 새 header를 만들지 않는다 — application과 framework가
이미 `X-Forwarded-For`를 읽는다.

port는 넣지 않는다. `X-Forwarded-For`는 주소 목록이고 port는 그 문법에 없다.

### 믿을 수 있는 것은 **맨 오른쪽**뿐이다

```text
X-Forwarded-For: 1.2.3.4, 10.0.0.5, 192.168.0.9
                 └ 클라이언트가 보냈을 수 있다    └ gateway가 관측했다
```

gateway는 왼쪽의 값들을 **검증하지 않는다.** 직접 노출된 상태에서 클라이언트가
`X-Forwarded-For: 1.2.3.4`를 보내면 그것이 그대로 남고 그 뒤에 진짜 peer가 붙는다.

**맨 오른쪽 값만 gateway가 직접 관측한 것이다.** 그 왼쪽은 클라이언트나 상위
프록시가 주장한 값이다. application은 신뢰 경계를 알고 읽어야 한다.

trusted proxy 목록을 두지 않는다. 그것은 설정 항목이 늘고 "누가 신뢰받는가"를
Core가 알아야 하는데, ingress는 §61이 Core 범위 밖으로 분리한 것이다.

### `X-Forwarded-Proto`는 건드리지 않는다

Core는 TLS를 제공하지 않는다. TLS를 끝내는 쪽이 그 header의 주인이고, gateway가
덮어쓰면 상위 프록시가 적은 사실을 지운다.

→ [§24 HTTP Contract](#24-http-contract) ·
[§61 Security](../05-beyond-mvp/02-platform-features.md) ·
[§74 Host Interface](../04-platform/03-host-interface.md)

---

## 82. Per-Application Peer Allowlist

application은 **자기에게 닿아도 되는 peer 대역**을 선언할 수 있다. gateway가
직접 관측한 TCP peer 주소로 판단하고, 맞지 않으면 `403`으로 거절한다.

```toml
[server]
host  = "notes.home.arpa"
port  = 3103
allow = ["192.168.0.0/16", "10.0.0.0/8"]
```

없으면 모두 허용한다. 기존 manifest는 그대로 동작한다.

### 무엇이 문제였나

한 daemon에 **공개 app과 LAN 전용 app이 함께** 있을 때, 지금은 둘을 가를 방법이
없다. 모든 app이 같은 listener를 공유하고 gateway는 `Host` header만 본다.

```text
인터넷 → 터널 → cloudflared → gateway :8080    peer = 127.0.0.1
같은 Wi-Fi ───────────────────→ gateway :8080    peer = 192.168.x.x
```

LAN 전용 app을 막는 것이 **"터널 설정에 그 이름이 없다"뿐이다.** 와일드카드
`*.example.com`, `Host` 재작성, catch-all 규칙 중 하나만 잘못 써도 LAN 전용 app이
인터넷에 열린다. 설정 파일 한 줄이 유일한 방어선이다.

§61은 ingress security를 ingress 구현으로 분리했다. **그 분리는 유지한다** — 이
절은 ingress를 대신하지 않는다. 다만 *"누가 이 app에 닿아도 되는가"는 app의
성질*이고, 그것을 manifest에 적을 수 있어야 한다. 방어를 두 겹으로 둔다.

### TCP peer만 본다 — `X-Forwarded-For`는 보지 않는다

**이것이 이 절에서 가장 중요한 결정이다.**

```text
TCP peer            gateway가 kernel에서 직접 얻는다. 위조할 수 없다
X-Forwarded-For     클라이언트가 보낼 수 있다. 위조할 수 있다
```

§81이 적은 그대로다 — XFF의 맨 왼쪽은 아무나 주장할 수 있는 값이다. 접근 제어가
그것을 믿으면 `X-Forwarded-For: 192.168.0.1` 한 줄로 우회된다.

판단에 쓰는 값은 §81이 XFF **맨 오른쪽에 덧붙이는 값과 같다.** 하나의 사실을 두
곳에서 쓴다.

### IPv4로 맞춘 IPv6를 IPv4로 본다

gateway가 `::`에 bind하면 LAN의 `192.168.0.9`가 **`::ffff:192.168.0.9`로**
도착한다. 이것을 IPv4로 되돌리지 않으면 `allow = ["192.168.0.0/16"]`이 조용히
어긋난다.

```text
::ffff:192.168.0.9  →  192.168.0.9     이 변환을 먼저 한다
```

**어긋나는 방향이 둘 다 나쁘다.** LAN 규칙이 LAN 요청을 막으면(fail-closed) 사람이
알아채고 규칙을 넓히는데, 그러다 터널까지 들어온다. loopback 규칙이 `::1`을 막으면
터널이 죽는다. 둘 다 "규칙을 썼는데 생각과 다르게 동작하는" 상태다.

### 규칙이 조용히 넓어지는 쪽으로 틀리지 않는다

문법은 맞지만 **쓴 사람의 뜻과 다르게 동작하는** 두 가지를 거부한다.

```text
192.168.0.9/16        prefix 밖에 bit가 켜져 있다        거부
::ffff:192.168.0.0/x  IPv4로 맞춘 IPv6를 그대로 썼다      거부
```

**host bit**: `192.168.0.9/16`을 받아들이려면 host bit를 지워야 하는데, 그것은
"이 주소 하나"를 의도한 사람에게 **대역 전체를 여는 것**이다. 거부하고 고쳐 쓸
것(`192.168.0.0/16`)을 오류에 적는다.

**IPv4로 맞춘 IPv6 literal**: 정규화하면 `/112`가 IPv4의 prefix로 재해석되어
뜻이 바뀌고, 정규화하지 않으면 peer만 IPv4로 되돌아가 영영 맞지 않는다. 어느
쪽도 뜻과 다르므로 IPv4로 쓰라고 말한다. **peer 주소의 변환은 그대로 한다** —
위의 "IPv4로 맞춘 IPv6를 IPv4로 본다"는 접속한 peer에 대한 것이고, 이것은
manifest에 적는 값에 대한 것이다.

### 거절할 때

```text
403 Forbidden
app을 깨우지 않는다          §78과 같은 자리 — 깨우기 전에 판단한다
로그 한 줄 (app · peer)      막힌 것을 운영자가 알아야 한다
```

**깨우지 않는 것이 핵심이다.** 거절할 요청이 process를 띄우면 그것만으로
자원 고갈 수단이 된다.

`404`가 아니라 `403`인 이유: app은 존재하고 요청자가 자격이 없다. `404`로
숨기는 편이 정보를 덜 주지만, `Host`를 아는 사람은 이미 그 이름을 안다.

### 틀린 CIDR은 기동을 막는다

§21의 전체 검증에 들어간다. 하나라도 문법이 틀리면 **daemon이 뜨지 않고**, §76
재적재는 거부되며 기존 상태가 남는다.

접근 제어 규칙을 조용히 건너뛰면 **열린 채로 도는 것**과 같다. 규칙을 쓴 사람은
그것이 적용된다고 믿는다.

### 하지 않는 것

| | |
| --- | --- |
| listener 분리 | `--listen`을 여럿 두고 app이 고르는 방식. 더 강하지만 계약이 크다. peer 제한으로 충분한지 먼저 본다 |
| deny 목록 | allow만 둔다. 둘을 함께 두면 우선순위를 정해야 하고, 그 규칙을 잘못 읽는 것이 바로 구멍이다 |
| host 이름 | 역방향 조회는 느리고 믿을 수 없다 |

→ [§81 Forwarded Client Address](#81-forwarded-client-address) ·
[§21 Manifest Validation](../02-architecture/05-manifest-and-readiness.md) ·
[§61 Security](../05-beyond-mvp/02-platform-features.md) ·
[§78 Memory Admission Control](../04-platform/03-host-interface.md)

---

## 83. Request Intake Limits

gateway가 **요청을 받는 쪽**에 두 가지 상한을 둔다. §75는 backend의 *응답*을
기다리는 시간이고, 이 절은 클라이언트가 보내는 *요청*이다.

```text
--header-read-timeout-seconds  요청 header를 다 받기까지 (기본 15초, 켜져 있음)
--max-request-body-mb          요청 본문 크기 (기본 0 = 꺼짐)
```

**기본값이 다른 이유가 이 절의 핵심이다.**

### 둘은 위험의 성질이 다르다

```text
느린 header   연결 하나가 Core의 task와 socket을 붙잡는다   Core 자신의 문제
큰 본문       Core를 거쳐 backend로 흐를 뿐                 backend를 위한 방어
```

gateway는 요청 본문을 **버퍼링하지 않는다.** `Incoming`을 그대로 backend로
흘린다. 그래서 큰 본문은 Core의 메모리를 먹지 않는다. 막아 주는 대상은 Core가
아니라 application이다.

느린 header는 다르다. header를 다 받기 전에는 어느 app으로 갈지도 모르므로,
Core가 연결과 task를 쥔 채 기다린다. **1,000개를 천천히 열면 Core가 멈춘다.**
이것이 slowloris다.

### header 읽기 상한은 켜 둔다

정상 클라이언트가 요청 header를 보내는 데 15초가 걸릴 이유가 없다. 끊어서
곤란해지는 쪽이 없으므로 기본값을 둔다.

`0`으로 끄는 길은 남긴다. 아주 느린 링크에서 운영자가 판단할 수 있다.

### 본문 상한은 꺼 둔다

**어떤 기본값이든 누군가의 정상 업로드를 깨뜨린다.** §75가 응답 body에 상한을
두지 않은 것과 같은 이유다 — 막으려는 것은 큰 요청이 아니라 *악의적인* 요청인데,
Core는 둘을 구분할 수 없다.

application 프레임워크는 대개 자기 상한을 가지고 있다. Core의 상한은 그것이
없거나 더 앞에서 끊고 싶을 때 쓰는 것이다.

### 상한을 넘으면

```text
Content-Length가 상한을 넘는다   413, app을 깨우지 않는다
흐르다가 상한을 넘는다 (chunked)  전송을 끊는다
```

**`Content-Length`를 먼저 본다.** 그러면 backend에 접속하기도 전에, app을
깨우기도 전에 끝난다 — §82·§78과 같은 자리다.

`Content-Length`가 없는 chunked 요청은 흘려 보면서 센다. 넘는 순간 끊지만, 그때는
이미 backend가 일부를 받았다. **깨끗한 `413`을 줄 수 없다** — 응답이 이미 오고
있을 수 있다. 그래도 끊는 이유는 `Content-Length`만 보면 chunked로 우회되기
때문이다. 상한을 켠 사람은 그것이 적용된다고 믿는다.

### 하지 않는 것

| | |
| --- | --- |
| 연결 수 제한 | OS의 `somaxconn`과 fd 한계가 이미 건다. Core가 또 세면 두 곳이 된다 |
| 속도 제한 | 요청자를 구분해야 하는데 Core는 인증이 없다. §61이 분리한 것 |
| 응답 body 상한 | §75가 두지 않기로 한 것. backend는 Core가 띄운 것이라 신뢰 경계가 다르다 |

→ [§75 Response Header Timeout](#75-response-header-timeout) ·
[§82 Per-Application Peer Allowlist](#82-per-application-peer-allowlist) ·
[§24 HTTP Contract](#24-http-contract) ·
[§61 Security](../05-beyond-mvp/02-platform-features.md)

---

## 84. Upgrade Requests Are Refused, Not Downgraded

gateway는 `Upgrade: websocket` 요청을 **`501`로 거절한다.** 조용히 header를 지우고
평범한 요청으로 넘기지 않는다.

### 무엇이 문제였나

§24가 WebSocket을 "보장하지 않음"으로 두었다. 그 결정은 그대로다 — 이 절은
WebSocket을 **지원하는 것이 아니라, 지원하지 않는다는 사실을 보이게 하는 것**이다.

지금까지의 동작:

```text
클라이언트   GET /ws
            Upgrade: websocket        hop-by-hop이라 gateway가 지운다
            Connection: Upgrade       같이 지운다
     ↓
backend     평범한 GET으로 보인다 → 200이나 400을 돌려준다 (101이 아니다)
     ↓
클라이언트   "handshake failed" — 왜인지 알 수 없다
```

**지운 쪽이 아무 말도 하지 않는 것이 문제다.** 앱을 의심하고, 프레임워크를
의심하고, 터널을 의심한 다음에야 gateway에 닿는다.

### 무엇을 하나

```text
Upgrade: websocket        501, app을 깨우지 않는다, 로그 한 줄
그 밖의 Upgrade 토큰       지금처럼 지우고 넘긴다
```

`501 Not Implemented`가 맞는 코드다 — 요청은 올바르고 **이 서버가 그 기능을
구현하지 않았다.** `400`은 클라이언트를 탓하는 것이고, `426 Upgrade Required`는
방향이 반대다.

**`websocket`만 본다.** `Upgrade: h2c`처럼 다른 토큰을 보내는 클라이언트가 있고,
RFC는 서버가 그것을 무시해도 된다고 한다. 지금 그 요청들은 정상 처리되고 있으므로
깨뜨리지 않는다.

### 깨우지 않는다

§82 · §83과 같은 자리다. 거절할 요청이 process를 띄울 이유가 없고, 핸드셰이크에
실패한 클라이언트는 **대개 바로 다시 시도한다** — 그때마다 앱을 깨우면 그것만으로
자원이 샌다.

### 지원하게 된다면

이 절을 지우고 §24의 "보장하지 않음"에서 WebSocket을 뺀 뒤, hyper의 upgrade
경로를 붙인다. 그때 scale-to-zero와의 상호작용을 먼저 정해야 한다 —

```text
연결이 열려 있는 동안 앱을 재울 수 없다      lease를 쥐는 것으로 될 것이다
연결이 오래 살아 있으면 idle이 영영 오지 않는다
재적재(§76)와 eviction(§80)이 연결을 끊는가
```

**지금 정하지 않는다.** 실제로 필요해진 뒤에 정한다 ([§64 Rule 8](../06-conventions/01-development-rules.md)).

→ [§24 HTTP Contract](#24-http-contract) ·
[§82 Per-Application Peer Allowlist](#82-per-application-peer-allowlist) ·
[§83 Request Intake Limits](#83-request-intake-limits)

---

← [Application Manifest와 Readiness](../02-architecture/05-manifest-and-readiness.md) · [문서 인덱스](../README.md) · [Scale-to-Zero와 Graceful Shutdown](../02-architecture/07-scale-to-zero.md) →
