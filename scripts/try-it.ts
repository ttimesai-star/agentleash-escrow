// npm run try — a full AgentLeash Escrow session on Stellar TESTNET with YOUR OWN fresh keys, no Rust build.
// 1) makes three new testnet keys (owner, agent, seller) and funds them with friendbot (testnet XLM, no value);
// 2) owner and seller get a USDC trustline, owner buys 60 testnet USDC (Circle) for XLM on the testnet DEX;
// 3) deploys your own LeashVault + LeashResolver from the wasm already uploaded to testnet (same code as ours);
// 4) runs two flows: release over HTTP 402 (seller paid against its signed receipt) and refund (no receipt by the
//    60 s deadline); 5) prints the transaction links. Every transaction is signed by the keys made in step 1.
// Keys are written to AL_KEYS_FILE (default ~/.agentleash/testnet-keys.json), never into the repo.
import { Keypair, Horizon, Asset, Operation, TransactionBuilder, Networks, BASE_FEE } from "@stellar/stellar-sdk";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

const HORIZON = "https://horizon-testnet.stellar.org";
const USDC = new Asset("USDC", "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"); // Circle testnet USDC (SAC CBIELTK6…DAMA)
const BUY_USDC = "60"; // the deploy script funds the vault with 60 USDC
const MAX_XLM = "250"; // ceiling for the DEX purchase (about 75 XLM at 02.11 prices)
// Wasm uploaded to testnet on 02.11.2026 from commit 250a428 (see deployments/testnet.json of this repo).
const VAULT_WASM_HASH = "e97a74bf871078e450dea529ec64b218aaab1adfefde49d3c2e2200c48319189";
const RESOLVER_WASM_HASH = "184f12c8929ff4f9441a6ba972284355bbd97239f73c1faf1af0cc4077d7f329";

const keysFile = process.env.AL_KEYS_FILE ?? join(homedir(), ".agentleash", "testnet-keys.json");
const horizon = new Horizon.Server(HORIZON);
const say = (s: string) => console.log(`\n== ${s}`);

type K = { public: string; secret: string };
function loadOrMakeKeys(): Record<string, K> {
  if (existsSync(keysFile)) {
    console.log(`using existing keys from ${keysFile}`);
    return JSON.parse(readFileSync(keysFile, "utf8")).keys;
  }
  const keys: Record<string, K> = {};
  for (const name of ["al-owner", "al-agent", "al-seller"]) {
    const kp = Keypair.random();
    keys[name] = { public: kp.publicKey(), secret: kp.secret() };
  }
  mkdirSync(dirname(keysFile), { recursive: true });
  writeFileSync(keysFile, JSON.stringify({ network: "testnet", keys }, null, 2) + "\n", { mode: 0o600 });
  console.log(`new testnet keys written to ${keysFile} (testnet only; do not reuse on mainnet)`);
  return keys;
}

async function friendbot(g: string) {
  try {
    await horizon.loadAccount(g);
    return; // already exists
  } catch { /* not found: fund it */ }
  const r = await fetch(`https://friendbot.stellar.org/?addr=${g}`);
  if (!r.ok) throw new Error(`friendbot failed for ${g}: HTTP ${r.status} ${(await r.text()).slice(0, 300)}`);
  console.log(`funded ${g} with testnet XLM`);
}

async function submit(secret: string, ops: ReturnType<typeof Operation.changeTrust>[]) {
  const kp = Keypair.fromSecret(secret);
  const acc = await horizon.loadAccount(kp.publicKey());
  const b = new TransactionBuilder(acc, { fee: BASE_FEE, networkPassphrase: Networks.TESTNET });
  for (const op of ops) b.addOperation(op);
  const tx = b.setTimeout(120).build();
  tx.sign(kp);
  const res = await horizon.submitTransaction(tx);
  return res.hash;
}

async function hasTrustline(g: string) {
  const acc = await horizon.loadAccount(g);
  return acc.balances.some((b: any) => b.asset_code === USDC.code && b.asset_issuer === USDC.issuer);
}

async function usdcBalance(g: string) {
  const acc = await horizon.loadAccount(g);
  const b: any = acc.balances.find((x: any) => x.asset_code === USDC.code && x.asset_issuer === USDC.issuer);
  return b ? Number(b.balance) : 0;
}

function node(args: string[], env: NodeJS.ProcessEnv) {
  const r = spawnSync(process.execPath, args, { stdio: "inherit", env: { ...process.env, ...env } });
  if (r.status !== 0) throw new Error(`node ${args.join(" ")} exited with ${r.status}`);
}

// --- preflight: stellar-cli present
const cli = spawnSync(process.env.STELLAR_BIN ?? "stellar", ["--version"], { encoding: "utf8" });
if (cli.status !== 0) throw new Error("stellar-cli not found: install it (https://developers.stellar.org/docs/tools/cli) or set STELLAR_BIN");
console.log(`stellar-cli: ${cli.stdout.split("\n")[0]}`);

say("1/5 keys");
const keys = loadOrMakeKeys();
for (const k of Object.values(keys)) await friendbot(k.public);

say("2/5 testnet USDC");
const owner = keys["al-owner"], seller = keys["al-seller"];
for (const k of [owner, seller]) {
  if (!(await hasTrustline(k.public))) console.log(`trustline ${k.public.slice(0, 6)}… tx ${await submit(k.secret, [Operation.changeTrust({ asset: USDC })])}`);
}
if ((await usdcBalance(owner.public)) < Number(BUY_USDC)) {
  const h = await submit(owner.secret, [
    Operation.pathPaymentStrictReceive({ sendAsset: Asset.native(), sendMax: MAX_XLM, destination: owner.public, destAsset: USDC, destAmount: BUY_USDC, path: [] }),
  ]);
  console.log(`bought ${BUY_USDC} USDC for XLM on the testnet DEX, tx ${h}`);
}

say("3/5 deploy your LeashVault + LeashResolver (code already on testnet, no build)");
const env = { AL_KEYS_FILE: keysFile, VAULT_WASM_HASH, RESOLVER_WASM_HASH };
node(["scripts/deploy-testnet.ts"], env);

say("4/5 release over HTTP 402 (seller paid against its signed receipt)");
node(["scripts/demo.ts", "release402"], env);

say("5/5 refund (seller silent until the 60 s deadline)");
node(["scripts/demo.ts", "refund"], env);

const d = JSON.parse(readFileSync("deployments/testnet.json", "utf8"));
const ex = (kind: string, id?: string) => (id ? `https://stellar.expert/explorer/testnet/${kind}/${id}` : "(missing)");
console.log(`
== done. Your session, every tx signed by your own keys:
vault        ${ex("contract", d.vault)}
deploy       ${ex("tx", d.txs?.deploy_vault)}
release pay  ${ex("tx", d.runs?.release402?.pay_tx)}
release      ${ex("tx", d.runs?.release402?.settle_tx)}
refund pay   ${ex("tx", d.runs?.refund?.pay_tx)}
refund       ${ex("tx", d.runs?.refund?.refund_tx)}

If you want to share it: open an issue titled "Testnet session" in github.com/ttimesai-star/agentleash-escrow
with the vault address above (public testnet data), or reply where you found this. Nothing else is collected.
Note: this run overwrote deployments/testnet.json in YOUR clone; git checkout it to get ours back.`);
