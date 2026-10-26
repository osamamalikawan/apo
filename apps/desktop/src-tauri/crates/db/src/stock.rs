//! The core of stock handling: which ledger movements each operation produces, and posting them.
//!
//! The planners are pure functions that mirror `packages/domain/src/stock-ops.ts`; both are tested against
//! `packages/test-vectors/stock-operations.json`, so the two languages cannot drift apart.
//!
//! Where in-transit stock lives: dispatched stock leaves the source's `on_hand` and shows up in the DESTINATION's
//! `in_transit`; receiving moves it to the destination's `on_hand`; cancelling returns it to the source's `on_hand`.
//!
//! `post_all` writes the movements inside the caller's transaction: it refuses to take any bucket below zero (with a
//! message that says how much there is), appends to the append-only ledger (the database trigger keeps the balances in
//! step) and queues each movement for sync. Quantities are whole numbers.

use crate::master::enqueue;
use crate::users::{Principal, Scope, UserError};
use rusqlite::{params, Connection, OptionalExtension};

pub const ON_HAND: &str = "on_hand";
pub const DAMAGED: &str = "damaged";
pub const IN_TRANSIT: &str = "in_transit";

/// Biggest quantity accepted in one line: stops a typo (an extra zero) from becoming a million parts.
pub const MAX_QTY: i64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub location_id: String,
    pub product_id: String,
    pub bucket: &'static str,
    pub delta: i64,
    pub movement_type: &'static str,
    pub key: String,
}

fn mv(location: &str, product: &str, bucket: &'static str, delta: i64, movement_type: &'static str, key: String) -> Planned {
    Planned { location_id: location.into(), product_id: product.into(), bucket, delta, movement_type, key }
}

fn positive(qty: i64) -> Result<(), UserError> {
    if qty <= 0 || qty > MAX_QTY {
        return Err(UserError::BadInput("quantity must be a whole number from 1 to 1,000,000"));
    }
    Ok(())
}

pub fn plan_receive(location: &str, product: &str, qty: i64, key: &str) -> Result<Vec<Planned>, UserError> {
    positive(qty)?;
    Ok(vec![mv(location, product, ON_HAND, qty, "purchase", format!("{key}:m1"))])
}

/// `qty` is signed: positive for stock found, negative for stock lost.
pub fn plan_correct(location: &str, product: &str, qty: i64, key: &str) -> Result<Vec<Planned>, UserError> {
    if qty == 0 || qty.abs() > MAX_QTY {
        return Err(UserError::BadInput("correction must be a whole number other than 0, up to 1,000,000"));
    }
    Ok(vec![mv(location, product, ON_HAND, qty, "adjustment", format!("{key}:m1"))])
}

pub fn plan_damage(location: &str, product: &str, qty: i64, key: &str) -> Result<Vec<Planned>, UserError> {
    positive(qty)?;
    Ok(vec![
        mv(location, product, ON_HAND, -qty, "damage", format!("{key}:m1")),
        mv(location, product, DAMAGED, qty, "damage", format!("{key}:m2")),
    ])
}

pub fn plan_write_off(location: &str, product: &str, qty: i64, key: &str) -> Result<Vec<Planned>, UserError> {
    positive(qty)?;
    Ok(vec![mv(location, product, DAMAGED, -qty, "write_off", format!("{key}:m1"))])
}

/// (product id, quantity)
pub type Lines = [(String, i64)];

pub fn plan_dispatch(from: &str, to: &str, lines: &Lines, key: &str) -> Result<Vec<Planned>, UserError> {
    if from == to {
        return Err(UserError::BadInput("a transfer needs two different locations"));
    }
    let mut out = Vec::new();
    for (i, (p, q)) in lines.iter().enumerate() {
        positive(*q)?;
        out.push(mv(from, p, ON_HAND, -q, "transfer_out", format!("{key}:{i}:out")));
        out.push(mv(to, p, IN_TRANSIT, *q, "transfer_out", format!("{key}:{i}:transit")));
    }
    Ok(out)
}

