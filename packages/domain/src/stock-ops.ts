// Which ledger movements each stock operation produces. The Rust code (crates/db/src/stock.rs) must produce exactly the
// same list; packages/test-vectors/stock-operations.json is checked by both. Buckets and the "never below zero" rule are
// in ledger.ts.
//
// Where in-transit stock lives: when stock is dispatched it leaves the source's on_hand and appears in the DESTINATION's
// in_transit bucket, so the destination can see what is coming. Receiving moves it to the destination's on_hand;
// cancelling sends it back to the source's on_hand. Total stock is never counted twice.
import type { Bucket } from "./ledger.ts";

export type MovementType =
  | "purchase" | "adjustment" | "damage" | "write_off"
  | "transfer_out" | "transfer_in" | "transfer_cancel" | "count_adjustment";

export interface PlannedMovement {
  locationId: string;
  productId: string;
  bucket: Bucket;
  delta: number;
  type: MovementType;
  key: string;
}

function positive(qty: number, what = "quantity") {
  if (!Number.isInteger(qty) || qty <= 0) throw new Error(`${what} must be a positive whole number`);
}

/** Restock: stock arrives at a location. */
export function receiveStock(p: { location: string; product: string; qty: number; key: string }): PlannedMovement[] {
  positive(p.qty);
  return [{ locationId: p.location, productId: p.product, bucket: "on_hand", delta: p.qty, type: "purchase", key: `${p.key}:m1` }];
}

/** Correction: the quantity is signed (+ found, - lost). */
export function correctStock(p: { location: string; product: string; qty: number; key: string }): PlannedMovement[] {
  if (!Number.isInteger(p.qty) || p.qty === 0) throw new Error("correction must be a non-zero whole number");
  return [{ locationId: p.location, productId: p.product, bucket: "on_hand", delta: p.qty, type: "adjustment", key: `${p.key}:m1` }];
}

/** Damage: good stock becomes damaged stock at the same location. */
export function damageStock(p: { location: string; product: string; qty: number; key: string }): PlannedMovement[] {
  positive(p.qty);
  return [
    { locationId: p.location, productId: p.product, bucket: "on_hand", delta: -p.qty, type: "damage", key: `${p.key}:m1` },
    { locationId: p.location, productId: p.product, bucket: "damaged", delta: p.qty, type: "damage", key: `${p.key}:m2` },
  ];
}

/** Write-off: damaged stock is thrown away. */
export function writeOffStock(p: { location: string; product: string; qty: number; key: string }): PlannedMovement[] {
  positive(p.qty);
  return [{ locationId: p.location, productId: p.product, bucket: "damaged", delta: -p.qty, type: "write_off", key: `${p.key}:m1` }];
}

interface TransferLine { product: string; qty: number }

export function dispatchTransfer(p: { from: string; to: string; lines: TransferLine[]; key: string }): PlannedMovement[] {
  if (p.from === p.to) throw new Error("transfer source and destination must differ");
  return p.lines.flatMap((l, i) => {
    positive(l.qty);
    return [
      { locationId: p.from, productId: l.product, bucket: "on_hand" as const, delta: -l.qty, type: "transfer_out" as const, key: `${p.key}:${i}:out` },
      { locationId: p.to, productId: l.product, bucket: "in_transit" as const, delta: l.qty, type: "transfer_out" as const, key: `${p.key}:${i}:transit` },
    ];
  });
}

export function receiveTransfer(p: { to: string; lines: TransferLine[]; key: string }): PlannedMovement[] {
  return p.lines.flatMap((l, i) => {
    positive(l.qty);
    return [
      { locationId: p.to, productId: l.product, bucket: "in_transit" as const, delta: -l.qty, type: "transfer_in" as const, key: `${p.key}:recv:${i}:transit` },
      { locationId: p.to, productId: l.product, bucket: "on_hand" as const, delta: l.qty, type: "transfer_in" as const, key: `${p.key}:recv:${i}:in` },
    ];
  });
}

export function cancelTransfer(p: { from: string; to: string; lines: TransferLine[]; key: string }): PlannedMovement[] {
  return p.lines.flatMap((l, i) => {
    positive(l.qty);
    return [
      { locationId: p.to, productId: l.product, bucket: "in_transit" as const, delta: -l.qty, type: "transfer_cancel" as const, key: `${p.key}:cancel:${i}:transit` },
      { locationId: p.from, productId: l.product, bucket: "on_hand" as const, delta: l.qty, type: "transfer_cancel" as const, key: `${p.key}:cancel:${i}:back` },
    ];
  });
}

/** A stock count: each line compares what was counted with what the system had; a difference is one movement. */
export function countAdjustments(p: { location: string; lines: { product: string; expected: number; counted: number }[]; key: string }): PlannedMovement[] {
  const out: PlannedMovement[] = [];
  p.lines.forEach((l, i) => {
    if (!Number.isInteger(l.counted) || l.counted < 0) throw new Error("counted quantity must be zero or more");
    const delta = l.counted - l.expected;
    if (delta !== 0) out.push({ locationId: p.location, productId: l.product, bucket: "on_hand", delta, type: "count_adjustment", key: `${p.key}:${i}:count` });
  });
  return out;
}
