# 아키텍처

## 컴포넌트 연결 구조

```
┌─────────────────────────────────────────────────────────────────┐
│  deterministic::Runner  (단일 스레드 비동기 실행기)              │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  simulated::Network  (인-프로세스 p2p, 링크 설정 가능)    │   │
│  │    Oracle → 검증자별 Control → 3개의 채널 쌍              │   │
│  └───────────────────────┬──────────────────────────────────┘   │
│                          │ (Sender, Receiver) × 3               │
│  ┌───────────────────────▼──────────────────────────────────┐   │
│  │  검증자 N (레이블이 붙은 서브 컨텍스트로 생성)            │   │
│  │                                                          │   │
│  │  ┌─────────────┐   propose/verify   ┌────────────────┐  │   │
│  │  │  Engine     │◄──────────────────►│  Application   │  │   │
│  │  │  (simplex)  │   relay (블록)     │  (state_machine│  │   │
│  │  │             │                    └────────┬───────┘  │   │
│  │  │             │  활동 보고                  │ relay    │   │
│  │  │             │──────────────────►┌─────────▼───────┐  │   │
│  │  │             │                  │  Reporter        │  │   │
│  │  │             │                  │  (mock)          │  │   │
│  │  └─────────────┘                  └────────┬───────┘  │   │
│  └────────────────────────────────────────────│───────────┘   │
│                                               │ Monitor<View>  │
│  ┌────────────────────────────────────────────▼───────────┐   │
│  │  Finalizer (검증자별)                                    │   │
│  │  – Reporter를 구독                                       │   │
│  │  – REQUIRED_BLOCKS 뷰가 확정될 때까지 대기               │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## 상태 머신 (`state_machine.rs`)

Phase 2에서 mock 애플리케이션을 실제 상태 머신으로 교체했습니다.

### 블록/트랜잭션 타입

```
Transaction { from: u8, to: u8, amount: u64 }
  └─ 계좌 인덱스(0~3) 간 토큰 이체 하나를 표현

Block { view: u64, parent: Sha256Digest, transactions: Vec<Transaction> }
  └─ 합의 한 뷰에 확정되는 단위. 직렬화: view(8) + parent(32) + tx_count(4) + tx*(10 each)
```

### 계좌 모델

제네시스에서 계좌 0~3을 각각 1,000,000 토큰으로 초기화합니다.

```
계좌 0: 1,000,000 토큰
계좌 1: 1,000,000 토큰
계좌 2: 1,000,000 토큰
계좌 3: 1,000,000 토큰
```

Phase 2에서는 제네시스 상태를 기준으로 트랜잭션 유효성을 검증합니다. 블록 확정 후 잔액 갱신(크로스 블록 상태 추적)은 Phase 3에서 추가합니다.

### Application 액터 패턴

Commonware는 비동기 액터 패턴을 권장합니다. 합의 엔진은 `Clone + Send` 핸들(Mailbox)만 보유하고, 실제 상태는 백그라운드 태스크(Application 액터)에만 존재합니다.

```
┌─────────────────────────┐      mpsc 채널      ┌──────────────────────────────┐
│  Engine (simplex)       │ ──── Message ──────► │  Application 액터 (run loop) │
│                         │                     │                              │
│  automaton: Mailbox     │ ◄─── oneshot 응답 ── │  balances: HashMap<u8, u64>  │
│  relay:     Mailbox     │                     │  pending:  HashMap<D, Bytes>  │
└─────────────────────────┘                     │  seen:     HashMap<D, Bytes>  │
                                                │  waiters:  HashMap<D, Vec<_>> │
                                                └──────────────────────────────┘
