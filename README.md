# AgentLeash Escrow (Stellar Soroban)

**Status: prototype, unaudited, testnet only.** No mainnet configuration exists in this repository.

An AI agent pays sellers of paid APIs from a policy vault. Every payment goes into a
[Trustless Work](https://www.trustlesswork.com) single-release escrow; the seller is paid only against an ed25519
receipt over the result hash, otherwise the money returns to the vault after the deadline.

Work in progress — testnet contract IDs, transaction hashes and the demo will be listed here.

License: MIT. The Trustless Work escrow wasm is not part of this repository (`scripts/fetch-tw-wasm.sh` fetches it from
testnet by hash for builds and tests).
