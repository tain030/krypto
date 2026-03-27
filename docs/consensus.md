# 합의 알고리즘 — Simplex BFT

## 알고리즘 개요

Simplex는 Commonware가 구현한 3단계 BFT 합의 프로토콜입니다. 4개의 검증자와 10ms 시뮬레이션 네트워크 지연 환경에서 약 600ms의 최종성(finality)을 달성합니다.

### 뷰(View)별 단계

```
View N
 │
 ├─ Propose   ── 선출된 리더가 블록 제안을 브로드캐스트
 │
 ├─ Notarize  ── 검증자들이 제안에 투표
 │               Q-쿼럼(≥80% 지분) 달성 시 → Notarization 인증서 발행
 │
 └─ Finalize  ── 검증자들이 공증된 블록을 최종 확정
                 Q-쿼럼의 finalize 투표 수집 시 → Finalization 인증서 발행
```

### 쿼럼 구조

| 쿼럼 | 임계값 | 용도 |
|---|---|---|
| L-쿼럼 | ≥40% | 다음 뷰로 진행 (활성성 보장 — 블록 미확정 시에도 진행) |
| Q-쿼럼 | ≥80% | Notarization / Finalization (안전성 보장) |

리더가 응답하지 않을 경우, 검증자들은 **nullify** 투표를 브로드캐스트합니다. L-쿼럼의 nullify가 수집되면 해당 뷰는 **무효화(nullified)**되고, 블록 확정 없이 View N+1로 넘어갑니다.

### 안전성과 활성성

- **안전성**: 비잔틴 검증자 ≤20% 조건 (Q-쿼럼 임계값 기준)
- **활성성**: 정직한 검증자 ≥60% 온라인 조건 (L-쿼럼 임계값 기준)
- 4개 검증자, 비잔틴 0개 조건: 두 속성 모두 충분한 여유를 가지고 충족

### 리더 선출

`RoundRobin<Sha256>` — 현재 인증서의 SHA-256 해시를 사용하여 뷰 번호 기반의 결정적 라운드 로빈 방식으로 선출합니다.

---

## Commonware Simplex API

### 핵심 타입

```rust
// 합의 엔진 — 컨텍스트 이후 5개의 제네릭 파라미터
Engine<deterministic::Context, ed25519::Scheme, RoundRobin, Sha256Digest, Sequential>

// Config 구조체 (모든 필드 필수)
config::Config {
    blocker,             // Oracle::control(validator) — 네트워크 흐름 제어
    scheme,              // ed25519::Scheme — 서명 및 인증서 검증
    elector,             // RoundRobin — 리더 선출
    automaton,           // impl Automaton — 블록 제안 및 검증
    relay,               // impl Relay — 확정된 블록을 앱에 전달
    reporter,            // impl Reporter — 합의 활동 전체 관찰
    partition,           // String — 스토리지 네임스페이스 (검증자마다 고유)
    mailbox_size: 1024,
    epoch: Epoch::new(0),
    leader_timeout:        Duration::from_secs(1),  // 리더 응답 대기 시간
    certification_timeout: Duration::from_secs(2),  // 인증서 수집 대기 시간
    timeout_retry:         Duration::from_secs(10),
    fetch_timeout:         Duration::from_secs(1),
    activity_timeout:  Delta::new(10),   // 검증자 비활성 판정까지 뷰 수
    skip_timeout:      Delta::new(5),    // 뷰 스킵 허용까지 연속 nullify 수
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
//   pending   = 채널 0 — vote 메시지
//   recovered = 채널 1 — certificate 메시지
//   resolver  = 채널 2 — 블록 fetch / resolver 메시지
engine.start(pending, recovered, resolver);
```

### 블록 확정 관찰

```rust
// reporter::Reporter는 Monitor<Index = View>를 구현
let (mut latest, mut monitor): (View, Receiver<View>) = reporter.subscribe().await;
while latest.get() < target {
    latest = monitor.recv().await.unwrap();
    println!("finalized view {}", latest.get());
}
```

`Reporter` mock은 내부 상태도 직접 조회할 수 있습니다:
```rust
reporter.finalizations.lock()   // HashMap<View, Finalization<...>>
reporter.notarizations.lock()   // HashMap<View, Notarization<...>>
reporter.faults.lock()          // 감지된 비잔틴 증거
reporter.invalid.lock()         // 수신한 유효하지 않은 메시지 수
```

---

## Ed25519 인증서 스킴

검증자는 신원 확인과 서명 모두에 Ed25519를 사용합니다. 스킴은 `commonware-cryptography`의 `impl_certificate_ed25519!` 매크로로 생성됩니다.

```rust
// 테스트용 N개의 검증자 키 세트 생성
let fixture = ed25519::fixture(&mut context, namespace, n);
// fixture.participants — Vec<Ed25519PublicKey>, 결정적으로 정렬된 순서
// fixture.schemes      — Vec<ed25519::Scheme>, 검증자별 1개 (개인키 포함)
// fixture.verifier     — ed25519::Scheme (검증 전용, 개인키 없음)
```
