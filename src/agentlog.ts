// Agent-side action log in the AgentLog v1 entry format (github.com/ttimesai-star/agentlog-mcp), signed with the
// agent's Stellar ed25519 key instead of EIP-191: sig = ed25519("agentlog:v1:" + entry_hash), signer = G... address.
// Each entry links to the previous one, so a deleted, reordered or edited step breaks verification.
import { readFileSync, appendFileSync, existsSync } from "node:fs";
import { Keypair } from "@stellar/stellar-sdk";
import { canonical, sha256, hashJson } from "./receipt.ts";

export type Entry = {
  v: 1; agent_id: string; run_id: string; step: number; action: string; tool: string; note: string;
  input_hash: string; output_hash: string; prev_entry_hash: string; timestamp: number;
  signer: string; entry_hash: string; sig: string;
};

const ZERO = "0x" + "00".repeat(32);
const body = (e: Omit<Entry, "entry_hash" | "sig">) => canonical(e);

export function readLog(file: string): Entry[] {
  return existsSync(file) ? readFileSync(file, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];
}

export function append(
  file: string, secret: string,
  a: { agent_id: string; run_id: string; action: string; tool: string; note: string; input: unknown; output: unknown },
): Entry {
  const prev = readLog(file).filter((e) => e.run_id === a.run_id).at(-1);
  const kp = Keypair.fromSecret(secret);
  const base = {
    v: 1 as const, agent_id: a.agent_id, run_id: a.run_id, step: prev ? prev.step + 1 : 0, action: a.action, tool: a.tool,
    note: a.note, input_hash: "0x" + hashJson(a.input), output_hash: "0x" + hashJson(a.output),
    prev_entry_hash: prev ? prev.entry_hash : ZERO, timestamp: Date.now(), signer: kp.publicKey(),
  };
  const entry_hash = "0x" + sha256(body(base)).toString("hex");
  const sig = kp.sign(Buffer.from(`agentlog:v1:${entry_hash}`)).toString("hex");
  const e: Entry = { ...base, entry_hash, sig };
  appendFileSync(file, JSON.stringify(e) + "\n");
  return e;
}

/** Verify one run: hashes, links from step 0, signatures by the expected agent key. Returns the first break or null. */
export function verifyRun(entries: Entry[], expectedSigner: string): { step: number; kind: string } | null {
  let prev = ZERO;
  for (let i = 0; i < entries.length; i++) {
    const { entry_hash, sig, ...rest } = entries[i];
    if (rest.step !== i) return { step: i, kind: "deleted_or_reordered" };
    if (rest.prev_entry_hash !== prev) return { step: i, kind: "relinked" };
    if ("0x" + sha256(body(rest)).toString("hex") !== entry_hash) return { step: i, kind: "edited" };
    if (rest.signer !== expectedSigner) return { step: i, kind: "foreign_signer" };
    if (!Keypair.fromPublicKey(rest.signer).verify(Buffer.from(`agentlog:v1:${entry_hash}`), Buffer.from(sig, "hex"))) {
      return { step: i, kind: "bad_signature" };
    }
    prev = entry_hash;
  }
  return null;
}
