# Host Interface

> **Nimwake PRD** › 플랫폼
>
> 원문 구간: `PRD.md` §74

---

## 74. Host Interface

Core는 자기를 감독하는 host를 알지 못한다. 그러나 host의 supervisor는
`nimwaked`의 **외부 동작 몇 가지에 사실상 의존한다.**

Android Host Layer가 대표적이다. Termux 레이어의 supervisor가 proot 안의
`nimwaked`를 기동·감시·정지한다. → [§49–§51](02-android.md)

의존이 암묵적이면 인자 형태 하나만 바뀌어도 supervisor가 조용히 깨진다.
그래서 **여기 적힌 것은 계약이다.** 바뀌면 CHANGELOG와 PR 본문에 명시한다.

---

### 실행

```text
nimwaked [APPS_DIR] [OPTIONS]
```

| 옵션            | 의미                                                        |
| --------------- | ----------------------------------------------------------- |
| `--apps <DIR>`  | manifest 디렉터리. 위치 인자와 같다                          |
| `--listen <ADDR>` | gateway listen 주소. 기본 `127.0.0.1:8080`                 |
| `--port <PORT>` | listen 주소의 port만 바꾼다. `0`이면 비어 있는 port를 고른다 |
| `--min-free-memory-mb <MB>` | 잠든 app을 깨우기 전 필요한 가용 메모리. 기본 `0`은 검사하지 않는다 → [§78](../02-architecture/06-gateway-and-requests.md) |
| `--trace-json`  | 구간 계측을 JSON 한 줄씩 출력한다 → [§72](../03-delivery/05-instrumentation.md) |

환경 변수 `NIMWAKE_LISTEN`이 기본 listen 주소를 정한다. 명령줄 인자가 우선한다.

기동에 성공하면 **실제 listen 주소를 stdout 한 줄로 알린다.**

```text
listening on http://127.0.0.1:8080 — every application starts asleep
```

`--port 0`으로 띄운 쪽은 이 줄을 읽어 port를 알아낸다.

---

### Listen 주소

기본값은 loopback이다.

```text
127.0.0.1:8080
```

**외부 노출은 opt-in이다.** Core는 TLS도 인증도 제공하지 않는다.
LAN이나 그 밖으로 여는 것은 host가 명시적으로 선택해야 한다.
→ [§46 Ingress Independence](01-environments.md), [§20 Process Isolation](../02-architecture/04-runtime.md)

```bash
nimwaked --listen 0.0.0.0:8080     # LAN에 연다
NIMWAKE_LISTEN=0.0.0.0:8080 nimwaked
```

manifest 검증은 이 설정값을 그대로 쓴다. application이 gateway와 같은 port를
쓰면 기동을 거부한다. → [§21](../02-architecture/05-manifest-and-readiness.md)

---

### 종료

운영자가 보내는 신호는 셋이다.

```text
SIGINT · SIGTERM   정상 종료
SIGHUP             manifest 재적재 (§76)
SIGUSR1            상태 보고 (§79)
```

`SIGINT` 또는 `SIGTERM`을 받으면

```text
새 연결 수락 중지
      ↓
진행 중인 연결 마무리
      ↓
모든 application 정지 (SIGTERM → shutdown timeout → SIGKILL)
      ↓
정지 완료를 기다린 뒤 프로세스 종료
```

**정지 완료를 기다린다.** 기다리지 않으면 `shutdown_timeout_seconds`가 무시되고
application이 유예 없이 죽는다. → [§30](../02-architecture/07-scale-to-zero.md)

daemon이 종료된 뒤 managed child는 하나도 남지 않는다.
→ [Invariant 10](../02-architecture/02-lifecycle-invariants.md), [§71](../02-architecture/08-failure-handling.md)

---

### 종료 코드

supervisor는 daemon이 죽으면 재기동한다. 그러나 **재시도해도 나아지지 않는 실패**에서는
멈추고 operator에게 넘겨야 한다. 종료 코드로 그것을 구분한다.

`sysexits` 관례를 따른다.

| 코드 | 이름              | 의미                          | supervisor 동작                |
| ---: | ----------------- | ----------------------------- | ------------------------------ |
|  `0` |                   | 정상 종료 (`SIGINT`/`SIGTERM`) | 재기동하지 않음                 |
| `78` | `EX_CONFIG`       | manifest·설정 오류             | 재기동하지 않음, `UNHEALTHY`    |
| `69` | `EX_UNAVAILABLE`  | listen 주소 bind 실패          | backoff 후 재시도               |
|  `1` |                   | 그 외                          | backoff 후 재시도               |

`78`은 사람이 고쳐야 하는 실패다. manifest가 없거나, 파싱되지 않거나,
host/port가 충돌하거나, 명령줄 인자가 잘못된 경우가 여기 해당한다.

`69`는 시간이 해결할 수 있는 실패다. 이전 인스턴스가 아직 port를 쥐고 있는 경우가 대표적이다.

