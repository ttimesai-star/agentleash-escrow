// Shared helpers for testnet scripts. Keys come only from a JSON file outside the repo (AL_KEYS_FILE);
// a secret is handed to stellar-cli through the STELLAR_ACCOUNT env var, never through argv or the repo.
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync, existsSync, mkdirSync, appendFileSync } from "node:fs";
import { StrKey } from "@stellar/stellar-sdk";

export const NETWORK = "testnet"; // no mainnet switch in these scripts, by design
export const USDC_SAC = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA"; // Circle testnet USDC
export const TW_WASM_HASH = "7c3f7b2af92ad86092708b23babf80f9e1308d7f3ce18b703b9499192ecc934b";
export const TW_FEE = "GA6KH5VWPCHBOEF63X57SPX6T4H366YFFKKGCVDBTXT2N7JVL6PJCK7G"; // fee receiver seen in TW testnet releases
export const DEPLOY_FILE = "deployments/testnet.json";
export const RUN_LOG = "deployments/testnet-runs.jsonl";

export type Key = { public: string; secret: string };
export type Keys = { owner: Key; agent: Key; seller: Key };

export function loadKeys(): Keys {
  const f = process.env.AL_KEYS_FILE;
  if (!f) throw new Error("set AL_KEYS_FILE to the testnet keys JSON (kept outside the repo)");
  const k = JSON.parse(readFileSync(f, "utf8")).keys;
  return { owner: k["al-owner"], agent: k["al-agent"], seller: k["al-seller"] };
}

export class CliError extends Error {
  stderr: string;
  tx?: string;
  constructor(msg: string, stderr: string, tx?: string) {
    super(msg);
    this.stderr = stderr;
    this.tx = tx;
  }
}

/** Run stellar-cli on testnet. `secret` signs as the source account. Returns stdout and the tx hash, if any. */
export function stellar(args: string[], secret?: string): { out: string; tx?: string } {
  const env: NodeJS.ProcessEnv = { ...process.env, STELLAR_NETWORK: NETWORK };
  if (secret) env.STELLAR_ACCOUNT = secret;
  const r = spawnSync(process.env.STELLAR_BIN ?? "stellar", args, { env, encoding: "utf8" });
  const stderr = r.stderr ?? "";
  const tx = stderr.match(/Signing transaction: ([0-9a-f]{64})/)?.[1];
  if (r.status !== 0) {
    throw new CliError(`stellar ${args.slice(0, 4).join(" ")} failed:\n${stderr.slice(-2500)}`, stderr, tx);
  }
  return { out: (r.stdout ?? "").trim(), tx };
}

/** Raw ed25519 public key (hex) of a G... account: the seller signs receipts with its Stellar key. */
export function rawPkHex(g: string): string {
  return Buffer.from(StrKey.decodeEd25519PublicKey(g)).toString("hex");
}

export function readDeploy(): any {
  if (!existsSync(DEPLOY_FILE)) throw new Error(`${DEPLOY_FILE} missing: run npm run deploy:testnet`);
  return JSON.parse(readFileSync(DEPLOY_FILE, "utf8"));
}

export function writeDeploy(d: unknown) {
  mkdirSync("deployments", { recursive: true });
  writeFileSync(DEPLOY_FILE, JSON.stringify(d, null, 2) + "\n");
}

export function logRun(rec: Record<string, unknown>) {
  mkdirSync("deployments", { recursive: true });
  appendFileSync(RUN_LOG, JSON.stringify({ ts: new Date().toISOString(), ...rec }) + "\n");
}

export const expert = (kind: "tx" | "contract" | "account", id: string) =>
  `https://stellar.expert/explorer/testnet/${kind}/${id}`;