```

`Mailbox`는 `Automaton`, `CertifiableAutomaton`, `Relay` 세 트레이트를 구현합니다. 각 메서드는 메시지를 채널로 보내고 oneshot 채널로 응답을 받습니다.

### 검증 로직

`verify()` 호출 시:
1. 블록 내용이 `seen`(다른 검증자에서 수신) 또는 `pending`(자신이 제안)에 있으면 즉시 검증
2. 없으면 `waiters`에 등록 후 대기 — 릴레이에서 블록이 도착하면 깨워서 검증
3. 검증 항목: 뷰 번호 일치, 부모 다이제스트 일치, 트랜잭션 잔액 시뮬레이션, 내용↔다이제스트 해시 일치

## 데이터 플로우

### 블록 제안
1. 엔진이 현재 뷰의 리더를 선출 (RoundRobin)
2. 엔진이 `Automaton::propose()`를 호출
3. Application이 `Block { view, parent, [tx: from→to, amount=1] }` 를 생성하고 SHA-256 해시(=다이제스트)를 반환
4. 엔진이 `pending` 채널을 통해 제안(다이제스트만)을 모든 피어에 브로드캐스트
5. 엔진이 `Relay::broadcast()`를 호출 → Application이 블록 전체 내용을 공유 릴레이로 전송

### Notarization (공증)
1. 피어들이 `pending` 채널로 제안을 수신
2. 엔진이 수신한 각 제안에 대해 `Automaton::verify()`를 호출
3. 유효한 경우, 엔진이 notarize 투표에 서명 후 브로드캐스트
4. Q-쿼럼(≥80%)의 투표 수집 완료 시 Notarization 인증서 생성
5. 인증서를 `recovered` 채널로 브로드캐스트

### Finalization (최종 확정)
1. 검증자들이 Notarization 인증서 수신
2. 엔진이 finalize 투표에 서명 후 브로드캐스트
3. Q-쿼럼의 finalize 투표 수집 완료 시 Finalization 인증서 생성
4. 엔진이 `Relay::broadcast()`를 호출하여 확정된 블록을 Application에 전달
5. 엔진이 `Reporter::report(Activity::Finalization(...))`를 호출하여 관찰자에게 알림
6. Reporter의 Monitor가 모든 구독자(finalizer 태스크들)에게 알림 전송

### 뷰 스킵
`leader_timeout` 내에 notarization이 이루어지지 않으면, 검증자들이 **Nullify** 투표를 전송합니다.
L-쿼럼의 nullify 수집 → Nullification 인증서 → 블록 확정 없이 뷰 진행

이것이 실행 결과에서 뷰 번호 간격(view 1, 4, 8이 nullified)이 생기는 이유입니다.

## 공유 Relay (`relay::Relay`)

`relay::Relay<Sha256Digest, Ed25519PublicKey>`는 모든 검증자 Application 액터가 공유하는 인-메모리 pub/sub 버스입니다. 제안자가 `broadcast()`로 블록 내용을 전송하면, 나머지 검증자들의 `broadcast_rx` 채널로 `(digest, Bytes)` 쌍이 전달됩니다. 이는 실제 p2p 네트워크의 블록 전파(가십) 레이어를 시뮬레이션합니다.

- P2P 채널(`pending`, `recovered`, `resolver`)은 합의 **메시지**(투표, 인증서)를 전달
- 공유 Relay는 블록 **내용**(Transaction 목록 전체)을 전달

## P2P 채널 구조

각 검증자는 네트워크 Oracle에 3개의 채널 쌍을 등록합니다:

| ID | 이름 | 내용 |
|---|---|---|
| 0 | pending (vote) | `Vote` 및 `Nullify` 메시지 |
| 1 | recovered (cert) | `Notarization` 및 `Nullification` 인증서 |
| 2 | resolver | 블록 fetch 요청 및 응답 |

시뮬레이션 네트워크는 링크별로 지연(latency), 지터(jitter), 성공률(success_rate)을 설정할 수 있어 네트워크 파티션 및 저하된 연결 환경 테스트에 적합합니다.

## 런타임 모델

`deterministic::Runner`는 결정적 스케줄러를 사용하는 단일 스레드에서 모든 태스크를 실행합니다. 동일한 RNG 시드(`deterministic::Config::new()`는 고정된 기본 시드 사용)가 주어지면 실행이 완전히 재현 가능합니다. 즉, 실행할 때마다 동일한 순서로 뷰가 확정됩니다.

---

## 이더리움과의 비교

### 합의 아키텍처 비교

이더리움(post-Merge)과 Krypto의 구조적 차이를 레이어별로 비교합니다.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  이더리움 (post-Merge, PoS)                                                  │
│                                                                              │
│  ┌─────────────────────────┐    Engine API    ┌──────────────────────────┐  │
│  │  합의 레이어 (CL)        │◄────────────────►│  실행 레이어 (EL)         │  │
│  │  Lighthouse / Prysm     │                  │  go-ethereum / Reth      │  │
│  │                         │                  │                          │  │
│  │  - BeaconChain          │                  │  - EVM 실행              │  │
│  │  - 검증자 관리 (~1M명)   │                  │  - 상태 트리 (MPT)        │  │
│  │  - Casper FFG (finality)│                  │  - mempool               │  │
│  │  - LMD-GHOST (fork 선택)│                  │  - devp2p 네트워킹        │  │
│  └──────────────┬──────────┘                  └──────────────────────────┘  │
│                 │ libp2p (gossipsub)                                         │
│                 │ 전 세계 ~500k 검증자 노드                                   │
└──────────────────────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────────────────────┐
│  Krypto (현재, Phase 2)                                                      │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │  단일 노드 바이너리                                                   │    │
│  │                                                                     │    │
│  │  합의(Simplex) + 실행(state_machine) + 네트워킹(simulated) 통합     │    │
│  │  검증자 4명, 인-프로세스, 계좌 잔액 + 토큰 이체 블록                 │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────────────┘
```

