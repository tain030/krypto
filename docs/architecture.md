# Architecture

## Component Wiring

```
┌─────────────────────────────────────────────────────────────────┐
│  deterministic::Runner  (single-threaded async executor)        │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  simulated::Network  (in-process p2p, configurable links)│   │
│  │    Oracle → Control per validator → 3 channel pairs      │   │
│  └───────────────────────┬──────────────────────────────────┘   │
│                          │ (Sender, Receiver) × 3               │
│  ┌───────────────────────▼──────────────────────────────────┐   │
│  │  Validator N (spawned as labeled sub-context)            │   │
│  │                                                          │   │
│  │  ┌─────────────┐   propose/verify   ┌────────────────┐  │   │
│  │  │  Engine     │◄──────────────────►│  Application   │  │   │
│  │  │  (simplex)  │   relay (blocks)   │  (mock)        │  │   │
│  │  │             │                    └────────┬───────┘  │   │
│  │  │             │  report activity            │ relay    │   │
│  │  │             │──────────────────►┌─────────▼───────┐  │   │
│  │  │             │                  │  Reporter        │  │   │
│  │  │             │                  │  (mock)          │  │   │
│  │  └─────────────┘                  └────────┬───────┘  │   │
│  └────────────────────────────────────────────│───────────┘   │
│                                               │ Monitor<View>  │
│  ┌────────────────────────────────────────────▼───────────┐   │
│  │  Finalizer (per validator)                              │   │
│  │  – subscribes to Reporter                               │   │
│  │  – waits until REQUIRED_BLOCKS views finalized          │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Data Flow

### Block Proposal
1. Engine elects a leader for the current view (RoundRobin)
2. Engine calls `Automaton::propose()` on the Application mock
3. Application generates a random 32-byte payload and returns it
4. Engine broadcasts the proposal to all peers via the `pending` channel

### Notarization
1. Peers receive the proposal via `pending` channel
2. Engine calls `Automaton::verify()` on each received proposal
3. If valid, engine signs a notarize vote and broadcasts it
4. Once Q-quorum (≥80%) of votes are collected, a Notarization certificate is formed
5. Certificate is broadcast via `recovered` channel

### Finalization
1. Validators receive the Notarization certificate
2. Engine signs a finalize vote and broadcasts it
3. Once Q-quorum of finalize votes are collected, a Finalization certificate is formed
4. Engine calls `Relay::broadcast()` to deliver the finalized block to the Application
5. Engine calls `Reporter::report(Activity::Finalization(...))` to notify observers
6. Reporter's Monitor notifies all subscribers (our finalizer goroutines)

### View Skipping
If no notarization is reached within `leader_timeout`, validators send **Nullify** votes.
L-quorum of nullifies → Nullification certificate → view advances without finalizing a block.
This is why the output shows gaps (views 1, 4, 8 were nullified in the test run).

## Shared Relay (`relay::Relay`)

The `relay::Relay<Sha256Digest, Ed25519PublicKey>` is an in-memory store shared across all validators. When a validator's engine finalizes a block, it calls `Relay::broadcast()`, which stores the block so any validator can retrieve it via `Relay::retrieve()`. This simulates the gossip layer that would exist in a real p2p network.

## P2P Channels

Each validator registers 3 channel pairs with the network Oracle:

| ID | Name | Contents |
|---|---|---|
| 0 | pending (vote) | `Vote` and `Nullify` messages |
| 1 | recovered (cert) | `Notarization` and `Nullification` certificates |
| 2 | resolver | Block fetch requests and responses |

The simulated network applies configurable per-link latency/jitter/success-rate, making it suitable for testing network partition and degraded connectivity scenarios.

## Runtime Model

`deterministic::Runner` runs all tasks on a single thread with a deterministic scheduler. Given the same RNG seed (`deterministic::Config::new()` uses a fixed default seed), the execution is fully reproducible — the same sequence of views will be finalized on every run.
