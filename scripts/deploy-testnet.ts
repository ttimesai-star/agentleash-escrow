// npm run deploy:testnet — upload both wasm, deploy LeashVault + LeashResolver (bound to each other),
// allow-list the seller, fund the Vault with testnet USDC. Writes deployments/testnet.json.
import { randomBytes } from "node:crypto";
import { loadKeys, stellar, rawPkHex, writeDeploy, USDC_SAC, TW_WASM_HASH, TW_FEE, expert } from "./lib.ts";

const WASM_DIR = process.env.WASM_DIR ?? "target/wasm32v1-none/release";
const UNIT = 10_000_000n;
const LIMIT = 25n * UNIT; // per window
const PERIOD = 3600; // seconds
const MAX_PER_CALL = 10n * UNIT;
const FUND = 60n * UNIT;

const k = loadKeys();
const txs: Record<string, string | undefined> = {};

const up = (name: string) => {
  const r = stellar(["contract", "upload", "--wasm", `${WASM_DIR}/${name}.wasm`], k.owner.secret);
  txs[`upload_${name}`] = r.tx;
  return r.out;
};
// VAULT_WASM_HASH / RESOLVER_WASM_HASH: deploy code already uploaded to testnet (no local build; used by npm run try).
const resolverHash = process.env.RESOLVER_WASM_HASH || up("leash_resolver");
const vaultHash = process.env.VAULT_WASM_HASH || up("leash_vault");

// The resolver address is fixed in advance by (deployer, salt), so each contract can be built knowing the other.
const salt = randomBytes(32).toString("hex");
const resolver = stellar(["contract", "id", "wasm", "--salt", salt], k.owner.secret).out;

const validUntil = Math.floor(Date.now() / 1000) + 30 * 86400;
const v = stellar(
  [
    "contract", "deploy", "--wasm-hash", vaultHash, "--",
    "--owner", k.owner.public,
    "--agent", k.agent.public,
    "--agent_valid_until", String(validUntil),
    "--token", USDC_SAC,
    "--tw_wasm_hash", TW_WASM_HASH,
    "--tw_fee", TW_FEE,
    "--resolver", resolver,
    "--limit", String(LIMIT),
    "--period_secs", String(PERIOD),
    "--max_per_call", String(MAX_PER_CALL),
  ],
  k.owner.secret,
);
const vault = v.out;
txs.deploy_vault = v.tx;

const r = stellar(["contract", "deploy", "--wasm-hash", resolverHash, "--salt", salt, "--", "--vault", vault], k.owner.secret);
if (r.out !== resolver) throw new Error(`resolver landed at ${r.out}, expected ${resolver}`);
txs.deploy_resolver = r.tx;

txs.set_payee = stellar(
  ["contract", "invoke", "--id", vault, "--", "set_payee", "--seller", k.seller.public, "--receipt_pk", rawPkHex(k.seller.public)],
  k.owner.secret,
).tx;

txs.fund_vault = stellar(
  ["contract", "invoke", "--id", USDC_SAC, "--", "transfer", "--from", k.owner.public, "--to", vault, "--amount", String(FUND)],
  k.owner.secret,
).tx;

const d = {
  network: "testnet",
  deployed_at: new Date().toISOString(),
  vault,
  resolver,
  vault_wasm_hash: vaultHash,
  resolver_wasm_hash: resolverHash,
  tw_wasm_hash: TW_WASM_HASH,
  tw_fee: TW_FEE,
  token: USDC_SAC,
  owner: k.owner.public,
  agent: k.agent.public,
  seller: k.seller.public,
  limits: { limit: String(LIMIT), period_secs: PERIOD, max_per_call: String(MAX_PER_CALL), agent_valid_until: validUntil },
  txs,
  runs: {},
};
writeDeploy(d);
console.log(`vault    ${vault}  ${expert("contract", vault)}`);
console.log(`resolver ${resolver}  ${expert("contract", resolver)}`);
console.log(JSON.stringify(txs, null, 1));
