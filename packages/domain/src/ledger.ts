// Reference in-memory stock ledger. It defines the posting rules that the Rust ledger,
// the SQLite triggers and the Postgres RPC must all follow; shared vectors check they agree.
export type Bucket = "on_hand" | "damaged" | "in_transit";
export interface Movement {
  locationId: string; productId: string; bucket: Bucket;
  delta: number; key: string; allowNegative?: boolean;
}
export type PostResult = "posted" | "duplicate" | "rejected";
type Bal = Record<Bucket, number>;

export class InMemoryLedger {
  readonly movements: Movement[] = [];
  private keys = new Set<string>();
  private bal = new Map<string, Bal>();

  private slot(l: string, p: string): Bal {
    const k = `${l}|${p}`;
    let b = this.bal.get(k);
    if (!b) { b = { on_hand: 0, damaged: 0, in_transit: 0 }; this.bal.set(k, b); }
    return b;
  }
  balance(l: string, p: string): Bal { return { ...this.slot(l, p) }; }

  post(m: Movement): PostResult {
    if (!Number.isInteger(m.delta) || m.delta === 0) return "rejected";
    if (this.keys.has(m.key)) return "duplicate";
    const b = this.slot(m.locationId, m.productId);
    if (!m.allowNegative && b[m.bucket] + m.delta < 0) return "rejected";
    b[m.bucket] += m.delta;
    this.keys.add(m.key);
    this.movements.push(m);
    return "posted";
  }

  /** Rebuild balances purely from the movement list; used to prove balances reconcile. */
  rebuild(): Map<string, Bal> {
    const out = new Map<string, Bal>();
    for (const m of this.movements) {
      const k = `${m.locationId}|${m.productId}`;
      const b = out.get(k) ?? { on_hand: 0, damaged: 0, in_transit: 0 };
      b[m.bucket] += m.delta;
      out.set(k, b);
    }
    return out;
  }
}

/** A direct transfer = two movements with derived keys so a retry can never post half. */
export function transferMovements(p: {
  productId: string; from: string; to: string; qty: number; key: string;
}): [Movement, Movement] {
  if (!Number.isInteger(p.qty) || p.qty <= 0) throw new Error("transfer qty must be a positive integer");
  if (p.from === p.to) throw new Error("transfer source and destination must differ");
  return [
    { locationId: p.from, productId: p.productId, bucket: "on_hand", delta: -p.qty, key: `${p.key}:out` },
    { locationId: p.to, productId: p.productId, bucket: "on_hand", delta: p.qty, key: `${p.key}:in` },
  ];
}
