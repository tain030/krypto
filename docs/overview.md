# Krypto — 개요

Krypto는 처음부터 직접 만들어보는 블록체인 학습 프로젝트입니다. 현재 구현체는 [Commonware](https://commonware.xyz) 프리미티브를 사용하여 **Simplex BFT 합의 알고리즘**을 실행하는 최소한의 **Layer 1 블록체인**입니다.

## 목표

- 합의, 네트워킹, 상태 머신의 기초를 직접 구현하면서 이해하기
- 이더리움 `EXECUTE` 프리컴파일이 도입되는 [EIP-8079](https://eips.ethereum.org/EIPS/eip-8079) 네이티브 롤업으로의 발전 경로 확보
- 의도적으로 의존성이 강한 프레임워크 사용 지양 (OP Stack, OP Succinct 미사용)

## 기술 스택

| 컴포넌트 | 크레이트 | 역할 |
|---|---|---|
| 합의 | `commonware-consensus` (simplex) | BFT 방식의 블록 순서 결정 |
| 암호화 | `commonware-cryptography` (ed25519) | 검증자 신원 및 서명 |
| 네트워킹 | `commonware-p2p` (simulated) | 로컬 테스트용 인-프로세스 p2p |
| 런타임 | `commonware-runtime` (deterministic) | 비동기 태스크 스케줄러 |
| 실행 | *(현재 mock)* | 블록 제안 및 검증 |

## 로드맵

```
Phase 1 (현재) — Simplex 합의, 4개 시뮬레이션 검증자, mock 애플리케이션
Phase 2        — mock 앱을 실제 상태 머신으로 교체 (계좌 잔액 + revm 기반 EVM)
Phase 3        — 이더리움 정산 레이어 (SNARK 증명 → EIP-8079 EXECUTE 프리컴파일)
```

## 디렉터리 구조

```
krypto/
├── Cargo.toml          워크스페이스 루트
├── node/
│   ├── Cargo.toml      노드 바이너리 의존성
│   └── src/
│       └── main.rs     진입점 — 합의, p2p, 런타임을 연결
└── docs/
    ├── overview.md     이 파일
    ├── consensus.md    Simplex 알고리즘 및 Commonware API 상세
    ├── architecture.md 컴포넌트 연결 구조 및 데이터 플로우
    └── usage.md        빌드 및 실행 방법
```
