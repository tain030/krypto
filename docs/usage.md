# Usage

## Prerequisites

- Rust 1.75+ (edition 2021)
- Cargo

```sh
# Check your Rust version
rustc --version
```

## Build

```sh
# From the workspace root
cargo build

# Release build (much faster execution)
cargo build --release
```

## Run

```sh
cargo run
# or
cargo run --release
```

### Expected Output

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

Gaps in view numbers (e.g., view 1 skipped, view 4 skipped) are **expected and correct** — they represent views that were nullified because the leader timed out. Simplex advances without a block in those views.

## Configuration

All tunable constants are at the top of `node/src/main.rs`:

```rust
const NAMESPACE: &[u8] = b"krypto_l1";   // consensus namespace (key derivation)
const NUM_VALIDATORS: u32 = 4;            // number of validators in the local cluster
const REQUIRED_BLOCKS: u64 = 10;          // how many blocks to finalize before stopping
```

### Network Parameters

Simulated link settings (in `main.rs` near the link setup):

```rust
let link = Link {
    latency: Duration::from_millis(10),   // base one-way latency
    jitter: Duration::from_millis(1),     // random jitter added to latency
    success_rate: 1.0,                    // 1.0 = no packet loss; 0.9 = 10% drop rate
};
```

### Engine Timeouts

Inside `config::Config` in `main.rs`:

```rust
leader_timeout:        Duration::from_secs(1),  // time before nullifying a slow leader
certification_timeout: Duration::from_secs(2),  // time to wait for cert after notarization
activity_timeout:      Delta::new(10),           // views of inactivity before marking validator offline
skip_timeout:          Delta::new(5),            // consecutive nullified views before skipping ahead
```

## Determinism

The deterministic runtime uses a fixed RNG seed by default, so every run produces the **same sequence of finalized views**. To randomize:

```rust
// In main() before executor.start(...)
use commonware_utils::FuzzRng;
let rng_seed: u64 = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos() as u64;
let cfg = deterministic::Config::new()
    .with_rng(Box::new(FuzzRng::new(rng_seed.to_be_bytes().to_vec())));
let executor = deterministic::Runner::new(cfg);
```

## Increasing Validators

Change `NUM_VALIDATORS`. Simplex requires ≥4 validators for BFT guarantees (tolerates 1 Byzantine node):

| Validators | Max Byzantine | Min Honest |
|---|---|---|
| 4 | 0 (margin only) | 4 |
| 5 | 1 | 4 |
| 7 | 1 | 6 |
| 10 | 2 | 8 |

Simplex requires >80% honest validators for liveness, which is stricter than classical PBFT (>67%).

## Enabling Verbose Tracing

Commonware uses the `tracing` crate internally. Add to `node/Cargo.toml`:

```toml
tracing-subscriber = "0.3"
```

And at the start of `main()`:

```rust
tracing_subscriber::fmt()
    .with_env_filter("commonware_consensus=debug")
    .init();
```

Then set `RUST_LOG=debug` when running.
