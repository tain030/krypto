# 사용법

## 사전 요구사항

- Rust 1.75 이상 (edition 2021)
- Cargo

```sh
# Rust 버전 확인
rustc --version
```

## 빌드

```sh
# 워크스페이스 루트에서 실행
cargo build

# 릴리즈 빌드 (실행 속도 훨씬 빠름)
cargo build --release
```

## 실행

```sh
cargo run
# 또는
cargo run --release
```

### 예상 출력

```
[validator_3] finalized view 0
[validator_0] finalized view 0
[validator_2] finalized view 0
[validator_1] finalized view 0
[validator_1] finalized view 2
...
[validator_3] ✓ reached 10 finalized blocks
[validator_2] ✓ reached 10 finalized blocks
[validator_0] ✓ reached 10 finalized blocks
[validator_1] ✓ reached 10 finalized blocks
All validators finalized 10 blocks. Consensus works!
```

뷰 번호 사이의 간격(예: view 1 스킵, view 4 스킵)은 **정상적인 동작입니다.** 리더가 타임아웃되어 해당 뷰가 nullified된 것으로, Simplex는 블록 없이 다음 뷰로 진행합니다.

## 설정값

모든 조정 가능한 상수는 `node/src/main.rs` 상단에 있습니다:

```rust
const NAMESPACE: &[u8] = b"krypto_l1";   // 합의 네임스페이스 (키 유도에 사용)
const NUM_VALIDATORS: u32 = 4;            // 로컬 클러스터의 검증자 수
const REQUIRED_BLOCKS: u64 = 10;          // 종료 전 확정할 블록 수
```

### 네트워크 파라미터

`main.rs`의 링크 설정 부분:

```rust
let link = Link {
    latency: Duration::from_millis(10),   // 기본 단방향 지연시간
    jitter: Duration::from_millis(1),     // 지연에 추가되는 랜덤 지터
    success_rate: 1.0,                    // 1.0 = 패킷 손실 없음; 0.9 = 10% 손실
};
```

### 엔진 타임아웃

`main.rs`의 `config::Config` 내부:

```rust
leader_timeout:        Duration::from_secs(1),  // 느린 리더 nullify 전 대기 시간
certification_timeout: Duration::from_secs(2),  // notarization 후 인증서 대기 시간
activity_timeout:      Delta::new(10),           // 검증자 오프라인 판정까지 비활성 뷰 수
skip_timeout:          Delta::new(5),            // 뷰 스킵 허용까지 연속 nullify 뷰 수
```

## 결정적 실행

결정적 런타임은 기본적으로 고정된 RNG 시드를 사용하므로, 매 실행마다 **동일한 뷰 시퀀스**가 확정됩니다. 랜덤화하려면:

```rust
// main()에서 executor.start(...) 호출 전에 추가
use commonware_utils::FuzzRng;
let rng_seed: u64 = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos() as u64;
let cfg = deterministic::Config::new()
    .with_rng(Box::new(FuzzRng::new(rng_seed.to_be_bytes().to_vec())));
let executor = deterministic::Runner::new(cfg);
```

## 검증자 수 늘리기

`NUM_VALIDATORS`를 변경하면 됩니다. BFT 안전성 보장을 위해 최소 4개의 검증자가 필요합니다(비잔틴 노드 1개 허용):

| 검증자 수 | 최대 비잔틴 | 최소 정직 |
|---|---|---|
| 4 | 0 (여유분만) | 4 |
| 5 | 1 | 4 |
| 7 | 1 | 6 |
| 10 | 2 | 8 |

Simplex는 활성성을 위해 정직한 검증자 >80%를 요구합니다. 이는 기존 PBFT(>67%)보다 엄격한 조건입니다.

## 상세 트레이싱 활성화

Commonware는 내부적으로 `tracing` 크레이트를 사용합니다. `node/Cargo.toml`에 추가:

```toml
tracing-subscriber = "0.3"
```

`main()` 시작 부분에 추가:

```rust
tracing_subscriber::fmt()
    .with_env_filter("commonware_consensus=debug")
    .init();
```

실행 시 `RUST_LOG=debug` 환경변수를 설정하면 내부 로그를 볼 수 있습니다.
