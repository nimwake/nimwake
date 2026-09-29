# CLI · Deployment · Storage · Observability · Security

> **Nimwake PRD** › Post Core MVP
>
> 원문 구간: `PRD.md` §56–§61

---

## 56. CLI — Post Core MVP

예상 CLI:

```bash
nimwake list
nimwake status <app>
nimwake deploy <artifact>
nimwake rollback <app>
nimwake wake <app>
nimwake sleep <app>
nimwake restart <app>
nimwake logs <app>
nimwake inspect <app>
nimwake system
```

CLI와 daemon의 IPC는 Unix Domain Socket을 우선 고려한다.

```text
nimwake CLI
     │
     ▼
nimwake.sock
     │
     ▼
nimwaked
```

Public admin TCP API는 필수로 두지 않는다.

---

## 57. Deployment — Post Core MVP

Nimwake는 application build를 하지 않는다.

장기적으로 표준 deployment input은 다음 형태를 고려한다.

```text
release.tar.gz
nimwake.toml
```

flow:

```text
artifact
   ↓
extract
   ↓
releases/<id>
   ↓
validate
   ↓
registry update
   ↓
current
   ↓
SLEEPING
```

배포 완료 시 application을 즉시 실행할 필요는 없다.

---

## 58. Filesystem — Future

예상 구조:

```text
~/.nimwake/

├── apps/
│   └── <app>/
│       ├── releases/
│       └── current
│
├── logs/
├── data/
├── config/
└── nimwake.sock
```

일반 Linux 설치에서는 향후 다음 위치를 지원할 수 있다.

```text
/var/lib/nimwake
/etc/nimwake
/run/nimwake
```

Android/proot 환경에서는 `$HOME/.nimwake` 기반을 우선한다.

---

## 59. Database Strategy

Nimwake Core는 application database를 자동 provision하지 않는다.

저빈도 application에는 SQLite를 권장할 수 있다.

```text
App → SQLite
```

PostgreSQL이 필요한 경우 별도의 always-running infrastructure service로 운영한다.

```text
App A ─┐
App B ─┼──▶ PostgreSQL
App C ─┘
```

application scale-to-zero와 DB lifecycle을 분리한다.

---

## 60. Logging & Observability — Future

향후 application-level metric:

- request count
- active request count
- cold start count
- cold start duration
- last request
- uptime
- RSS
- CPU
- exit code
- crash count
- health failure count

system-level metric:

- total RAM
- available RAM
- swap
- storage
- running app count
- sleeping app count
- eviction count
- Nimwake uptime

Core MVP에서는 필수 metric만 tracing/log 형태로 제공한다.

---

## 61. Security

기본 원칙:

- application internal port는 loopback bind 권장
- public HTTP는 Gateway를 통해 노출
- application working directory 분리
- environment / secret 분리
- secret을 log에 출력하지 않음
- Admin IPC는 Unix Socket 우선
- trusted code만 실행
- hostile multi-tenant workload 미지원

Ingress security는 ingress implementation의 책임으로 분리한다.

---

← [Resource Management](../05-beyond-mvp/01-resource-management.md) · [문서 인덱스](../README.md) · [Long-Term Roadmap](../05-beyond-mvp/03-long-term-roadmap.md) →
