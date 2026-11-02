import { describe, expect, it } from "vitest";
import { Keypair } from "@stellar/stellar-sdk";
import { canonical, hashJson, receiptDigest, signReceipt } from "../src/receipt.ts";

const VAULT = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
const ESCROW = "CAILWU67NWQKJ6OKHPLUZDMMTH2CFLF2YCDBOGGGDLQPNR6NCQDXIDUX";

describe("receipt", () => {
  it("digest matches the contract vector (contracts/leash-vault test receipt_digest_matches_ts_vector)", () => {
    const d = receiptDigest(VAULT, ESCROW, "01".repeat(32), "02".repeat(32));
    expect(d.toString("hex")).toBe("e7eb3e14b50d974478a6d3ec5af39297a27c90b6b45ba03799ce7a1aa2056346");
  });

  it("signature verifies with the seller's Stellar key and binds the escrow", () => {
    const kp = Keypair.random();
    const sig = Buffer.from(signReceipt(kp.secret(), VAULT, ESCROW, "01".repeat(32), "02".repeat(32)), "hex");
    expect(kp.verify(receiptDigest(VAULT, ESCROW, "01".repeat(32), "02".repeat(32)), sig)).toBe(true);
    expect(kp.verify(receiptDigest(VAULT, VAULT, "01".repeat(32), "02".repeat(32)), sig)).toBe(false);
  });

  it("canonical JSON sorts keys recursively", () => {
    expect(canonical({ b: 1, a: { d: [2, { z: 1, y: 2 }], c: "x" } })).toBe('{"a":{"c":"x","d":[2,{"y":2,"z":1}]},"b":1}');
    expect(hashJson({ b: 1, a: 2 })).toBe(hashJson({ a: 2, b: 1 }));
  });
});