---

### Health probe

supervisor는 gateway가 살아 있는지 주기적으로 확인해야 한다.

**probe는 어떤 application의 상태도 바꾸지 않아야 한다.**

등록된 application의 `Host`로 probe하면 그 요청이 `acquire()`를 거쳐 application을
깨우고 idle timer를 계속 리셋한다. supervisor가 scale-to-zero를 무력화하게 된다.

계약:

> 등록되지 않은 `Host`로 온 요청은 `404`를 받으며, 어떤 application의 lifecycle도 건드리지 않는다.

```bash
curl -s -o /dev/null -w '%{http_code}' -H 'Host: nimwake-health.invalid' http://127.0.0.1:8080/
# 404 → gateway alive
```

Gateway는 `registry.resolve()`가 실패하면 `acquire()` 전에 응답한다.
상태를 보지 않는다는 원칙(Invariant 3)이 여기서는 반대로, **상태를 건드리지 않는다**는 보장이 된다.

전용 health endpoint(`/__nimwake/health` 등)와 Admin IPC는 Post-MVP다.
→ [§60](../05-beyond-mvp/02-platform-features.md)

## 79. Status Snapshot

```text
SIGUSR1  →  지금 상태를 로그에 한 번 쓴다
```

### 무엇이 문제였나

**돌고 있는 daemon에게 "무슨 버전이고 지금 무엇을 하고 있나"를 물을 방법이 없다.**

실제로 겪은 것:

```text
deploy 뒤에도 옛 바이너리가 메모리에 남아 있었다
  → 기동 로그의 버전 줄을 grep해서 알아냈다
  → 모르고 72시간 측정을 시작할 뻔했다
app이 깨어 있는지 확인하려면
  → /proc을 뒤지거나 요청을 보내 봤다
```

`deploy`는 떠 있는 process를 바꾸지 않는다. 그래서 "디스크의 바이너리"와 "돌고
있는 것"이 다를 수 있고, **그 차이를 물어볼 창구가 없었다.**

### 무엇을 쓰나

```text
nimwaked <version> — <N> application(s)
  <name>  <host>  <STATE>  pid=<OS pid>  process=<내부 식별자>
          active=<n>  pending=<n>  since=<기동부터>  idle=<마지막 요청부터>
```

`AppStatus`(§11)를 그대로 쓴다. 새 지표를 만들지 않는다.

`pid`는 **OS의 pid**다. `process`는 stale event 판정(Invariant 8)을 위한 내부
식별자이고 `ps`와 맞춰 볼 수 없다. 둘을 함께 적는다.

`idle`을 적는 이유: eviction(§80)이 **가장 오래 유휴인 것**을 고르므로, 운영자가
다음에 무엇이 내려갈지 이 값으로 알 수 있다.

### 왜 신호인가

```text
CLI · unix socket IPC   §56 Post-MVP다
gateway 엔드포인트       안 된다 — 아래
Public admin TCP API    §56이 "필수로 두지 않는다"고 적었다
신호                    같은 uid를 요구한다. 새 공격면이 없다
```

**gateway에 붙이면 안 된다.** §74는 외부 노출을 opt-in으로 두고 Core는 TLS도
인증도 제공하지 않는다. `--listen 0.0.0.0`으로 열면 application 이름과 pid가
LAN의 누구에게나 보인다.

### 어디로 쓰나

로그(stdout)다. supervisor가 이미 그것을 파일로 모으고 있다. 새 경로를 만들지
않는다.

### 무엇을 하지 않나

```text
기계가 읽는 형식을 약속하지 않는다   사람이 읽는 것이다. 기계용은 §56의 IPC다
상태를 바꾸지 않는다                읽기만 한다
응답을 돌려주지 않는다              신호에는 반환값이 없다. 로그를 보면 된다
```

`SIGHUP`(§76)과 나란히 둔다 — 둘 다 운영자가 보내는 신호이고, 하나는 바꾸고
하나는 보여 준다.

### 등록은 알림보다 먼저다

```text
signal 등록  →  "listening on ..." 출력  →  serve
```

`SIGHUP`과 `SIGUSR1`의 **기본 동작은 프로세스 종료**다. `listening on` 줄을 보고
바로 신호를 보내는 쪽이 있으므로(supervisor · 스크립트 · 테스트) 등록이 늦으면
그 창에서 daemon이 죽는다.

창이 좁아 15회 시도에서 재현되지 않았다. **그래도 기본 동작이 종료인 신호에
경쟁을 남겨 두지 않는다.**

→ [§11 상태 모델](../02-architecture/03-lifecycle-and-state.md) ·
[§56 CLI](../05-beyond-mvp/02-platform-features.md) ·
[§76 Manifest Reload](../02-architecture/05-manifest-and-readiness.md)

---

← [Android Deployment와 검증](02-android.md) · [문서 인덱스](../README.md) · [Resource Management](../05-beyond-mvp/01-resource-management.md) →
