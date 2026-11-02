import { describe, expect, it } from "vitest";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Keypair } from "@stellar/stellar-sdk";
import { append, readLog, verifyRun, type Entry } from "../src/agentlog.ts";

function run3() {
  const file = join(mkdtempSync(join(tmpdir(), "al-")), "log.jsonl");
  const kp = Keypair.random();
  for (const action of ["policy.check", "pay", "settle"]) {
    append(file, kp.secret(), { agent_id: "demo", run_id: "r1", action, tool: "leash", note: action, input: { a: action }, output: { ok: true } });
  }
  return { log: readLog(file), kp };
}

describe("agentlog (ed25519)", () => {
  it("an untouched run verifies", () => {
    const { log, kp } = run3();
    expect(log.map((e) => e.step)).toEqual([0, 1, 2]);
    expect(verifyRun(log, kp.publicKey())).toBeNull();
  });
  it("an edited step is caught", () => {
    const { log, kp } = run3();
    const bad: Entry[] = log.map((e) => ({ ...e }));
    bad[1].note = "nothing happened";
    expect(verifyRun(bad, kp.publicKey())).toEqual({ step: 1, kind: "edited" });
  });
  it("a deleted step is caught", () => {
    const { log, kp } = run3();
    expect(verifyRun([log[0], log[2]], kp.publicKey())?.step).toBe(1);
  });
  it("another key is caught", () => {
    const { log } = run3();
    expect(verifyRun(log, Keypair.random().publicKey())).toEqual({ step: 0, kind: "foreign_signer" });
  });
});
