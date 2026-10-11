// Money is integer minor units (bigint). Floating point is never used for amounts.
export interface Money { readonly minor: bigint; readonly currency: string }

export function money(minor: bigint | number, currency = "PKR"): Money {
  if (typeof minor === "number") {
    if (!Number.isSafeInteger(minor)) throw new Error("money: number must be a safe integer of minor units");
    minor = BigInt(minor);
  }
  return { minor, currency };
}

function same(a: Money, b: Money): void {
  if (a.currency !== b.currency) throw new Error(`money: currency mismatch ${a.currency}/${b.currency}`);
}
export const add = (a: Money, b: Money): Money => (same(a, b), money(a.minor + b.minor, a.currency));
export const sub = (a: Money, b: Money): Money => (same(a, b), money(a.minor - b.minor, a.currency));
export const neg = (a: Money): Money => money(-a.minor, a.currency);
export const isZero = (a: Money): boolean => a.minor === 0n;

/** Parse "1234.50" (up to `exp` decimals) into minor units. */
export function fromDecimal(s: string, currency = "PKR", exp = 2): Money {
  const m = /^(-?)(\d+)(?:\.(\d+))?$/.exec(s.trim());
  if (!m) throw new Error(`money: invalid decimal "${s}"`);
  const [, sign, whole, frac = ""] = m;
  if (frac.length > exp) throw new Error(`money: too many decimals in "${s}"`);
  const v = BigInt(whole! + frac.padEnd(exp, "0"));
  return money(sign === "-" ? -v : v, currency);
}

export function toDecimal(a: Money, exp = 2): string {
  const neg = a.minor < 0n;
  const abs = (neg ? -a.minor : a.minor).toString().padStart(exp + 1, "0");
  const out = `${abs.slice(0, abs.length - exp)}.${abs.slice(abs.length - exp)}`;
  return neg ? `-${out}` : out;
}

/** Apply a rate in basis points (e.g. 1800 = 18%), rounding half away from zero. */
export function applyBps(a: Money, bps: number): Money {
  if (!Number.isInteger(bps)) throw new Error("money: bps must be an integer");
  const num = a.minor * BigInt(bps);
  const den = 10000n;
  const abs = num < 0n ? -num : num;
  let q = abs / den;
  if ((abs % den) * 2n >= den) q += 1n;
  return money(num < 0n ? -q : q, a.currency);
}

/** Split an amount by integer ratios using largest remainder; the parts always sum to the total. */
export function allocate(a: Money, ratios: number[]): Money[] {
  const total = ratios.reduce((s, r) => s + r, 0);
  if (ratios.some((r) => !Number.isInteger(r) || r < 0) || total <= 0) throw new Error("money: bad ratios");
  const T = BigInt(total);
  const sign = a.minor < 0n ? -1n : 1n;
  const abs = a.minor * sign;
  const shares = ratios.map((r) => (abs * BigInt(r)) / T);
  let left = abs - shares.reduce((s, x) => s + x, 0n);
  const order = ratios
    .map((r, i) => ({ i, rem: (abs * BigInt(r)) % T }))
    .sort((x, y) => (x.rem === y.rem ? x.i - y.i : x.rem > y.rem ? -1 : 1));
  for (const { i } of order) { if (left <= 0n) break; shares[i]! += 1n; left -= 1n; }
  return shares.map((s) => money(s * sign, a.currency));
}
