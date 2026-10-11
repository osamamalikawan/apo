import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { verify } from "../../../tools/dev-license-key.mjs";
import { InMemoryLedger, transferMovements, authorize, hasPermission,
  evaluate, canWrite, isIsoSeconds,
  money, add, sub, fromDecimal, toDecimal, applyBps, allocate } from "../src/index.ts";

const vec = (f: string) => JSON.parse(readFileSync(new URL(`../../test-vectors/${f}`, import.meta.url), "utf8"));

// ---- shared ledger vectors (the SQLite test runs the same file) ----
for (const seq of vec("ledger-sequences.json")) {
  test(`ledger vector: ${seq.name}`, () => {
    const l = new InMemoryLedger();
    for (const s of seq.steps) {
      const r = l.post({ locationId: s.loc, productId: "p1", bucket: s.bucket, delta: s.delta, key: s.key, allowNegative: !!s.allow_negative });
      assert.equal(r, s.expect, `step ${s.key}`);
    }
    for (const [loc, want] of Object.entries(seq.final)) assert.deepEqual(l.balance(loc, "p1"), want);
    // balances must reconcile with the ledger
    for (const [k, b] of l.rebuild()) assert.deepEqual(l.balance(...(k.split("|") as [string, string])), b);
  });
}

test("transferMovements derives stable keys and validates input", () => {
  const [o, i] = transferMovements({ productId: "p", from: "a", to: "b", qty: 5, key: "t9" });
  assert.deepEqual([o.key, i.key, o.delta, i.delta], ["t9:out", "t9:in", -5, 5]);
  assert.throws(() => transferMovements({ productId: "p", from: "a", to: "a", qty: 1, key: "k" }));
  assert.throws(() => transferMovements({ productId: "p", from: "a", to: "b", qty: 0, key: "k" }));
});

// ---- permission vectors ----
const pv = vec("permissions.json");
for (const c of pv.cases) {
  test(`permission: ${c.ctx} ${c.perm} ${JSON.stringify(c.scope)}`, () => {
    const d = authorize(pv.contexts[c.ctx], c.perm, c.scope);
    assert.equal(d.allowed, c.allowed);
    if (!d.allowed) assert.equal(d.reason, c.reason);
  });
}
test("wildcards do not leak across modules", () => {
  assert.equal(hasPermission(["sales.*"], "sales.create"), true);
  assert.equal(hasPermission(["sales.*"], "salesx.create"), false);
  assert.equal(hasPermission([], "sales.view"), false);
});

// ---- money ----
test("money arithmetic is exact (0.1 + 0.2)", () => {
  const s = add(fromDecimal("0.10"), fromDecimal("0.20"));
  assert.equal(toDecimal(s), "0.30");
  assert.equal(toDecimal(sub(money(5), money(105))), "-1.00");
});
test("money rejects mixed currencies and bad input", () => {
  assert.throws(() => add(money(1, "PKR"), money(1, "USD")));
  assert.throws(() => fromDecimal("1.234"));
  assert.throws(() => money(1.5));
});
test("applyBps rounds half away from zero", () => {
  assert.equal(applyBps(money(1050), 1800).minor, 189n);   // 189.0
  assert.equal(applyBps(money(5), 1000).minor, 1n);        // 0.5 -> 1
  assert.equal(applyBps(money(-5), 1000).minor, -1n);
});
test("allocate preserves the total", () => {
  const parts = allocate(money(100), [1, 1, 1]);
  assert.deepEqual(parts.map((p) => p.minor), [34n, 33n, 33n]);
  const neg = allocate(money(-100), [1, 1, 1]);
  assert.equal(neg.reduce((s, p) => s + p.minor, 0n), -100n);
});

// ---- entitlements (shared vectors; the Rust server runs the same file) ----
const ev = vec("entitlements.json");
for (const c of ev.evaluation_cases) {
  test(`entitlement: ${c.name}`, () => {
    const d = evaluate(c.ent, "t1", c.now, c.high_water);
    assert.deepEqual([d.mode, d.reason], [c.mode, c.reason]);
  });
}
test("only full and grace modes may write", () => {
  assert.deepEqual(["full", "grace", "read_only", "unlicensed"].map((m) => canWrite(m as never)), [true, true, false, false]);
});
test("entitlement dates use the fixed UTC seconds format", () => {
  assert.equal(isIsoSeconds("2026-10-10T10:00:00Z"), true);
  assert.equal(isIsoSeconds("2026-10-10T10:00:00.000Z"), false);
  assert.equal(isIsoSeconds("2026-10-10 10:00:00"), false);
});
test("vector signatures verify with the right key and fail otherwise", () => {
  for (const [name, e] of Object.entries<any>(ev.envelopes)) {
    assert.equal(verify(ev.public_key_b64, e.payload_b64, e.signature_b64), true, name);
    assert.equal(verify(ev.other_public_key_b64, e.payload_b64, e.signature_b64), false, `${name} with wrong key`);
  }
  assert.equal(verify(ev.public_key_b64, ev.tampered.payload_b64, ev.tampered.signature_b64), false, "tampered payload");
});
test("vector payloads carry the fields the server relies on", () => {
  const p = ev.envelopes.active_rev1.payload;
  for (const k of ["tenant_id", "revision", "status", "issued_at", "full_until", "grace_until", "limits"]) assert.ok(k in p, k);
  assert.ok(isIsoSeconds(p.issued_at) && isIsoSeconds(p.full_until) && isIsoSeconds(p.grace_until));
});
