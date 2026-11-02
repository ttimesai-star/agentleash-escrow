// Seller receipt: ed25519 signature over
//   sha256("ALSH-RCPT-v1" || xdr(ScVal vault) || xdr(ScVal escrow) || request_hash || result_hash)
// — the same bytes LeashVault.settle verifies (contracts/leash-vault/src/lib.rs, receipt_digest).
import { createHash } from "node:crypto";
import { Address, Keypair } from "@stellar/stellar-sdk";

const TAG = Buffer.from("ALSH-RCPT-v1");

export const sha256 = (b: Buffer | string) => createHash("sha256").update(b).digest();

/** Canonical JSON (keys sorted recursively, no spaces) — the result hash is sha256 of this, as in AgentLog. */
export function canonical(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o).sort().map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`).join(",")}}`;
  }
  return JSON.stringify(v);
}

export const hashJson = (v: unknown) => sha256(canonical(v)).toString("hex");

export function receiptDigest(vault: string, escrow: string, requestHash: string, resultHash: string): Buffer {
  return sha256(
    Buffer.concat([
      TAG,
      new Address(vault).toScVal().toXDR(),
      new Address(escrow).toScVal().toXDR(),
      Buffer.from(requestHash, "hex"),
      Buffer.from(resultHash, "hex"),
    ]),
  );
}

export function signReceipt(sellerSecret: string, vault: string, escrow: string, requestHash: string, resultHash: string): string {
  return Keypair.fromSecret(sellerSecret).sign(receiptDigest(vault, escrow, requestHash, resultHash)).toString("hex");
}
