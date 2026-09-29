# 제품 포지셔닝과 핵심 성공 조건

> **Nimwake PRD** › 규칙과 포지셔닝
>
> 원문 구간: `PRD.md` §66–§67

---

## 66. 최종 제품 포지셔닝

Nimwake는 기존 범용 self-hosted PaaS를 그대로 재현하는 것을 목표로 하지 않는다.

핵심 포지셔닝은 다음과 같다.

```text
Resource-Constrained Hardware
            +
Raw Process Runtime
            +
Scale-to-Zero
            +
Low Memory Overhead
            +
Runtime Independence
            +
Provider Independence
```

즉,

> **많은 애플리케이션을 배포해 두되, 필요한 애플리케이션만 살아 있게 만드는 초경량 단일 머신 PaaS**

를 목표로 한다.

초기 실제 검증 환경은 4GB RAM Android 스마트폰이지만 Nimwake 제품 자체는 Android에 종속되지 않는다.

---

## 67. 핵심 성공 조건

Nimwake의 성공 여부는 feature 개수로 판단하지 않는다.

다음 lifecycle이 장시간 반복되어도 안정적이어야 한다.

```text
request
   ↓
wake
   ↓
serve
   ↓
idle
   ↓
sleep
   ↓
repeat
```

그리고 이 과정에서 다음 조건을 만족해야 한다.

```text
one application
     ↓
at most one process

concurrent requests
     ↓
one startup

active request
     ↓
no idle shutdown

1000 lifecycle cycles
     ↓
no process leak
```

이 lifecycle의 신뢰성을 증명하는 것이 Nimwake Core의 첫 번째 목표다.

---

← [Development Rules](../06-conventions/01-development-rules.md) · [문서 인덱스](../README.md)
