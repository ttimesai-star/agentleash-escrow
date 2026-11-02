import { describe, expect, it } from "vitest";
import { checkPay, type Cfg, type Win } from "../src/policy.ts";

const U = 10_000_000n;
const cfg: Cfg = { revoked: false, agent_valid_until: 2000, limit: 25n * U, period_secs: 3600, max_per_call: 10n * U };
const win: Win = { start: 1000, spent: 0n };
const ok = (over: Partial<{ cfg: Cfg; win: Win; allowed: boolean; bal: bigint; amt: bigint; to: number; now: number }> = {}) =>
  checkPay(over.cfg ?? cfg, over.win ?? win, over.allowed ?? true, over.bal ?? 100n * U, over.amt ?? U, over.to ?? 600, over.now ?? 1500);

describe("pre-sign policy (mirrors LeashVault.pay)", () => {
  it("allows a payment inside every limit", () => expect(ok()).toBeNull());
  it("refuses a revoked agent", () => expect(ok({ cfg: { ...cfg, revoked: true } })?.code).toBe(2));
  it("refuses an expired agent, allows at valid_until", () => {
    expect(ok({ now: 2001 })?.code).toBe(2);
    expect(ok({ now: 2000 })).toBeNull();
  });
  it("refuses a seller off the allow-list", () => expect(ok({ allowed: false })?.code).toBe(3));
  it("refuses zero amount", () => expect(ok({ amt: 0n })?.code).toBe(4));
  it("refuses over max_per_call", () => {
    expect(ok({ amt: 10n * U + 1n })?.code).toBe(5);
    expect(ok({ amt: 10n * U })).toBeNull();
  });
  it("refuses over the window limit, resets at period end", () => {
    const full = { start: 1000, spent: 20n * U };
    expect(ok({ win: full, amt: 5n * U + 1n })?.code).toBe(6);
    expect(ok({ win: full, amt: 5n * U })).toBeNull();
    const later = { ...cfg, agent_valid_until: 10_000 };
    expect(ok({ cfg: later, win: full, amt: 10n * U, now: 4599 })?.code).toBe(6);
    expect(ok({ cfg: later, win: full, amt: 10n * U, now: 4600 })).toBeNull();
  });
  it("refuses a timeout outside 60..86400", () => {
    expect(ok({ to: 59 })?.code).toBe(7);
    expect(ok({ to: 86_401 })?.code).toBe(7);
    expect(ok({ to: 60 })).toBeNull();
  });
  it("refuses more than the vault balance", () => expect(ok({ bal: U - 1n })?.code).toBe(8));
});
