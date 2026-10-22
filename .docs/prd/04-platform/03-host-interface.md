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
| `-V`, `--version` | 버전 한 줄만 출력하고 `0`으로 끝낸다. daemon을 띄우지 않고 manifest도 읽지 않는다 |

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

### 버전을 묻는 방법

```text
nimwaked --version
nimwaked 0.1.0-rc5
```

**daemon을 띄우지 않고** 묻는다. manifest도 읽지 않으므로 배포 전에도 답한다.

호스트가 이것을 필요로 하는 이유: **모르는 인자는 설정 오류(`78`)다.** 새 flag를
붙일지 결정하려면 먼저 그 binary가 무엇인지 알아야 한다. `nimwake-android`가
`--min-free-memory-mb`를 호스트 설정으로 넘기면서 그 문제를 만났다.

돌고 있는 daemon의 버전은 다른 질문이다 — `deploy`가 떠 있는 process를 바꾸지
않으므로 디스크와 메모리가 다를 수 있다. 그쪽은 [§79](#79-status-snapshot)의
`SIGUSR1`이 답한다.

### 정지는 시작과 완료를 모두 남긴다

```text
INFO stopping application  app=blog operation_id=op#2 process=proc#1 reason=idle
INFO stopped application   app=blog operation_id=op#2 process=proc#1 took_ms=41 waiting=0
```

`operation_id`가 두 줄을 짝지운다. `took_ms`는 **정지가 시작된 때부터**이고,
정지 중에 요청이 합류해도 그 시각으로 다시 잡지 않는다 — 합류부터 재면 걸린
시간이 실제보다 짧게 나오고, 그 수치를 보고 정지가 빠르다고 판단하게 된다.

완료 줄이 없으면 정지가 끝났는지도, 얼마나 걸렸는지도 알 수 없다.
`nimwake-android`가 블로그의 SIGTERM 정지(35~46ms)를 재면서 **프로세스를 직접
봐야 했다** — Core가 시작만 남기고 있었다.

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

→ [§11 상태 모델](../02-architecture/01-core-architecture.md) ·
[§56 CLI](../05-beyond-mvp/02-platform-features.md) ·
[§76 Manifest Reload](../02-architecture/05-manifest-and-readiness.md)

---

## 85. Control Socket

`nimwaked`는 **unix domain socket**으로 상태를 묻고 조작할 창구를 둔다. §56이
"Unix Domain Socket을 우선 고려한다"고 둔 것의 구체적 계약이다.

```text
nimwake CLI  ──→  nimwake.sock  ──→  nimwaked
```

### 무엇이 문제였나

§79는 `SIGUSR1`로 상태를 **로그에 쓴다.** 사람이 읽기에는 충분하지만, 프로그램이
읽으려면 신호를 보내고 로그를 거슬러 찾아야 한다.

`nimwake-android`가 실제로 그렇게 하고 있다.

```text
scripts/apps.sh:203        kill -USR1 "$daemon"  →  nimwaked.log를 grep
scripts/device-status.sh   grep -o 'nimwaked [0-9][0-9a-z.-]*' … | tail -n 1
tests/device/app.sh        "app의 pid는 Core에게 묻는다 — SIGUSR1 상태 스냅샷"
```

**이 방식은 세 가지로 깨진다.**

```text
경합        신호를 보낸 뒤 로그가 쓰이기까지 기다려야 하고, 얼마를 기다릴지 모른다
구분 불가    같은 app의 옛 스냅샷과 새 스냅샷을 섞어 읽는다
형식 변경    §79는 "사람이 읽는다. 기계가 읽는 형식을 약속하지 않는다"고 적혀 있다
```

마지막 것이 핵심이다. §79는 **의도적으로** 형식을 약속하지 않았다. 그것을
파싱하는 쪽이 생겼다면, 약속할 창구를 따로 주는 것이 맞다.

### 신호를 없애지 않는다

`SIGUSR1`은 그대로 둔다. **socket이 없을 때도 상태를 볼 길이 있어야 한다** —
운영자가 로그만 들고 있는 상황, socket을 켜지 않은 배포가 그것이다.

둘은 같은 사실을 다른 독자에게 준다.

```text
SIGUSR1        사람이 읽는다. 형식을 약속하지 않는다 (§79)
control socket 기계가 읽는다. 형식을 약속한다 (이 절)
```

### 켜야 생긴다

```text
--control-socket <PATH>    없으면 socket을 만들지 않는다
```

**기본이 꺼짐이다.** 두 가지 이유다.

파일을 만드는 것은 부수효과다. 아무도 요구하지 않은 socket을 어딘가에 만들면,
그 경로를 고르는 순간 Core가 파일 배치를 정하는 셈이 된다. 그것은 §58
Filesystem의 주제이고 아직 정하지 않았다.

그리고 **쓸 수 있는 경로를 Core가 추측할 수 없다.** `$XDG_RUNTIME_DIR`은 Android
proot에 없고, `/run`은 쓸 수 없고, `/tmp`는 다른 사용자가 보는 곳이다. 경로를
아는 것은 호스트다.

### 같은 uid만 닿는다

```text
socket 파일 권한    0600
디렉터리            호스트가 만든다. Core는 만들지 않는다
```

§79가 신호를 고른 이유와 같다 — **인증을 만들지 않는다.** 신호는 같은 uid를
요구하고, 0600 socket도 같은 uid를 요구한다. 새 공격면이 없다.

경로의 디렉터리가 없으면 **기동을 거부한다**(`78`). 만들어 주지 않는다 — 그
디렉터리의 권한을 Core가 정하게 되고, 그것이 socket의 권한보다 느슨하면 0600이
의미를 잃는다.

### 남은 socket 파일

`bind`는 경로가 이미 있으면 실패한다. daemon이 비정상 종료하면 파일이 남는다.

```text
경로에 연결해 본다
  ├── 연결됨    다른 daemon이 살아 있다 → 기동 거부 (78)
  └── 실패      남은 파일이다 → 지우고 bind
```

**연결해 보고 판단한다.** 무조건 지우면 돌고 있는 daemon의 socket을 빼앗아, 두
daemon이 같은 app을 관리하는 상태로 간다. Invariant 1이 금지하는 것이다.

종료할 때도 지운다. 정상 종료에서 파일을 남기지 않는다.

### 주고받는 형식

**연결 하나에 요청 하나, 응답 하나, 그리고 닫는다.**

```text
클라이언트 → {"command":"status"}\n
서버      → {"contract":1,"version":"0.1.1","apps":[…]}\n
            연결 닫음
```

framing을 두지 않는다. 연결이 닫히는 것이 메시지의 끝이다. 지속 연결과 요청
식별자를 두면 상태를 들고 있어야 하고, CLI 한 번 실행에 그것이 필요하지 않다.

JSON인 이유: 파싱하는 쪽이 이미 `jq`를 쓴다. 줄 단위 `key=value`는 중첩을
표현할 수 없고, app 목록은 중첩이다.

### 계약 번호를 함께 보낸다

```text
"contract": 1
```

**이 창구는 다른 저장소가 의존한다.** 형식을 바꿀 때 읽는 쪽이 알아야 한다.
번호가 올라가면 읽는 쪽이 자기가 아는 번호인지 보고 판단한다.

모르는 필드를 더하는 것은 번호를 올리지 않는다. 필드를 **없애거나 뜻을 바꾸는
것**이 올린다.

### 모르는 명령

```text
{"command":"nonsense"}  →  {"contract":1,"error":"unknown command: nonsense"}
```

연결을 끊지 않고 오류를 돌려준다. 끊으면 읽는 쪽이 "daemon이 죽었나"와
"명령을 모르나"를 구분할 수 없다.

JSON이 아닌 입력, 너무 긴 입력도 같다 — 오류를 돌려주고 닫는다. 요청 한 줄의
상한을 두어 긴 입력이 메모리를 먹지 않게 한다.

### `status`가 먼저다

§56의 명령 목록 중 **읽기만 하는 것**을 먼저 넣는다.

```text
status      지금 무엇이 어떤 상태인가        이 절
list        app 목록                        status가 포함한다
wake/sleep  상태를 바꾼다                    나중에
deploy      §57의 주제. 호스트가 이미 한다    Core가 하지 않는다
logs        파일을 읽는 것. socket이 필요 없다
```

**읽기부터 넣는 이유**: 지금 깨져 있는 것이 읽기다(로그 파싱). 그리고 쓰기
명령은 "누가 이 app을 멈출 수 있는가"를 정해야 하는데, 그것은 0600보다 섬세한
질문이고 지금 사례가 없다.

`deploy`는 Core가 하지 않는다. `nimwake-android`가 배포 계약을 가지고 있고
([§57](../05-beyond-mvp/02-platform-features.md)), 그것을 Core가 다시 만들면 두 곳이 된다.

### `status`가 주는 것

§79가 로그에 쓰는 것과 **같은 사실**이다. 출처를 둘로 두지 않는다.

```text
contract    이 형식의 번호
version     돌고 있는 daemon의 버전 — 디스크의 바이너리와 다를 수 있다 (§74)
apps[]      name · host · state · pid · active · pending · since_ms · idle_ms · error
```

`pid`는 OS pid다. 내부 식별자가 아니라 `ps`와 맞출 수 있는 값이어야 한다 —
§79에서 한 번 틀렸던 것이다.

→ [§56 CLI](../05-beyond-mvp/02-platform-features.md) ·
[§79 Status Snapshot](#79-status-snapshot) ·
[§74 Host Interface](#74-host-interface) ·
[§58 Filesystem](../05-beyond-mvp/02-platform-features.md)

---

## 86. `nimwake` CLI

§85가 창구의 전송 형식을 정했다. 이 절은 **그 창구를 쓰는 명령**의 표면이다.

```bash
nimwake status          사람이 읽는 표
nimwake status --json   §85의 응답을 그대로
```

§56이 명령 목록을 두었고, 그중 `status`가 먼저 들어간다. 왜 읽기부터인지는
§85에 적었다.

### 왜 별도 바이너리인가

`nimwaked`에 붙이지 않는다. **daemon을 띄우는 바이너리와 daemon에게 묻는
바이너리는 수명이 다르다.**

```text
nimwaked    떠 있는다. supervisor가 관리한다
nimwake     한 번 실행하고 끝난다. 사람이 입력한다
```

한 바이너리에 두면 `nimwaked status`가 되는데, 그러면 "이것이 daemon을 띄우는
명령인가 묻는 명령인가"를 인자가 결정한다. 실수로 daemon을 하나 더 띄우는
길을 만들 이유가 없다.

### socket 경로를 추측하지 않는다

```text
--socket <PATH>      명시
NIMWAKE_SOCKET       환경 변수
둘 다 없으면          설정 오류 (78)
```

§85가 기본 경로를 두지 않은 것과 같은 이유다 — 쓸 수 있는 경로를 아는 것은
호스트다. CLI가 `/run/nimwake.sock`을 먼저 들여다보면, 그 경로에 다른 것이
있는 기계에서 엉뚱한 곳에 묻는다.

**환경 변수를 두는 이유**: 호스트가 한 번 export하면 운영자가 경로를 외우지
않아도 된다. 플래그가 환경 변수를 이긴다 — 한 번만 다른 곳에 묻고 싶은 경우가
있다.

### 사람이 먼저, 기계는 `--json`

```text
APP      STATE      PID     ACTIVE  IDLE
blog     RUNNING    12971        0  3.1s
notes    SLEEPING     —          0  —
```

**기본이 사람인 이유**: 터미널에 치는 사람이 더 많다. 기계는 `--json`을 적으면
되고, 그쪽은 §85의 응답을 **가공하지 않고 그대로** 낸다 — CLI가 다시 포맷하면
계약이 두 곳이 된다.

사람이 읽는 표의 열 구성은 **약속하지 않는다.** §79의 로그와 같다. 파싱할 것이
필요하면 `--json`이 그것이다.

### 종료 코드로 구분한다

```text
0   답을 받았다
69  창구에 닿지 못했다        daemon이 꺼졌거나 경로가 틀렸다
70  daemon이 오류를 돌려줬다  모르는 명령 등
78  인자가 틀렸다             경로를 주지 않았거나 모르는 플래그
```

**69와 78을 가른다.** 스크립트가 "아직 안 떴다"(기다린다)와 "설정이 틀렸다"
(고친다)를 다르게 다뤄야 한다. 둘을 같은 코드로 내면 재시도 루프가 영원히 돈다.

`nimwaked`의 코드와 같은 뜻을 쓴다 — 78은 설정, 69는 쓸 수 없음.

### 하지 않는 것

| | |
| --- | --- |
| daemon 띄우기 | `nimwaked`가 한다. CLI는 묻기만 한다 |
| `deploy` | §57이고 `nimwake-android`가 이미 한다. Core가 다시 만들면 두 곳이 된다 |
| `logs` | 파일을 읽는 것이다. `tail`이 더 잘 한다 |
| 상태를 바꾸는 명령 | §85에 적은 대로 "누가 멈출 수 있는가"를 먼저 정해야 한다 |

→ [§85 Control Socket](#85-control-socket) ·
[§56 CLI](../05-beyond-mvp/02-platform-features.md) ·
[§74 Host Interface](#74-host-interface)

---

← [Android Deployment와 검증](02-android.md) · [문서 인덱스](../README.md) · [Resource Management](../05-beyond-mvp/01-resource-management.md) →
