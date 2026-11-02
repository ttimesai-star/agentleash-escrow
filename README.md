# AgentLeash Escrow (Stellar Soroban + Trustless Work)

**Status: prototype, unaudited, testnet only.** There is no mainnet configuration in this repository.

An AI agent pays sellers of paid APIs from a policy vault. The owner sets the limits; the agent can move money only
through `pay`. Every payment goes into a [Trustless Work](https://www.trustlesswork.com) single-release escrow. The
seller is paid only against an ed25519 receipt over the hash of the result it delivered; without a receipt the money
returns to the vault after the deadline.

```
owner  --set_payee / set_limits / revoke_agent / withdraw-->  LeashVault (holds the agent's USDC)
agent  --pay(seller, amount, request_hash, timeout)------->  LeashVault
          policy check -> deploy TW escrow from the pinned wasm hash -> initialize_escrow -> fund_escrow
seller --serves the call, signs receipt(vault, escrow, request_hash, result_hash)
anyone --settle(escrow, result_hash, sig)  (now <= deadline) -> TW approve_milestone + release_funds -> seller
anyone --refund(escrow)                    (now >  deadline) -> TW dispute_escrow -> LeashResolver -> resolve_dispute -> vault
```

Roles in every escrow: `approver = release_signer = platform = LeashVault`, `dispute_resolver = LeashResolver`
(a small contract that obeys only its vault — a TW resolver may not open a dispute itself),
`service_provider = receiver = seller`, `platform_fee = 0`, one milestone.

Receipt: `ed25519(seller_key, sha256("ALSH-RCPT-v1" || xdr(vault) || xdr(escrow) || request_hash || result_hash))`.
The seller's key is recorded in the payment at `pay` time, so rotating or removing the seller later does not change
which key can settle an open payment. `result_hash` is SHA-256 of the canonical JSON of the response.

## Testnet deployment (02.11.2026)

| | |
|---|---|
| LeashVault | [`CA4FEKFU3U5OA3VUWUF4KZQW6NBTKTBYRTWLNHIROFKCHLFKWNQBHM22`](https://stellar.expert/explorer/testnet/contract/CA4FEKFU3U5OA3VUWUF4KZQW6NBTKTBYRTWLNHIROFKCHLFKWNQBHM22) (wasm `0b377167…600f`, 16 976 bytes) |
| LeashResolver | [`CC5O7ALAX3IALML4GD3ASQGB7YEEUFL4KFR5IR5AIHRGUODZHPUSPHIP`](https://stellar.expert/explorer/testnet/contract/CC5O7ALAX3IALML4GD3ASQGB7YEEUFL4KFR5IR5AIHRGUODZHPUSPHIP) (wasm `4aa133de…a684`, 1 403 bytes) |
| Trustless Work escrow wasm (used by hash) | `7c3f7b2af92ad86092708b23babf80f9e1308d7f3ce18b703b9499192ecc934b` — the single-release wasm of TW escrows already live on testnet |
| Token | Circle testnet USDC, SAC `CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA` |
| Limits | 10 USDC per call, 25 USDC per 3600 s window, agent key valid 30 days |

Flows (all transactions on stellar.expert testnet):

| Flow | Escrow | Transactions | Result |
|---|---|---|---|
| release | [`CBXEW22K…7H6K`](https://stellar.expert/explorer/testnet/contract/CBXEW22KM5RJIZTS4LZ2UUC4VPCRCVQ3S5OZLKJGLRDVCSMGA4FI7H6K) | pay [`765cdf4e…17d0`](https://stellar.expert/explorer/testnet/tx/765cdf4eabfa022aad11ec296d250131fc15a12f87f091b841caad09d8a617d0), settle [`1c530f64…7f1b`](https://stellar.expert/explorer/testnet/tx/1c530f64aebb152979bcd97587ba3fdc26111cd37ef178b0385d8fd3b7997f1b) | seller +1.994 of 2 USDC |
| refund (60 s timeout) | [`CCSIXPMH…TDZW`](https://stellar.expert/explorer/testnet/contract/CCSIXPMHDHEDYUGKUFS7JGTHVUB3PTDCG2F24S4JXE4OYJX3WDYXTDZW) | pay [`5731853d…356c`](https://stellar.expert/explorer/testnet/tx/5731853dcf6f288e9e5d546484286e8d7accbbea980ae6651d6b685fbfbe356c), refund [`dced1e2d…e643`](https://stellar.expert/explorer/testnet/tx/dced1e2dd3708b834ced6196059ad732c5d9d4ea1592658e9cdeb0f09da8e643) | vault +0.997 of 1 USDC; a valid receipt after the deadline → `Error(Contract, #11)`, after the refund → `Error(Contract, #10)` |
| forged receipts | [`CAAPCCFW…AYJN`](https://stellar.expert/explorer/testnet/contract/CAAPCCFWJM7JPW4L6EEUT2WECUCFTHL2RKHVAL4V36GOTF45BJSDAYJN) | pay [`fcfb4ff8…0572`](https://stellar.expert/explorer/testnet/tx/fcfb4ff803e1084365049efb9f35e69ad7cee5dde4d69dc19e89827fc2870572), genuine settle [`076b664c…d41f`](https://stellar.expert/explorer/testnet/tx/076b664c74c709b4960f489df44084224239b48f44f076f1d8e3a2ce97c8d41f) | wrong key / other result hash / receipt of another escrow → `Error(Crypto, InvalidInput)`, nothing paid |
| refused | — | revoke [`907e6c57…ba5f`](https://stellar.expert/explorer/testnet/tx/907e6c57ab95eed4e4a75e9d740c59081eb7e10eb4bd20d236b29e397dc6ba5f) | unknown seller `#3`, over max per call `#5`, over window limit `#6`, revoked agent `#2` — the SDK refuses before signing; with `--force-chain` the contract returns the same codes |

All records: [`deployments/testnet.json`](deployments/testnet.json), Vault events exported from RPC
[`deployments/vault-events.jsonl`](deployments/vault-events.jsonl), the agent's signed action log
[`deployments/agentlog.jsonl`](deployments/agentlog.jsonl). `npm run log:verify` → `payments 6: released 3, refunded 3 … verify: OK`.
A refund is counted separately from a release: it is not a completed sale.

## Tests

- `cargo test --workspace` — **40 passed** (Rust, against the real Trustless Work wasm in the Soroban test env;
  authorization is enforced: no blanket auth mocks in the flows, only the agent's or owner's signature on the outer call). Covers every policy refusal, window and
  deadline boundaries, replayed / late / forged receipts, settle after refund and the reverse, seller-opened disputes,
  the resolver refusing anyone but its vault, the seller unable to approve or release its own payment, events.
- `npm test` — **16 passed** (receipt digest vector shared with the Rust test, pre-sign policy, agent log tamper checks).
- CI: GitHub Actions on Ubuntu — tests, clippy `-D warnings`, wasm build, gitleaks over the full history.

## Run it

```sh
sh scripts/fetch-tw-wasm.sh                       # TW escrow wasm from testnet by hash (not committed)
cargo test --workspace
stellar contract build --package leash-resolver && stellar contract build --package leash-vault
npm ci && npm test
export AL_KEYS_FILE=/path/outside/repo/keys.json  # {"keys":{"al-owner":{public,secret},"al-agent":…,"al-seller":…}}
npm run deploy:testnet
npm run demo:release && npm run demo:refund && npm run demo:refused && npm run demo:forged
npm run log:export && npm run log:verify
```

## Known limits (honest list)

- **Fees:** Trustless Work keeps 0.3% on both paths — a refund returns 99.7%. `platform_fee` is 0.
- One escrow contract is deployed per payment (fine on testnet; on mainnet a pooled or multi-release design is the
  next step). The window limit counts money sent out; a refund does not restore it (conservative by design).
- `release_funds` / `resolve_dispute` take the TW fee address from the caller; the vault passes a pinned address
  (`GA6KH5VW…CK7G`, the receiver seen in TW testnet releases).
- Not done yet: the escrows are not registered in the TW indexer/API (needs a TW API key); OpenZeppelin smart accounts
  for the agent key are out of scope.
- No external audit. Do not use with real funds.

License: MIT. The Trustless Work escrow contract is not included; this project only calls it by its on-chain hash.
