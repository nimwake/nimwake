# Lifecycle Stress Test

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §43

---

## 43. Lifecycle Stress Test

다음 lifecycle을 반복 수행한다.

```text
sleep
 ↓
request
 ↓
start
 ↓
serve
 ↓
idle
 ↓
stop
 ↓
sleep
```

초기 목표:

```text
1000 lifecycle cycles
```

Nimwake는 장시간 실행되는 daemon이기 때문에 순간 최대 RPS보다 lifecycle 안정성이 더 중요하다.

---

### 누수 검증

종료 후 다음을 확인한다.

| 항목                    | 기준                  |
| ----------------------- | --------------------- |
| orphan process          | 0                     |
| zombie process          | 0                     |
| unexpected open port    | 0                     |
| file descriptor 증가    | bounded               |
| RSS 증가                | bounded               |
| spawn / stop count      | 일치                  |
| 남아 있는 lease         | 0                     |

before / after를 함께 기록한다.

```text
FD before                   9
FD after                    9
RSS before             13.8 MB
RSS after              14.1 MB
```

---

### Sustained Warm Load

짧은 storm과 별개로, 장시간 warm 트래픽을 유지하며 누수를 관찰한다.

```text
RUNNING 유지
   ↓
일정 RPS 지속
   ↓
RSS / FD / CPU 추이 기록
```

lifecycle stress가 반복 기동·종료의 누수를 본다면,
sustained warm load는 **proxy 경로 자체의 누수**를 본다.

---

### Daemon Shutdown 검증

stress test 종료 시 daemon을 정상 종료하고 다음을 확인한다.

```text
nimwaked 종료
      ↓
managed child process 잔존 == 0
```

→ [§71 Daemon–Child Lifetime](../02-architecture/08-failure-handling.md)

---

← [Load Tests](../03-delivery/03-load-tests.md) · [문서 인덱스](../README.md) · [Latency Instrumentation](../03-delivery/05-instrumentation.md) →
