// npm run log:export — Vault events from testnet RPC getEvents -> deployments/vault-events.jsonl
// npm run log:verify — every pay has exactly one settle or refund; the agent's AgentLog runs verify;
//                      every escrow the agent logged as paid is a real Vault pay event.
// Released and refunded are counted separately: a refund is not a completed sale.
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { rpc, scValToNative, xdr } from "@stellar/stellar-sdk";
import { readDeploy, loadKeys } from "./lib.ts";
import { readLog, verifyRun } from "../src/agentlog.ts";

const EVENTS = "deployments/vault-events.jsonl";
const AGENTLOG = "deployments/agentlog.jsonl";
const d = readDeploy();

const plain = (v: unknown): unknown =>
  typeof v === "bigint" ? v.toString()
  : Buffer.isBuffer(v) ? v.toString("hex")
  : Array.isArray(v) ? v.map(plain)
  : v && typeof v === "object" ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, plain(x)]))
  : v;

async function exportEvents() {
  const server = new rpc.Server("https://soroban-testnet.stellar.org");
  const tx = await server.getTransaction(d.txs.deploy_vault);
  const startLedger = (tx as any).ledger as number;
  const out: string[] = [];
  let cursor: string | undefined;
  for (;;) {
    const r: any = await server.getEvents(
      cursor
        ? { filters: [{ type: "contract", contractIds: [d.vault] }], cursor, limit: 100 }
        : { filters: [{ type: "contract", contractIds: [d.vault] }], startLedger, limit: 100 },
    );
    for (const e of r.events) {
      const topic = e.topic.map((t: xdr.ScVal) => plain(scValToNative(t)));
      out.push(JSON.stringify({ ledger: e.ledger, tx: e.txHash, kind: topic[1], topic, data: plain(scValToNative(e.value)) }));
    }
    if (r.events.length < 100) break;
    cursor = r.cursor;
  }
  writeFileSync(EVENTS, out.join("\n") + (out.length ? "\n" : ""));
  console.log(`${out.length} Vault events -> ${EVENTS}`);
}

function verify() {
  let ok = true;
  const fail = (m: string) => { ok = false; console.log(`FAIL ${m}`); };
  if (!existsSync(EVENTS)) throw new Error("run npm run log:export first");
  const evs = readFileSync(EVENTS, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l));
  const byEscrow = new Map<string, string[]>();
  for (const e of evs.filter((e) => ["pay", "settle", "refund"].includes(e.kind))) {
    const escrow = e.topic[2] as string;
    byEscrow.set(escrow, [...(byEscrow.get(escrow) ?? []), e.kind]);
  }
  let released = 0, refunded = 0;
  for (const [escrow, kinds] of byEscrow) {
    const closes = kinds.filter((k) => k !== "pay");
    if (kinds[0] !== "pay" || kinds.filter((k) => k === "pay").length !== 1) fail(`${escrow}: not exactly one pay first (${kinds})`);
    else if (closes.length !== 1) fail(`${escrow}: ${closes.length} settle/refund events (${kinds})`);
    else if (closes[0] === "settle") released++;
    else refunded++;
  }
  const agent = loadKeys().agent.public;
  const log = readLog(AGENTLOG);
  const runs = [...new Set(log.map((e) => e.run_id))];
  for (const run of runs) {
    const br = verifyRun(log.filter((e) => e.run_id === run), agent);
    if (br) fail(`agentlog run ${run}: step ${br.step} ${br.kind}`);
  }
  for (const e of log.filter((e) => e.action === "pay")) {
    const escrow = e.note.replace("escrow ", "");
    if (byEscrow.get(escrow)?.[0] !== "pay") fail(`agentlog pay ${escrow} has no Vault pay event`);
  }
  console.log(`payments ${byEscrow.size}: released ${released}, refunded ${refunded}; agentlog ${runs.length} runs, ${log.length} entries`);
  console.log(ok ? "verify: OK" : "verify: FAILED");
  if (!ok) process.exit(1);
}

if (process.argv[2] === "export") await exportEvents();
else if (process.argv[2] === "verify") verify();
else throw new Error("usage: log.ts <export|verify>");
