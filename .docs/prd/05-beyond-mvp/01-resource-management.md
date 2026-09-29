# Resource Management

> **Nimwake PRD** › Post Core MVP
>
> 원문 구간: `PRD.md` §52–§55

---

## 52. Resource Management — Post Core MVP

Core MVP가 안정화된 이후 memory-aware scheduling을 추가한다.

수집 후보:

- total memory
- available memory
- swap
- process RSS
- process CPU
- process uptime
- storage
- active request count

---

## 53. Memory Pressure

예:

```toml
[resources]
minimum_free_memory_mb = 500
```

새 application startup 이전에 available memory가 임계값 아래라면 eviction 후보를 찾는다.

---

## 54. Eviction Policy

후보 우선순위 예:

1. active request가 없는 application
2. always-on이 아닌 application
3. 낮은 priority
4. 오래된 last request
5. 높은 RSS

예:

```text
APP          RAM      IDLE      PRIORITY

blog         300MB      2m        50
api          120MB      5m        80
portfolio    250MB     43m        20   ← candidate
agent        150MB      8m        50
```

이 기능은 Core MVP 이후에 추가한다.

---

## 55. Application Resource Limits

향후 application별 soft memory limit을 지원할 수 있다.

```toml
memory_limit_mb = 400
```

Raw Process Runtime:

```text
RSS monitoring
     ↓
warning / terminate
```

cgroup/container runtime을 사용할 경우 hard limit으로 발전시킬 수 있다.

---

← [Android Deployment와 검증](../04-platform/02-android.md) · [문서 인덱스](../README.md) · [CLI · Deployment · Storage · Observability · Security](../05-beyond-mvp/02-platform-features.md) →