이더리움은 합의(CL)와 실행(EL)이 **별도 프로세스**로 Engine API(JSON-RPC)를 통해 통신합니다. Krypto는 현재 합의와 실행이 하나의 프로세스에 통합되어 있으며, Phase 3에서 revm을 붙이면 구조가 이더리움과 유사해집니다.

---

### 합의 단위 비교: 슬롯/에포크 vs 뷰

이더리움의 합의 시간 단위와 Simplex의 뷰를 대응시켜 비교합니다.

| 개념 | 이더리움 | Krypto (Simplex) |
|---|---|---|
| **기본 합의 단위** | 슬롯 (Slot, 12초) | 뷰 (View, 가변) |
| **상위 시간 단위** | 에포크 (Epoch, 32슬롯 = 6.4분) | 에포크 (Epoch, 검증자 세트 교체 단위) |
| **블록 제안자** | 슬롯마다 1명 무작위 선출 | 뷰마다 1명 RoundRobin 선출 |
| **투표자** | 슬롯마다 위원회(Committee) 배정 | 모든 검증자 |
| **투표 메시지** | Attestation | Notarize / Finalize / Nullify |
| **집계 인증서** | AggregatedAttestation (BLS) | Notarization / Finalization |
| **피날리티** | Casper FFG, ~12-15분 (2 에포크) | ~600ms (4노드, 10ms 링크) |
| **슬롯/뷰 스킵** | 없음 (빈 슬롯은 missed block) | Nullification으로 스킵 |

#### 슬롯 vs 뷰의 핵심 차이

이더리움의 **슬롯**은 시계 기반으로 12초마다 무조건 진행됩니다. 블록이 없으면 "missed slot"으로 기록되고 체인은 그냥 앞으로 나아갑니다.

Simplex의 **뷰**는 시계 기반이 아닙니다. 리더가 응답하면 빠르게 끝나고, 응답이 없으면 `leader_timeout` 후 Nullification으로 스킵됩니다. 즉 뷰의 실제 소요 시간은 네트워크 상황에 따라 달라집니다.

---

### 합의 흐름 비교: Attestation vs Notarize+Finalize

