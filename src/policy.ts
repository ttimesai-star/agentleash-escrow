// Pre-sign policy check: the agent refuses a payment locally, with a reason, before it signs anything.
// Mirrors LeashVault.pay checks in the same order; codes are the contract's #[contracterror] numbers.
export type Cfg = { revoked: boolean; agent_valid_until: number; limit: bigint; period_secs: number; max_per_call: bigint };
export type Win = { start: number; spent: bigint };
export type Refusal = { code: number; name: string; reason: string };

export const MIN_TIMEOUT = 60;
export const MAX_TIMEOUT = 86_400;

export function checkPay(
  cfg: Cfg, win: Win, payeeAllowed: boolean, balance: bigint, amount: bigint, timeout: number, now: number,
): Refusal | null {
  if (cfg.revoked || now > cfg.agent_valid_until) return { code: 2, name: "AgentInactive", reason: "agent key is revoked or expired" };
  if (!payeeAllowed) return { code: 3, name: "PayeeNotAllowed", reason: "seller is not on the owner's allow-list" };
  if (amount <= 0n) return { code: 4, name: "BadAmount", reason: "amount must be positive" };
  if (amount > cfg.max_per_call) return { code: 5, name: "OverMaxPerCall", reason: `amount ${amount} > max_per_call ${cfg.max_per_call}` };
  const spent = now >= win.start + cfg.period_secs ? 0n : win.spent;
  if (spent + amount > cfg.limit) return { code: 6, name: "OverWindowLimit", reason: `spent ${spent} + ${amount} > window limit ${cfg.limit}` };
  if (timeout < MIN_TIMEOUT || timeout > MAX_TIMEOUT) return { code: 7, name: "BadTimeout", reason: `timeout must be ${MIN_TIMEOUT}..${MAX_TIMEOUT} s` };
  if (balance < amount) return { code: 8, name: "InsufficientBalance", reason: `vault balance ${balance} < ${amount}` };
  return null;
}
