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

---

← [Android Deployment와 검증](02-android.md) · [문서 인덱스](../README.md) · [Resource Management](../05-beyond-mvp/01-resource-management.md) →