```
이더리움 슬롯 내 흐름 (12초)
─────────────────────────────────────────────────────
0s      4s      8s      12s
│       │       │        │
Propose Attest  Aggr.   다음 슬롯
        투표    전파

  1. Proposer가 ExecutionPayload 포함 BeaconBlock 생성
  2. 위원회 검증자들이 Attestation(BLS 서명) 전송
  3. Aggregator가 AggregatedAttestation 수집
  4. 다음 슬롯에서 집계된 투표가 on-chain에 포함
  5. 2 에포크(~12분) 후 Casper FFG로 finalized


Simplex 뷰 내 흐름 (~수백ms)
─────────────────────────────────────────────────────
0ms     ~10ms         ~20ms         ~30ms
│         │              │             │
Propose  Notarize     Notarization  Finalize
         투표 수집    인증서 발행   투표 수집
                                      │
                                   ~40ms
                                      │
                                   Finalization
                                   인증서 발행 → 즉시 확정
```

이더리움의 Attestation은 "이 블록이 올바르다"는 하나의 투표로 Notarize + Finalize 역할을 동시에 하지만, 실제 피날리티는 Casper FFG가 2 에포크 후에 체크포인트를 확정합니다. Simplex는 한 뷰 안에서 두 번의 Q-쿼럼을 달성하는 즉시 확정됩니다.

---

### 검증자 세트 비교

| | 이더리움 | Krypto (현재) |
|---|---|---|
| **검증자 수** | ~1,000,000명 | 4명 (로컬 시뮬레이션) |
| **진입 조건** | 32 ETH 스테이킹 | 코드에 하드코딩 |
| **선출 방식** | RANDAO 기반 의사난수 | RoundRobin |
| **슬래싱** | 이중 서명 시 ETH 소각 | Reporter.faults에 기록 (미구현) |
| **보상** | 블록 수수료 + 이자 | 없음 (미구현) |

---

### 네트워킹 비교

| | 이더리움 | Krypto (현재) |
|---|---|---|
| **프로토콜** | libp2p (gossipsub, req/resp) | commonware-p2p simulated |
| **P2P 레이어** | 실제 TCP/IP, 전 세계 노드 | 인-프로세스 채널 |
| **블록 전파** | gossipsub 브로드캐스트 | simulated::Network (Oracle 제어) |
| **지연 시뮬레이션** | 실제 네트워크 환경 | Link { latency, jitter, success_rate } |
| **블록 sync** | snap sync, full sync | resolver 채널 (fetch) |

이더리움의 gossipsub은 토픽 기반 pub/sub으로 블록과 attestation을 전파합니다. Commonware의 simulated Network는 Oracle이 링크를 직접 제어하므로 네트워크 파티션, 패킷 손실, 지연 등을 코드에서 정밀하게 재현할 수 있습니다.

---

### 피날리티 모델 비교

이더리움과 Simplex의 피날리티는 근본적으로 다른 모델을 사용합니다.

```
이더리움 피날리티 (Casper FFG):
─────────────────────────────────────────────────────
에포크 N    에포크 N+1    에포크 N+2
    │            │             │
  Justify     Finalize
  (N-1 체크포인트에  (N-2 체크포인트 확정)
   2/3+ 투표)

  - 에포크마다 체크포인트(첫 번째 슬롯) 생성
  - 2/3+ 검증자가 체크포인트에 투표 → Justified
  - 연속 2개 Justified → Finalized
  - 최소 2 에포크 = 12.8분 소요
  - 단 1/3+ 이 공격하면 finality 방해 가능

Simplex 피날리티:
─────────────────────────────────────────────────────
  - 뷰 내에서 Q-쿼럼 Notarization + Finalization 즉시 확정
  - 단일 뷰 단위 = 수백ms
  - 확정된 블록은 영구 불변 (롤백 불가)
  - 단 1/3+ 비잔틴이면 safety 위반 가능 (이더리움과 동일)
```

이더리움의 "probabilistic finality" (충분한 블록이 쌓이면 실질적으로 안전)와 달리, Simplex는 **결정론적 즉시 피날리티**를 제공합니다. 블록이 확정되는 순간 롤백이 수학적으로 불가능합니다.
