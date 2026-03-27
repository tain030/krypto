# 합의 알고리즘 상세 — Simplex BFT

## 목차

1. [뷰(View)란 무엇인가](#1-뷰view란-무엇인가)
2. [메시지 타입 전체 목록](#2-메시지-타입-전체-목록)
3. [Simplex 합의 흐름 상세](#3-simplex-합의-흐름-상세)
4. [쿼럼 수학](#4-쿼럼-수학)
5. [안전성 증명 직관](#5-안전성-증명-직관)
6. [다른 BFT 알고리즘과의 비교](#6-다른-bft-알고리즘과의-비교)
7. [Minimmit: Simplex와 무엇이 다른가](#7-minimmit-simplex와-무엇이-다른가)
8. [Commonware API 레퍼런스](#8-commonware-api-레퍼런스)

---

## 1. 뷰(View)란 무엇인가

### 개념

**뷰(View)**는 BFT 합의에서 "한 번의 합의 시도 단위"입니다. 각 뷰는 고유한 번호를 가지며, 한 명의 지정된 리더가 블록을 제안합니다.

```
뷰 번호  리더      결과
──────   ──────   ──────────────────────────────────────
View 0   검증자 A  블록 B0 확정 → 블록체인에 추가
View 1   검증자 B  리더 응답 없음 → 타임아웃, 블록 없이 종료
View 2   검증자 C  블록 B1 확정 → 블록체인에 추가
View 3   검증자 D  블록 B2 확정 → 블록체인에 추가
View 4   검증자 A  네트워크 파티션 → 타임아웃
View 5   검증자 B  블록 B3 확정 → 블록체인에 추가
```

**중요한 점**: 뷰 번호 ≠ 블록 번호. View 1과 View 4가 타임아웃되어 스킵되었으므로, View 5에서 확정된 블록은 3번째 블록(B3)입니다. 실제 출력에서 view 번호에 간격이 생기는 이유가 이것입니다.

### Round = Epoch + View

Commonware의 소스코드에는 `View`와 `Round`가 구분됩니다:

```rust
pub struct Round {
    epoch: Epoch,   // 검증자 세트가 교체되는 상위 단위 (예: 스테이킹 주기)
    view:  View,    // 해당 epoch 내의 합의 시도 번호
}
```

현재 구현에서는 `Epoch::new(0)` 고정이므로 `Round ≈ View`입니다. 에포크는 나중에 검증자 세트를 교체(리스테이킹 등)할 때 사용합니다.

### Proposal의 부모 체인

각 Proposal은 이전 뷰를 가리킵니다:

```rust
pub struct Proposal<D: Digest> {
    pub round:   Round,   // 이 제안이 속한 뷰
    pub parent:  View,    // 이 제안이 빌드하는 부모 뷰
    pub payload: D,       // 블록 내용의 다이제스트 (해시)
}
```

뷰가 스킵되면 `parent`가 연속되지 않을 수 있습니다. 예를 들어 View 1이 nullified되면 View 2의 제안은 `parent = View 0`을 가집니다. 검증자는 이 간격을 채우는 Nullification 인증서를 보유해야 안전하게 투표할 수 있습니다 — 스킵된 뷰가 나중에 확정되어 포크가 생기는 것을 방지하기 위해서입니다.

---

## 2. 메시지 타입 전체 목록

Simplex에는 3종류의 **개별 투표(Vote)**와 3종류의 **집계 인증서(Certificate)**가 있습니다.

### 개별 투표 (Vote)

검증자 한 명이 서명하여 전송하는 메시지입니다.

```
Vote<S, D>
 ├── Notarize<S, D>    — 블록 제안에 대한 찬성 투표
 ├── Nullify<S>        — 현재 뷰 무효화 투표 (리더 응답 없음)
 └── Finalize<S, D>    — Notarization 인증서를 확인한 후의 최종 확정 투표
```

**Notarize 구조**:
```rust
pub struct Notarize<S: Scheme, D: Digest> {
    pub proposal:    Proposal<D>,      // 어떤 블록에 투표하는지
    pub attestation: Attestation<S>,   // 내 서명 (검증자 인덱스 + 서명값)
}
```

**Nullify 구조**:
```rust
pub struct Nullify<S: Scheme> {
    pub round:       Round,            // 무효화할 뷰
    pub attestation: Attestation<S>,   // 내 서명
}
```

**Finalize 구조**:
```rust
pub struct Finalize<S: Scheme, D: Digest> {
    pub proposal:    Proposal<D>,      // 최종 확정할 블록
    pub attestation: Attestation<S>,   // 내 서명
}
```

### 집계 인증서 (Certificate)

Q-쿼럼 이상의 개별 투표를 집계하여 만든 증명서입니다. 이 인증서 하나로 쿼럼이 달성됐음을 누구나 검증할 수 있습니다.

```
Certificate<S, D>
 ├── Notarization<S, D>   — Q-쿼럼의 Notarize 투표를 집계한 공증 인증서
 ├── Nullification<S>     — Q-쿼럼의 Nullify 투표를 집계한 무효화 인증서
 └── Finalization<S, D>   — Q-쿼럼의 Finalize 투표를 집계한 최종 확정 인증서
```

**Notarization 구조**:
```rust
pub struct Notarization<S: Scheme, D: Digest> {
    pub proposal:    Proposal<D>,      // 공증된 블록
    pub certificate: S::Certificate,   // 집계 서명 (ed25519의 경우 개별 서명 목록)
}
```

### VoteTracker — 내부 투표 집계기

엔진 내부에서는 `VoteTracker`가 각 뷰의 투표 현황을 관리합니다:

```rust
pub struct VoteTracker<S, D> {
    notarizes: AttributableMap<Notarize<S, D>>,  // 검증자별 notarize 투표
    nullifies: AttributableMap<Nullify<S>>,       // 검증자별 nullify 투표
    finalizes: AttributableMap<Finalize<S, D>>,   // 검증자별 finalize 투표
}
```

`AttributableMap`은 검증자 인덱스를 키로 하는 배열로, **검증자 한 명당 투표 한 개만** 저장됩니다. 중복 투표는 자동으로 무시됩니다.

### Subject — 도메인 분리

서명 시에는 무엇에 서명하는지를 나타내는 `Subject`를 사용합니다. 이는 서로 다른 메시지 타입의 서명이 혼용되는 것을 방지합니다:

```rust
pub enum Subject<'a, D: Digest> {
    Notarize { proposal: &'a Proposal<D> },  // "나는 이 블록에 찬성합니다"
    Nullify  { round: Round },               // "나는 이 뷰를 무효화합니다"
    Finalize { proposal: &'a Proposal<D> },  // "나는 이 블록을 확정합니다"
}
```

Notarize 서명으로 Finalize를 위장하는 공격이 불가능합니다.

---

## 3. Simplex 합의 흐름 상세

### 정상 경로 (Happy Path)

```
검증자 A (리더)          검증자 B, C, D
      │                        │
  ① Propose(블록, parent)──────►│  채널 0 (pending)
      │                        │
      │◄──── Notarize(블록) ───│  각자 블록 검증 후 서명 전송
      │──── Notarize(블록) ───►│  리더도 자신의 투표 브로드캐스트
      │                        │
      │  [VoteTracker: notarize 수 ≥ Q-쿼럼]
      │                        │
  ② Notarization(cert)─────────►│  채널 1 (recovered): 집계 인증서
      │◄──── Notarization ────│
      │                        │
      │◄──── Finalize(블록) ──│  각자 인증서 확인 후 finalize 투표
      │──── Finalize(블록) ───►│  채널 0 (pending)
      │                        │
      │  [VoteTracker: finalize 수 ≥ Q-쿼럼]
      │                        │
  ③ Finalization(cert)──────────►│  채널 1 (recovered): 최종 확정
      │                        │
      └─ 블록 확정 ✓ ──────────┘  Reporter에 Activity::Finalization 전달
```

**단계 ①: Propose**
- RoundRobin 선출로 해당 뷰의 리더 결정
- 리더가 `Automaton::propose(Context)` 호출 → 블록 페이로드 획득
- `Proposal { round, parent, payload }` 생성 및 브로드캐스트

**단계 ②: Notarize → Notarization**
- 각 검증자가 제안 수신 후 `Automaton::verify(Context, payload)` 호출
- 유효하면 `Notarize::sign(scheme, proposal)` → 서명된 투표 생성
- 투표를 전체에 브로드캐스트
- Q-쿼럼 달성 시 `Notarization::from_notarizes(scheme, votes)` → 인증서 집계
- Notarization 인증서 브로드캐스트

**단계 ③: Finalize → Finalization**
- Notarization 인증서 수신 후 `Finalize::sign(scheme, proposal)` → finalize 투표
- Q-쿼럼 달성 시 `Finalization::from_finalizes(scheme, votes)` → 인증서 집계
- `Relay::broadcast(view, payload)` → 확정된 블록을 Application에 전달
- `Reporter::report(Activity::Finalization(...))` → 모든 구독자에게 알림

### 타임아웃 경로 (Nullify Path)

```
검증자 A (리더)          검증자 B, C, D
      │                        │
  leader_timeout 경과           │
      │                        │
      │◄──── Nullify(뷰N) ────│  각자 타임아웃 후 nullify 투표 전송
      │──── Nullify(뷰N) ─────►│
      │                        │
      │  [VoteTracker: nullify 수 ≥ L-쿼럼]
      │                        │
      └─ Nullification(cert)───►│  채널 1 (recovered)
                                │
                                └─ View N+1로 전진, 새 리더 선출
```

L-쿼럼(≥40%)만으로도 뷰를 진행할 수 있습니다. Q-쿼럼(≥80%)을 요구하면 단 한 명의 악의적 노드가 진행을 막을 수 있기 때문입니다.

### Certification — 특수 단계

Notarization이 발행된 후 Finalization이 오기 전에 `Certification` 단계가 존재합니다:

```rust
// Activity enum에서
Activity::Certification(Notarization<S, D>)   // Finalization의 선행 조건
```

Certification은 "내가 이 Notarization을 받았고, Finalize 투표를 보낼 것"임을 확인하는 신호입니다. 주로 느린 검증자가 인증서를 동기화하는 데 사용됩니다.

### 뷰 스킵 (Skip)

연속으로 `skip_timeout`개의 뷰가 nullified되면, 엔진은 더 앞선 뷰로 점프합니다. 이는 네트워크가 분리된 상태에서도 재연결 시 빠르게 동기화되도록 합니다.

---

## 4. 쿼럼 수학

검증자 n명, 비잔틴 f명이라고 할 때:

```
Q-쿼럼 임계값 = ⌈(2n + 3) / 3⌉   (약 2/3 이상 = 2f+1 이상)
L-쿼럼 임계값 = f + 1             (비잔틴보다 하나 더)
```

**4명 검증자 (n=4, f=0) 기준**:

| 쿼럼 | 계산 | 필요 표 수 |
|---|---|---|
| Q-쿼럼 | ⌈(8+3)/3⌉ = ⌈3.67⌉ | 4표 (전원) |
| L-쿼럼 | 0 + 1 | 1표 |

4명에서 f=0이므로 Q-쿼럼은 전원 동의가 필요합니다. 비잔틴 허용을 위해서는 최소 5명(f=1)이 필요합니다:

**5명 검증자 (n=5, f=1) 기준**:

| 쿼럼 | 계산 | 필요 표 수 |
|---|---|---|
| Q-쿼럼 | ⌈(10+3)/3⌉ = ⌈4.33⌉ | 5표 (전원) |
| L-쿼럼 | 1 + 1 | 2표 |

**7명 (n=7, f=2)**:

| 쿼럼 | 계산 | 필요 표 수 |
|---|---|---|
| Q-쿼럼 | ⌈(14+3)/3⌉ = ⌈5.67⌉ | 6표 |
| L-쿼럼 | 2 + 1 | 3표 |

Commonware 소스코드에서 이 임계값은 `N3f1` 타입으로 표현됩니다:
```rust
// Notarization 집계 시 2f+1 쿼럼을 요구
scheme.assemble::<_, N3f1>(iter.map(|n| n.attestation.clone()), strategy)?
```

---

## 5. 안전성 증명 직관

### 왜 두 블록이 같은 뷰에서 확정될 수 없는가?

같은 뷰에서 두 개의 다른 블록 X, Y가 확정되려면:
- X를 Finalize한 Q-쿼럼 집합 S₁
- Y를 Finalize한 Q-쿼럼 집합 S₂

n=7, f=2라면 Q-쿼럼은 6명입니다. S₁과 S₂는 각각 6명이고, 전체 7명 중 비잔틴이 최대 2명이므로:

```
|S₁ ∩ S₂| ≥ |S₁| + |S₂| - n = 6 + 6 - 7 = 5명이 겹침
겹치는 5명 중 비잔틴은 최대 2명이므로, 정직한 노드 3명이 X와 Y 모두에 서명한 셈
→ 정직한 노드는 하나의 뷰에서 하나의 블록에만 서명 → 모순
```

따라서 두 개의 서로 다른 Finalization이 같은 뷰에서 발생할 수 없습니다.

### Notarize와 Finalize를 분리하는 이유

PBFT/Tendermint 계열과 달리 Simplex는 **Notarize(공증)**와 **Finalize(확정)** 단계를 완전히 분리합니다.

- Notarize는 "이 블록이 유효하다"는 동의
- Finalize는 "Notarization 인증서를 보았고, 이것이 이 뷰의 최종 블록이다"는 확정

이 분리 덕분에 Tendermint의 복잡한 "락(lock)" 메커니즘이 필요 없습니다. 각 단계가 독립적인 인증서를 생성하므로 안전성 증명이 훨씬 단순해집니다.

---

## 6. 다른 BFT 알고리즘과의 비교

### 알고리즘별 특성 비교

| 알고리즘 | 연도 | 메시지 복잡도 | 안전 임계값 | 단계 수 | 특징 |
|---|---|---|---|---|---|
| **PBFT** | 1999 | O(n²) | < 1/3 | 3 (Pre-prepare→Prepare→Commit) | 최초 실용 BFT, 느림 |
| **Tendermint** | 2014 | O(n²) | < 1/3 | 2 (Prevote→Precommit) | 블록체인 최적화, 락 메커니즘 |
| **HotStuff** | 2018 | O(n) | < 1/3 | 3 (선형) | 리더 집계, Facebook Diem 기반 |
| **Simplex** | 2023 | O(n) | < 1/3 | 2+1 (Notarize+Finalize+Nullify) | 단순함 극대화, 분리된 인증서 |
| **Minimmit** | 2025 | O(n) | **< 1/5** | **2** (Notarize+Nullify) | 빠른 피날리티, 더 강한 요구조건 |

### Simplex가 PBFT보다 나은 점

PBFT의 핵심 문제는 O(n²) 메시지 복잡도입니다. 검증자가 100명이면 10,000개의 메시지가 오가야 합니다.

```
PBFT Prepare 단계: 모든 검증자가 모든 검증자에게 메시지 전송
→ n × n = O(n²)

Simplex: 검증자가 리더에게 투표, 리더가 집계 인증서를 브로드캐스트
→ n + n = O(n)
```

### Simplex가 Tendermint보다 단순한 점

Tendermint는 "락(lock)" 개념이 있어 검증자가 특정 블록에 "잠금"됩니다. 뷰 변경 시 락을 해제하는 복잡한 로직이 필요하고, 이 로직의 안전성 증명이 어렵습니다.

Simplex는 각 단계가 완전히 독립적인 인증서를 생성하므로 뷰 변경 시 특별한 상태 이전이 필요 없습니다.

---

## 7. Minimmit: Simplex와 무엇이 다른가

### 핵심 차이: Finalize 단계 제거

Simplex의 가장 큰 비용은 두 번의 Q-쿼럼 라운드를 기다리는 것입니다. Minimmit의 핵심 아이디어는:

> **"Notarization 인증서 자체가 곧 Finalization이다."**

```
Simplex:
  View N → Propose → Notarize(Q-쿼럼) → Finalize(Q-쿼럼) → 확정
            ↑ 1라운드 대기              ↑ 2라운드 대기

Minimmit:
  View N → Propose → Notarize(Q-쿼럼) → 즉시 확정
            ↑ 1라운드 대기만으로 끝
```

### 왜 Finalize 단계가 없어도 안전한가?

Simplex에서 Finalize 단계가 필요한 이유는 **두 개의 서로 다른 Notarization이 같은 뷰에서 존재할 수 있기 때문**입니다. Notarize(Q-쿼럼)가 달성됐다 해도, 비잔틴 노드가 일부 정직한 노드에는 A 블록의 Notarization을, 나머지에는 B 블록의 Notarization을 보여줄 수 있습니다.

Minimmit은 **Q-쿼럼 임계값을 80%(4f+1/5n)**으로 높여 이 문제를 해결합니다:

```
n=10, f=1 (Simplex 기준)  vs  n=10, f=1 (Minimmit 기준)

Simplex Q-쿼럼: ≥ 7표 (70%)
→ 7표짜리 Notarization A와 7표짜리 Notarization B가 공존 가능
   (A∩B ≥ 7+7-10=4, 비잔틴 3명이면 정직 1명만 겹침 → 불안전)

Minimmit Q-쿼럼: ≥ 9표 (80%)
→ A∩B ≥ 9+9-10=8, 비잔틴이 최대 2명이므로 정직 6명이 겹침
→ 정직한 노드는 한 블록에만 서명하므로 두 Notarization이 공존 불가
→ Notarization = Finalization이 안전함
```

### 단계별 비교 다이어그램

```
┌─────────────────────────────────────────────────────────────────────┐
│  Simplex                          Minimmit                          │
│                                                                     │
│  View N                           View N                            │
│   ┌──────────┐                     ┌──────────┐                     │
│   │ Propose  │                     │ Propose  │                     │
│   └────┬─────┘                     └────┬─────┘                     │
│        │ 리더 → 전체 브로드캐스트         │ 리더 → 전체 브로드캐스트         │
│   ┌────▼─────┐                     ┌────▼─────┐                     │
│   │ Notarize │ ← Q-쿼럼 ≥2f+1      │ Notarize │ ← Q-쿼럼 ≥4f+1      │
│   │          │   (~67%)            │          │   (~80%)            │
│   └────┬─────┘                     └────┬─────┘                     │
│        │ Notarization 인증서            │ Notarization 인증서         │
│   ┌────▼─────┐                          │                           │
│   │ Finalize │ ← Q-쿼럼 ≥2f+1          │ ← 이 단계가 없음!           │
│   │          │   (~67%)                │                           │
│   └────┬─────┘                     ┌────▼─────┐                     │
│        │                           │  확 정   │                     │
│   ┌────▼─────┐                     │  완 료   │                     │
│   │  확 정   │                     └──────────┘                     │
│   │  완 료   │                                                       │
│   └──────────┘                                                       │
│                                                                     │
│  피날리티: ~600ms (4노드)            피날리티: ~250ms (130ms 블록타임)  │
└─────────────────────────────────────────────────────────────────────┘
```

### Nullify 경로 비교

```
Simplex Nullify:
  leader_timeout → Nullify 투표 (L-쿼럼=f+1) → Nullification → 다음 뷰

Minimmit Nullify:
  leader_timeout → Nullify 투표 (L-쿼럼=2f+1) → Nullification → 다음 뷰
  ※ Minimmit은 L-쿼럼도 더 높음 (활성성이 약간 더 어려움)
```

### 트레이드오프 요약

| | Simplex | Minimmit |
|---|---|---|
| **피날리티 속도** | 느림 (2 라운드) | **빠름 (1 라운드)** |
| **안전 조건** | < 1/3 비잔틴 | **< 1/5 비잔틴** (더 엄격) |
| **구현 복잡도** | 낮음 | 낮음 (더 단순) |
| **Commonware 구현 상태** | ✅ BETA | ❌ 미구현 (논문/벤치만 존재) |
| **적합한 환경** | 허가형, 일반 검증자 세트 | **신뢰도 높은 검증자 세트** |

Minimmit은 속도가 빠르지만, 검증자의 20% 이상이 악의적이면 안전성이 깨집니다. Simplex는 33%까지 허용합니다. 어느 것이 적합한지는 검증자 세트의 신뢰도에 달려 있습니다.

---

## 8. Commonware API 레퍼런스

### 엔진 타입과 설정

```rust
// 합의 엔진 제네릭 파라미터
Engine<
    deterministic::Context,  // 런타임 컨텍스트
    ed25519::Scheme,          // 암호화 스킴 (서명/검증)
    RoundRobin,               // 리더 선출기
    Sha256Digest,             // 블록 페이로드 다이제스트 타입
    Sequential,               // 병렬화 전략
>

// Config 구조체 (모든 필드 필수)
config::Config {
    blocker,             // Oracle::control(validator) — 네트워크 흐름 제어
    scheme,              // ed25519::Scheme — 서명 및 인증서 검증
    elector,             // RoundRobin — 리더 선출
    automaton,           // impl Automaton — 블록 제안/검증 (애플리케이션 인터페이스)
    relay,               // impl Relay — 확정된 블록을 앱에 전달
    reporter,            // impl Reporter — 합의 활동 전체 관찰/기록
    partition,           // String — 저장소 네임스페이스 (검증자마다 고유해야 함)
    mailbox_size: 1024,
    epoch: Epoch::new(0),
    leader_timeout:        Duration::from_secs(1),  // 리더 응답 대기 시간
    certification_timeout: Duration::from_secs(2),  // 인증서 수집 대기 시간
    timeout_retry:         Duration::from_secs(10), // 타임아웃 재시도 간격
    fetch_timeout:         Duration::from_secs(1),  // 블록 fetch 대기 시간
    activity_timeout:  Delta::new(10),   // 검증자 비활성 판정까지 뷰 수
    skip_timeout:      Delta::new(5),    // 연속 nullify 후 뷰 스킵 임계값
    fetch_concurrent: 1,
    replay_buffer:  NZUsize!(1024 * 1024),
    write_buffer:   NZUsize!(1024 * 1024),
    page_cache:     CacheRef::from_pooler(&ctx, NZU16!(1024), NZUsize!(10)),
    strategy: Sequential,
}
```

### 엔진 시작

```rust
let engine = Engine::new(ctx.with_label("engine"), engine_cfg);

// 3개의 p2p 채널 쌍:
//   pending   = 채널 0 — Vote 메시지 (Notarize, Nullify, Finalize)
//   recovered = 채널 1 — Certificate 메시지 (Notarization, Nullification, Finalization)
//   resolver  = 채널 2 — 블록 fetch 요청/응답
engine.start(pending, recovered, resolver);
```

### 블록 확정 관찰

```rust
// Reporter는 Monitor<Index = View> 트레이트를 구현
let (mut latest, mut monitor): (View, Receiver<View>) = reporter.subscribe().await;
while latest.get() < target {
    latest = monitor.recv().await.unwrap();
    println!("finalized view {}", latest.get());
}
```

`Reporter` mock은 내부 상태를 직접 조회할 수 있습니다:
```rust
reporter.finalizations.lock()   // HashMap<View, Finalization<...>> — 확정된 블록들
reporter.notarizations.lock()   // HashMap<View, Notarization<...>> — 공증된 블록들
reporter.nullifications.lock()  // HashMap<View, Nullification<...>> — 무효화된 뷰들
reporter.faults.lock()          // HashMap<PublicKey, ...> — 감지된 비잔틴 증거
reporter.invalid.lock()         // usize — 수신한 유효하지 않은 메시지 수
```

### Ed25519 인증서 스킴

```rust
// 테스트용 N개의 검증자 키 세트 생성
let fixture = ed25519::fixture(&mut context, namespace, n);
// fixture.participants — Vec<Ed25519PublicKey>, 결정적으로 정렬된 순서
// fixture.schemes      — Vec<ed25519::Scheme>, 검증자별 1개 (개인키 포함)
// fixture.verifier     — ed25519::Scheme (검증 전용, 개인키 없음)
```

`impl_certificate_ed25519!` 매크로가 자동으로 `Scheme::signer()`, `Scheme::verifier()`, `fixture()` 등을 생성합니다. 동일한 매크로로 secp256r1, BLS12-381 등 다른 암호화 스킴도 Simplex에 연결할 수 있습니다.