pub fn plan_receive_transfer(to: &str, lines: &Lines, key: &str) -> Result<Vec<Planned>, UserError> {
    let mut out = Vec::new();
    for (i, (p, q)) in lines.iter().enumerate() {
        positive(*q)?;
        out.push(mv(to, p, IN_TRANSIT, -q, "transfer_in", format!("{key}:recv:{i}:transit")));
        out.push(mv(to, p, ON_HAND, *q, "transfer_in", format!("{key}:recv:{i}:in")));
    }
    Ok(out)
}

pub fn plan_cancel_transfer(from: &str, to: &str, lines: &Lines, key: &str) -> Result<Vec<Planned>, UserError> {
    let mut out = Vec::new();
    for (i, (p, q)) in lines.iter().enumerate() {
        positive(*q)?;
        out.push(mv(to, p, IN_TRANSIT, -q, "transfer_cancel", format!("{key}:cancel:{i}:transit")));
        out.push(mv(from, p, ON_HAND, *q, "transfer_cancel", format!("{key}:cancel:{i}:back")));
    }
    Ok(out)
}

/// (product id, expected, counted). Only differences become movements.
pub type CountLines = [(String, i64, i64)];

pub fn plan_count(location: &str, lines: &CountLines, key: &str) -> Result<Vec<Planned>, UserError> {
    let mut out = Vec::new();
    for (i, (p, expected, counted)) in lines.iter().enumerate() {
        if *counted < 0 || *counted > MAX_QTY {
            return Err(UserError::BadInput("counted quantity must be a whole number from 0 to 1,000,000"));
        }
        let delta = counted - expected;
        if delta != 0 {
            out.push(mv(location, p, ON_HAND, delta, "count_adjustment", format!("{key}:{i}:count")));
        }
    }
    Ok(out)
}

// ---------- checks the operations share ----------

/// An idempotency key made by the client for one attempt: it lets a retry be recognised, never repeated.
pub fn clean_key(raw: &str) -> Result<String, UserError> {
    let k = raw.trim();
    if !(8..=80).contains(&k.len()) || !k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')) {
        return Err(UserError::BadInput("idempotency_key must be 8 to 80 letters, digits or - _ : ."));
    }
    Ok(k.to_string())
}

/// The caller may use this location (defence in depth: the API checks it first).
pub fn guard_location(actor: &Principal, location: &str) -> Result<(), UserError> {
    actor
        .check_scope(Scope { location_id: Some(location), route_id: None })
        .map_err(|_| UserError::Forbidden("location_denied"))
}

pub fn require_location(tx: &Connection, tenant: &str, location: &str) -> Result<(), UserError> {
    let active: Option<i64> = tx
        .query_row("SELECT is_active FROM stock_locations WHERE tenant_id=?1 AND id=?2", params![tenant, location], |r| r.get(0))
        .optional()?;
    match active {
        None => Err(UserError::BadInput("unknown location")),
        Some(0) => Err(UserError::Conflict("that location is inactive")),
        Some(_) => Ok(()),
    }
}

/// `must_be_active`: restocking and moving need an active product; adjusting and counting do not.
pub fn require_product(tx: &Connection, tenant: &str, product: &str, must_be_active: bool) -> Result<(), UserError> {
    let active: Option<i64> = tx
        .query_row("SELECT is_active FROM products WHERE tenant_id=?1 AND id=?2", params![tenant, product], |r| r.get(0))
        .optional()?;
    match active {
        None => Err(UserError::BadInput("unknown product")),
        Some(0) if must_be_active => Err(UserError::Conflict("that product is inactive")),
        Some(_) => Ok(()),
    }
}

