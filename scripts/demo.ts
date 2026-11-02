// npm run demo:<release|refund|refused|forged> — the three flows (+ forged receipts) against the testnet deployment.
// The seller is simulated in-process (it answers the request and signs a receipt with its Stellar key).
import { loadKeys, stellar, readDeploy, writeDeploy, logRun, expert, CliError, USDC_SAC } from "./lib.ts";
import { checkPay, type Cfg, type Win } from "../src/policy.ts";
import { hashJson, signReceipt } from "../src/receipt.ts";
import { append } from "../src/agentlog.ts";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";

const UNIT = 10_000_000n;
const mode = process.argv[2];
const force = process.argv.includes("--force-chain");
const k = loadKeys();
const d = readDeploy();
const V = d.vault;
const RUN = `${mode}-${Date.now()}`;
const alog = (action: string, note: string, input: unknown, output: unknown) =>
  append("deployments/agentlog.jsonl", k.agent.secret, { agent_id: "agentleash-demo", run_id: RUN, action, tool: "leash-vault", note, input, output });

const view = (fn: string, ...args: string[]) => stellar(["contract", "invoke", "--id", V, "--send", "no", "--", fn, ...args], k.agent.secret).out;
const balance = (who: string) => BigInt(JSON.parse(stellar(["contract", "invoke", "--id", USDC_SAC, "--send", "no", "--", "balance", "--id", who], k.agent.secret).out));
const now = () => Math.floor(Date.now() / 1000);
const sleep = (s: number) => new Promise((r) => setTimeout(r, s * 1000));
const fmt = (x: bigint) => `${x / UNIT}.${(x % UNIT).toString().padStart(7, "0")}`;
const contractErr = (e: unknown) => {
  if (!(e instanceof CliError)) return String(e);
  const c = e.stderr.match(/Error\(Contract, #\d+\)/)?.[0];
  if (c) return c;
  const host = e.stderr.match(/Error\([A-Za-z]+, [A-Za-z]+\)/)?.[0];
  const why = e.stderr.match(/data:"([^"]+)"/g)?.at(-1)?.slice(6, -1);
  return host ? `${host}${why ? ` (${why})` : ""}` : e.stderr.split("\n").filter(Boolean).slice(-2).join(" | ");
};

function readPolicy() {
  const c = JSON.parse(view("config"));
  const w = JSON.parse(view("window"));
  const cfg: Cfg = { revoked: c.revoked, agent_valid_until: Number(c.agent_valid_until), limit: BigInt(c.limit), period_secs: Number(c.period_secs), max_per_call: BigInt(c.max_per_call) };
  const win: Win = { start: Number(w.start), spent: BigInt(w.spent) };
  return { cfg, win };
}

/** The agent: checks policy locally, then signs and sends `pay`. Returns escrow + tx, or the refusal. */
function agentPay(seller: string, amount: bigint, request: unknown, timeout: number) {
  const requestHash = hashJson(request);
  const { cfg, win } = readPolicy();
  const allowed = view("payee", "--seller", seller) !== "null";
  const refusal = checkPay(cfg, win, allowed, balance(V), amount, timeout, now());
  const input = { seller, amount: String(amount), request_hash: requestHash, timeout };
  if (refusal && !force) {
    alog("pay.refused", `local #${refusal.code} ${refusal.name}`, input, refusal);
    return { refused: `SDK refused before signing: #${refusal.code} ${refusal.name} — ${refusal.reason}`, requestHash };
  }
  try {
    const r = stellar(
      ["contract", "invoke", "--id", V, "--", "pay", "--seller", seller, "--amount", String(amount), "--request_hash", requestHash, "--timeout_secs", String(timeout)],
      k.agent.secret,
    );
    const escrow = JSON.parse(r.out) as string;
    alog("pay", `escrow ${escrow}`, input, { escrow, tx: r.tx });
    return { escrow, tx: r.tx, requestHash };
  } catch (e) {
    alog("pay.refused", `chain ${contractErr(e)}`, input, { error: contractErr(e) });
    return { refused: `chain refused: ${contractErr(e)}${refusal ? ` (SDK predicted #${refusal.code} ${refusal.name})` : ""}`, requestHash };
  }
}

/** The seller: serves the request, returns the result and a signed receipt for this escrow. */
function sellerServe(escrow: string, requestHash: string, request: { q: string }) {
  const result = { answer: `price of ${request.q}: 42.00`, served_at: new Date().toISOString() };
  const resultHash = hashJson(result);
  return { result, resultHash, sig: signReceipt(k.seller.secret, V, escrow, requestHash, resultHash) };
}

const settle = (escrow: string, resultHash: string, sig: string) =>
  stellar(["contract", "invoke", "--id", V, "--", "settle", "--escrow", escrow, "--result_hash", resultHash, "--sig", sig], k.seller.secret);

function record(name: string, rec: Record<string, unknown>) {
  d.runs[name] = { at: new Date().toISOString(), ...rec };
  writeDeploy(d);
  logRun({ flow: name, ...rec });
}

async function release() {
  const req = { q: "XLM/USDC", api: "demo-quotes", nonce: now() };
  const sellerBefore = balance(k.seller.public);
  const p = agentPay(k.seller.public, 2n * UNIT, req, 600);
  if (!p.escrow) throw new Error(p.refused);
  console.log(`pay     escrow ${p.escrow}\n        tx ${expert("tx", p.tx!)}`);
  const s = sellerServe(p.escrow, p.requestHash, req);
  const st = settle(p.escrow, s.resultHash, s.sig);
  const got = balance(k.seller.public) - sellerBefore;
  console.log(`settle  tx ${expert("tx", st.tx!)}\nseller +${fmt(got)} USDC (2.0000000 minus Trustless Work 0.3%)`);
  record("release", { escrow: p.escrow, pay_tx: p.tx, settle_tx: st.tx, amount: String(2n * UNIT), seller_received: String(got), request_hash: p.requestHash, result_hash: s.resultHash });
}

/** Same release, over HTTP: the seller answers 402 with its terms, checks the escrow on chain, then serves and settles. */
async function release402() {
  const PRICE = 2n * UNIT;
  const server = createServer((req, res) => {
    const request = { method: req.method, path: req.url };
    const requestHash = hashJson(request);
    const escrow = req.headers["x-leash-escrow"] as string | undefined;
    const reply = (code: number, body: unknown) => { res.writeHead(code, { "content-type": "application/json" }); res.end(JSON.stringify(body)); };
    if (!escrow) return reply(402, { payee: k.seller.public, price: String(PRICE), vault: V, request_hash: requestHash, timeout_secs: 600 });
    const p = JSON.parse(view("payment", "--escrow", escrow));
    if (p.seller !== k.seller.public || BigInt(p.amount) < PRICE || p.status !== "Open" || p.request_hash !== requestHash) {
      return reply(402, { error: "escrow does not pay this request", payment: p });
    }
    const s = sellerServe(escrow, requestHash, { q: String(req.url) });
    reply(200, { result: s.result, result_hash: s.resultHash });
    const st = settle(escrow, s.resultHash, s.sig); // the seller collects with its own receipt
    record("release402", { escrow, settle_tx: st.tx, request_hash: requestHash, result_hash: s.resultHash });
    console.log(`seller settled: tx ${expert("tx", st.tx!)}`);
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/quote/XLM-USDC`;
  const first = await fetch(url);
  const terms = await first.json();
  console.log(`GET ${url} -> ${first.status} ${JSON.stringify(terms)}`);
  if (terms.request_hash !== hashJson({ method: "GET", path: "/quote/XLM-USDC" })) throw new Error("seller quoted another request");
  const p = agentPay(terms.payee, BigInt(terms.price), { method: "GET", path: "/quote/XLM-USDC" }, terms.timeout_secs);
  if (!p.escrow) throw new Error(p.refused);
  console.log(`pay     escrow ${p.escrow}
        tx ${expert("tx", p.tx!)}`);
  const second = await fetch(url, { headers: { "x-leash-escrow": p.escrow } });
  const body = await second.json();
  console.log(`GET with escrow -> ${second.status} ${JSON.stringify(body.result)}`);
  if (hashJson(body.result) !== body.result_hash) throw new Error("result does not match its hash");
  await new Promise((r) => setTimeout(r, 15_000)); // let the seller's settle land
  d.runs.release402 = { ...d.runs.release402, pay_tx: p.tx };
  writeDeploy(d);
  server.close();
}

async function refund() {
  const req = { q: "BTC/USDC", api: "demo-quotes", nonce: now() };
  const p = agentPay(k.seller.public, 1n * UNIT, req, 60);
  if (!p.escrow) throw new Error(p.refused);
  const vaultAfterPay = balance(V);
  console.log(`pay     escrow ${p.escrow} (timeout 60 s)\n        tx ${expert("tx", p.tx!)}`);
  const late = sellerServe(p.escrow, p.requestHash, req); // the seller answers, but too late
  console.log("seller silent; waiting for the deadline...");
  await sleep(75);
  let lateErr = "";
  try { settle(p.escrow, late.resultHash, late.sig); lateErr = "UNEXPECTED: settled after deadline"; } catch (e) { lateErr = contractErr(e); }
  console.log(`settle after deadline, valid receipt -> ${lateErr} (DeadlinePassed = #11)`);
  const r = stellar(["contract", "invoke", "--id", V, "--", "refund", "--escrow", p.escrow], k.agent.secret);
  alog("refund", `escrow ${p.escrow}`, { escrow: p.escrow }, { tx: r.tx });
  const back = balance(V) - vaultAfterPay;
  let afterErr = "";
  try { settle(p.escrow, late.resultHash, late.sig); afterErr = "UNEXPECTED: settled after refund"; } catch (e) { afterErr = contractErr(e); }
  console.log(`refund  tx ${expert("tx", r.tx!)}\nvault +${fmt(back)} USDC back (1.0000000 minus Trustless Work 0.3%)\nsettle after refund -> ${afterErr} (NotOpen = #10)`);
  record("refund", { escrow: p.escrow, pay_tx: p.tx, refund_tx: r.tx, amount: String(UNIT), vault_returned: String(back), settle_after_deadline: lateErr, settle_after_refund: afterErr });
}

async function refused() {
  const stranger = k.owner.public; // not on the allow-list
  const out: Record<string, string> = {};
  const req = { q: "ETH/USDC", api: "demo-quotes", nonce: now() };
  out.unknown_seller = agentPay(stranger, UNIT, req, 600).refused ?? "UNEXPECTED: paid";
  out.over_max_per_call = agentPay(k.seller.public, 11n * UNIT, req, 600).refused ?? "UNEXPECTED: paid";
  // fill the window up to the limit with real payments that will refund, then one more
  const { cfg, win } = readPolicy();
  const spent = now() >= win.start + cfg.period_secs ? 0n : win.spent;
  const fills: string[] = [];
  let left = cfg.limit - spent;
  while (left >= cfg.max_per_call) {
    const p = agentPay(k.seller.public, cfg.max_per_call, { ...req, fill: fills.length }, 60);
    if (!p.escrow) throw new Error(p.refused);
    fills.push(p.escrow);
    left -= cfg.max_per_call;
  }
  out.over_window_limit = agentPay(k.seller.public, left + 1n, req, 600).refused ?? "UNEXPECTED: paid";
  const rv = stellar(["contract", "invoke", "--id", V, "--", "revoke_agent"], k.owner.secret);
  out.revoked_agent = agentPay(k.seller.public, UNIT, req, 600).refused ?? "UNEXPECTED: paid";
  const ra = stellar(["contract", "invoke", "--id", V, "--", "set_agent", "--agent", k.agent.public, "--valid_until", String(cfg.agent_valid_until)], k.owner.secret);
  for (const [k2, v] of Object.entries(out)) console.log(`${k2.padEnd(18)} ${v}`);
  console.log(`(window filled with ${fills.length} escrows of max_per_call; refund them after 60 s with npm run demo:refund-all)`);
  record(force ? "refused_chain" : "refused", { ...out, revoke_tx: rv.tx, restore_agent_tx: ra.tx, fill_escrows: fills });
}

async function forged() {
  const req = { q: "SOL/USDC", api: "demo-quotes", nonce: now() };
  const p = agentPay(k.seller.public, UNIT, req, 300);
  if (!p.escrow) throw new Error(p.refused);
  const good = sellerServe(p.escrow, p.requestHash, req);
  const out: Record<string, string> = {};
  const wrongKey = signReceipt(k.agent.secret, V, p.escrow, p.requestHash, good.resultHash);
  try { settle(p.escrow, good.resultHash, wrongKey); out.wrong_key = "UNEXPECTED: settled"; } catch (e) { out.wrong_key = contractErr(e); }
  const otherResult = hashJson({ answer: "tampered" });
  try { settle(p.escrow, otherResult, good.sig); out.other_result_hash = "UNEXPECTED: settled"; } catch (e) { out.other_result_hash = contractErr(e); }
  const otherEscrow = d.runs?.release?.escrow ?? V;
  const foreign = signReceipt(k.seller.secret, V, otherEscrow, p.requestHash, good.resultHash);
  try { settle(p.escrow, good.resultHash, foreign); out.receipt_for_other_escrow = "UNEXPECTED: settled"; } catch (e) { out.receipt_for_other_escrow = contractErr(e); }
  const st = settle(p.escrow, good.resultHash, good.sig);
  out.genuine_receipt = `settled, tx ${st.tx}`;
  for (const [k2, v] of Object.entries(out)) console.log(`${k2.padEnd(26)} ${v}`);
  record("forged", { escrow: p.escrow, pay_tx: p.tx, settle_tx: st.tx, ...out });
}

async function refundAll() {
  const fills: string[] = d.runs?.refused?.fill_escrows ?? [];
  for (const e of fills) {
    try {
      const r = stellar(["contract", "invoke", "--id", V, "--", "refund", "--escrow", e], k.agent.secret);
      alog("refund", `escrow ${e}`, { escrow: e }, { tx: r.tx });
      console.log(`refund ${e} tx ${r.tx}`);
    } catch (err) { console.log(`refund ${e}: ${contractErr(err)}`); }
  }
}

const flows: Record<string, () => Promise<void>> = { release, release402, refund, refused, forged, "refund-all": refundAll };
if (!flows[mode]) throw new Error(`usage: demo.ts <${Object.keys(flows).join("|")}> [--force-chain]`);
await flows[mode]();
