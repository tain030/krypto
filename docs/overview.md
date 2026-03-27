# Krypto — Overview

Krypto is a from-scratch blockchain learning project. The current implementation is a minimal **Layer 1 blockchain** running the **Simplex BFT consensus algorithm** using [Commonware](https://commonware.xyz) primitives.

## Goals

- Understand consensus, networking, and state machine fundamentals by building them directly
- Forward path to an [EIP-8079](https://eips.ethereum.org/EIPS/eip-8079) native rollup once the Ethereum `EXECUTE` precompile lands
- Deliberately avoid opinionated frameworks (no OP Stack, no OP Succinct)

## Stack

| Component | Crate | Role |
|---|---|---|
| Consensus | `commonware-consensus` (simplex) | BFT ordering of blocks |
| Cryptography | `commonware-cryptography` (ed25519) | Validator identity & signing |
| Networking | `commonware-p2p` (simulated) | In-process p2p for local testing |
| Runtime | `commonware-runtime` (deterministic) | Async task scheduler |
| Execution | *(mock for now)* | Block proposal & verification |

## Roadmap

```
Phase 1 (current) — Simplex consensus, 4 simulated validators, mock application
Phase 2           — Replace mock app with real state machine (account balances + EVM via revm)
Phase 3           — Ethereum settlement layer (SNARK proofs → EIP-8079 EXECUTE precompile)
```

## Repository Layout

```
krypto/
├── Cargo.toml          workspace root
├── node/
│   ├── Cargo.toml      node binary dependencies
│   └── src/
│       └── main.rs     entry point — wires together consensus, p2p, runtime
└── docs/
    ├── overview.md     this file
    ├── consensus.md    simplex algorithm & Commonware API details
    ├── architecture.md component wiring & data flow
    └── usage.md        how to build and run
```