pub fn bucket_balance(conn: &Connection, tenant: &str, location: &str, product: &str, bucket: &str) -> Result<i64, rusqlite::Error> {
    let col = match bucket {
        ON_HAND => "on_hand",
        DAMAGED => "damaged",
        _ => "in_transit",
    };
    Ok(conn
        .query_row(
            &format!("SELECT {col} FROM inventory_balances WHERE tenant_id=?1 AND location_id=?2 AND product_id=?3"),
            params![tenant, location, product],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

/// Next number such as `TR-000001`, unique per shop. Runs inside the writer's transaction, so it cannot race.
pub fn next_number(tx: &Connection, tenant: &str, table: &str, prefix: &str) -> Result<String, rusqlite::Error> {
    debug_assert!(matches!(table, "stock_transfers" | "stock_counts"));
    let mut n: i64 = tx.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE tenant_id=?1"), [tenant], |r| r.get(0))?;
    loop {
        n += 1;
        let candidate = format!("{prefix}-{n:06}");
        let taken: bool = tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE tenant_id=?1 AND number=?2)"), params![tenant, candidate], |r| r.get(0))?;
        if !taken {
            return Ok(candidate);
        }
    }
}

/// Append the movements to the ledger. All or nothing: the caller's transaction is dropped on error.
pub fn post_all(
    tx: &Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    source_type: &str,
    source_id: &str,
    planned: &[Planned],
    now: &str,
) -> Result<(), UserError> {
    for m in planned {
        let have = bucket_balance(tx, tenant, &m.location_id, &m.product_id, m.bucket)?;
        if have + m.delta < 0 {
            return Err(UserError::Short { product_id: m.product_id.clone(), location_id: m.location_id.clone(), have, need: -m.delta });
        }
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO stock_movements(id,tenant_id,location_id,product_id,bucket,qty_delta,movement_type,source_type,source_id,
                                         idempotency_key,origin_type,device_id,actor_id,occurred_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'server',?11,?12,?13)",
            params![id, tenant, m.location_id, m.product_id, m.bucket, m.delta, m.movement_type, source_type, source_id, m.key, device, actor.user_id, now],
        )?;
        let snapshot = serde_json::json!({
            "id": id, "location_id": m.location_id, "product_id": m.product_id, "bucket": m.bucket, "qty_delta": m.delta,
            "movement_type": m.movement_type, "source_type": source_type, "source_id": source_id, "idempotency_key": m.key, "occurred_at": now,
        });
        enqueue(tx, tenant, "stock_movement", &id, "post", &snapshot, None, 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run one vector case through the matching planner.
    fn run(op: &str, i: &serde_json::Value) -> Result<Vec<Planned>, UserError> {
        let st = |k: &str| i[k].as_str().unwrap_or("").to_string();
        let qty = || {
            // a fractional quantity in a vector must be refused, so it cannot be silently truncated
            let q = &i["qty"];
            if let Some(n) = q.as_i64() {
                Ok(n)
            } else {
                Err(UserError::BadInput("quantity must be a whole number"))
            }
        };
        let lines = || -> Vec<(String, i64)> { i["lines"].as_array().unwrap().iter().map(|l| (l["product"].as_str().unwrap().to_string(), l["qty"].as_i64().unwrap())).collect() };
        match op {
            "receive" => plan_receive(&st("location"), &st("product"), qty()?, &st("key")),
            "correct" => plan_correct(&st("location"), &st("product"), qty()?, &st("key")),
            "damage" => plan_damage(&st("location"), &st("product"), qty()?, &st("key")),
            "write_off" => plan_write_off(&st("location"), &st("product"), qty()?, &st("key")),
            "dispatch" => plan_dispatch(&st("from"), &st("to"), &lines(), &st("key")),
            "receive_transfer" => plan_receive_transfer(&st("to"), &lines(), &st("key")),
            "cancel_transfer" => plan_cancel_transfer(&st("from"), &st("to"), &lines(), &st("key")),
            "count" => {
                let cl: Vec<(String, i64, i64)> = i["lines"].as_array().unwrap().iter().map(|l| (l["product"].as_str().unwrap().to_string(), l["expected"].as_i64().unwrap(), l["counted"].as_i64().unwrap())).collect();
                plan_count(&st("location"), &cl, &st("key"))
            }
            other => panic!("unknown op {other}"),
        }
    }

    fn vectors() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../../../../packages/test-vectors/stock-operations.json")).unwrap()
    }

    fn wire(p: &Planned) -> serde_json::Value {
        serde_json::json!({ "location": p.location_id, "product": p.product_id, "bucket": p.bucket, "delta": p.delta, "type": p.movement_type, "key": p.key })
    }

    #[test]
    fn the_planners_produce_exactly_the_movements_in_the_shared_vectors() {
        let v = vectors();
        let cases = v["movements"].as_array().unwrap();
        assert!(cases.len() >= 9);
        for c in cases {
            let got: Vec<serde_json::Value> = run(c["op"].as_str().unwrap(), &c["input"]).unwrap().iter().map(wire).collect();
            assert_eq!(serde_json::Value::Array(got), c["expect"], "{}", c["name"]);
        }
    }

    #[test]
    fn the_planners_refuse_what_the_shared_vectors_refuse() {
        let v = vectors();
        let cases = v["errors"].as_array().unwrap();
        assert!(cases.len() >= 8);
        for c in cases {
            assert!(run(c["op"].as_str().unwrap(), &c["input"]).is_err(), "{}", c["name"]);
        }
    }

    #[test]
    fn the_scenarios_leave_the_balances_the_vectors_expect_on_a_real_ledger() {
        use crate::master::testutil::*;
        let v = vectors();
        for sc in v["scenarios"].as_array().unwrap() {
            let mut c = setup();
            c.execute_batch(
                "INSERT INTO stock_locations(id,tenant_id,name,code) VALUES ('main','t1','Main','MAIN'),('g1','t1','Godown','G1');
                 INSERT INTO products(id,tenant_id,sku,name) VALUES ('p1','t1','P1','Part 1'),('p2','t1','P2','Part 2'),('p3','t1','P3','Part 3');",
            )
            .unwrap();
            let me = actor(&["*"], &[]);
            for (n, step) in sc["steps"].as_array().unwrap().iter().enumerate() {
                let planned = run(step["op"].as_str().unwrap(), &step["input"]).unwrap();
                let tx = c.transaction().unwrap();
                let res = post_all(&tx, "t1", &me, None, "test", &format!("s{n}"), &planned, T0);
                let expect_rejected = step["expectRejected"].as_bool().unwrap_or(false);
                assert_eq!(res.is_err(), expect_rejected, "{} step {n}", sc["name"]);
                if res.is_ok() {
                    tx.commit().unwrap();
                } // otherwise the transaction is dropped: nothing from this step remains
            }
            for (k, want) in sc["balances"].as_object().unwrap() {
                let (loc, prod) = k.split_once('|').unwrap();
                for b in [ON_HAND, DAMAGED, IN_TRANSIT] {
                    assert_eq!(bucket_balance(&c, "t1", loc, prod, b).unwrap(), want[b].as_i64().unwrap(), "{} {k} {b}", sc["name"]);
                }
            }
            // the balance table always equals the sum of the ledger
            let drift: i64 = c.query_row(
                "SELECT COUNT(*) FROM inventory_balances ib WHERE ib.on_hand <> COALESCE((SELECT SUM(qty_delta) FROM stock_movements m WHERE m.location_id=ib.location_id AND m.product_id=ib.product_id AND m.bucket='on_hand'),0)",
                [],
                |r| r.get(0),
            )
            .unwrap();
            assert_eq!(drift, 0, "{}", sc["name"]);
        }
    }

    #[test]
    fn a_short_posting_names_what_is_available_and_needed() {
        use crate::master::testutil::*;
        let mut c = setup();
        c.execute_batch(
            "INSERT INTO stock_locations(id,tenant_id,name,code) VALUES ('main','t1','Main','MAIN');
             INSERT INTO products(id,tenant_id,sku,name) VALUES ('p1','t1','P1','Part 1');",
        )
        .unwrap();
        let me = actor(&["*"], &[]);
        let tx = c.transaction().unwrap();
        post_all(&tx, "t1", &me, None, "t", "s", &plan_receive("main", "p1", 3, "k-aaaa1111").unwrap(), T0).unwrap();
        let err = post_all(&tx, "t1", &me, None, "t", "s", &plan_damage("main", "p1", 5, "k-bbbb2222").unwrap(), T0).unwrap_err();
        assert!(matches!(err, UserError::Short { have: 3, need: 5, .. }), "{err:?}");
    }

    #[test]
    fn keys_are_checked() {
        assert_eq!(clean_key("  abc-12345  ").unwrap(), "abc-12345");
        for bad in ["short", "has space in it", "ünïcode-key-1", &"x".repeat(81), ""] {
            assert!(clean_key(bad).is_err(), "{bad:?}");
        }
    }
}
