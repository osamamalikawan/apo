//! Stock levels, restocking, adjustments, reorder levels and movement history.
//!
//! Every change goes through the append-only ledger (`stock::post_all`); the tables here (`stock_operations`) only
//! record WHY stock moved: who, where, how many and the reason. Each operation carries a client-made
//! `idempotency_key`: sending the same request again (a retry after a lost answer) returns the first result and
//! posts nothing twice. Reusing a key for a different request is refused.
//!
//! What a person may see is limited to the locations they can use: with `locations.all` that is every location,
//! otherwise only the ones assigned to them (the totals add up only what they can see).

use crate::catalog::{self, Product};
use crate::master::{clean, clean_opt, enqueue};
use crate::stock::{self, clean_key, guard_location, post_all, require_location, require_product, Planned, MAX_QTY};
use crate::users::{audit_entity, AccessContext, Principal, UserError};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use std::collections::BTreeMap;

// ---------- operations: restock and adjustments ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StockOperation {
    pub id: String,
    pub kind: String,
    pub location_id: String,
    pub product_id: String,
    pub qty: i64,
    pub note: Option<String>,
    pub reference: Option<String>,
    pub actor_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpOutcome {
    pub operation: StockOperation,
    /// True when this request had been made before (same idempotency key): nothing new was posted.
    pub duplicate: bool,
}

fn op_row(r: &Row) -> rusqlite::Result<StockOperation> {
    Ok(StockOperation { id: r.get(0)?, kind: r.get(1)?, location_id: r.get(2)?, product_id: r.get(3)?, qty: r.get(4)?, note: r.get(5)?, reference: r.get(6)?, actor_id: r.get(7)?, created_at: r.get(8)? })
}

const OP_COLS: &str = "id,kind,location_id,product_id,qty,note,reference,actor_id,created_at";

fn find_operation(c: &Connection, tenant: &str, key: &str) -> Result<Option<StockOperation>, rusqlite::Error> {
    c.query_row(&format!("SELECT {OP_COLS} FROM stock_operations WHERE tenant_id=?1 AND idempotency_key=?2"), params![tenant, key], op_row).optional()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustKind {
    /// Signed quantity: + stock found, - stock lost.
    Correction,
    /// Good stock becomes damaged stock.
    Damage,
    /// Damaged stock is thrown away.
    WriteOff,
}

impl AdjustKind {
    fn name(self) -> &'static str {
        match self {
            AdjustKind::Correction => "correction",
            AdjustKind::Damage => "damage",
            AdjustKind::WriteOff => "write_off",
        }
    }
}

struct OpRequest<'a> {
    kind: &'static str,
    location_id: &'a str,
    product_id: &'a str,
    qty: i64,
    note: Option<String>,
    reference: Option<String>,
    key: &'a str,
    product_must_be_active: bool,
}

fn run_operation(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    req: OpRequest,
    plan: impl Fn(&str) -> Result<Vec<Planned>, UserError>,
    now: &str,
) -> Result<OpOutcome, UserError> {
    let key = clean_key(req.key)?;
    guard_location(actor, req.location_id)?;
    let tx = conn.transaction()?;
    if let Some(prev) = find_operation(&tx, tenant, &key)? {
        let same = prev.kind == req.kind && prev.location_id == req.location_id && prev.product_id == req.product_id && prev.qty == req.qty;
        return if same { Ok(OpOutcome { operation: prev, duplicate: true }) } else { Err(UserError::Conflict("that idempotency key was already used for a different request")) };
    }
    require_location(&tx, tenant, req.location_id)?;
    require_product(&tx, tenant, req.product_id, req.product_must_be_active)?;
    let planned = plan(&key)?;
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO stock_operations(id,tenant_id,kind,location_id,product_id,qty,note,reference,actor_id,device_id,idempotency_key,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![id, tenant, req.kind, req.location_id, req.product_id, req.qty, req.note, req.reference, actor.user_id, device, key, now],
    )?;
    post_all(&tx, tenant, actor, device, req.kind, &id, &planned, now)?;
    let op = tx.query_row(&format!("SELECT {OP_COLS} FROM stock_operations WHERE id=?1"), [&id], op_row)?;
    let snap = serde_json::to_value(&op).unwrap_or(serde_json::Value::Null);
    audit_entity(&tx, tenant, Some(&actor.user_id), device, &format!("stock.{}", req.kind), "stock_operation", Some(&id), None, Some(&snap.to_string()), now)?;
    enqueue(&tx, tenant, "stock_operation", &id, "insert", &snap, None, 1)?;
    tx.commit()?;
    Ok(OpOutcome { operation: op, duplicate: false })
}

pub struct ReceiveRequest<'a> {
    pub location_id: &'a str,
    pub product_id: &'a str,
    pub qty: i64,
    pub note: Option<&'a str>,
    /// For example the supplier's bill number.
    pub reference: Option<&'a str>,
}

/// Stock arrives at a location (a purchase or an opening balance).
pub fn receive_stock(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, r: &ReceiveRequest, key: &str, now: &str) -> Result<OpOutcome, UserError> {
    let note = clean_opt(r.note, 200, "note is too long")?;
    let reference = clean_opt(r.reference, 60, "reference is too long")?;
    run_operation(
        conn, tenant, actor, device,
        OpRequest { kind: "receive", location_id: r.location_id, product_id: r.product_id, qty: r.qty, note, reference, key, product_must_be_active: true },
        |k| stock::plan_receive(r.location_id, r.product_id, r.qty, k),
        now,
    )
}

pub struct AdjustRequest<'a> {
    pub kind: AdjustKind,
    pub location_id: &'a str,
    pub product_id: &'a str,
    pub qty: i64,
    /// Why. Required: every adjustment has a reason on record.
    pub reason: &'a str,
}

pub fn adjust_stock(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, r: &AdjustRequest, key: &str, now: &str) -> Result<OpOutcome, UserError> {
    let reason = clean(r.reason, 3, 200, "give a reason of 3 to 200 characters")?;
    let kind = r.kind.name();
    run_operation(
        conn, tenant, actor, device,
        OpRequest { kind, location_id: r.location_id, product_id: r.product_id, qty: r.qty, note: Some(reason), reference: None, key, product_must_be_active: false },
        |k| match r.kind {
            AdjustKind::Correction => stock::plan_correct(r.location_id, r.product_id, r.qty, k),
            AdjustKind::Damage => stock::plan_damage(r.location_id, r.product_id, r.qty, k),
            AdjustKind::WriteOff => stock::plan_write_off(r.location_id, r.product_id, r.qty, k),
        },
        now,
    )
}

/// Set (or with `None` clear) the level at which a product counts as low at a location.
pub fn set_reorder_level(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, location_id: &str, product_id: &str, level: Option<i64>, now: &str) -> Result<(), UserError> {
    if let Some(l) = level {
        if !(0..=MAX_QTY).contains(&l) {
            return Err(UserError::BadInput("reorder level must be a whole number from 0 to 1,000,000"));
        }
    }
    guard_location(actor, location_id)?;
    let tx = conn.transaction()?;
    require_location(&tx, tenant, location_id)?;
    require_product(&tx, tenant, product_id, false)?;
    let before: Option<i64> = tx
        .query_row("SELECT reorder_level FROM inventory_balances WHERE tenant_id=?1 AND location_id=?2 AND product_id=?3", params![tenant, location_id, product_id], |r| r.get(0))
        .optional()?
        .flatten();
    tx.execute(
        "INSERT INTO inventory_balances(tenant_id,location_id,product_id,reorder_level) VALUES (?1,?2,?3,?4)
         ON CONFLICT(location_id,product_id) DO UPDATE SET reorder_level=excluded.reorder_level",
        params![tenant, location_id, product_id, level],
    )?;
    let entity_id = format!("{location_id}:{product_id}");
    let (b, a) = (serde_json::json!({ "reorder_level": before }).to_string(), serde_json::json!({ "reorder_level": level }).to_string());
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "stock.reorder_level", "stock_level", Some(&entity_id), Some(&b), Some(&a), now)?;
    tx.commit()?;
    Ok(())
}

// ---------- stock levels ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocationQty {
    pub location_id: String,
    pub on_hand: i64,
    pub damaged: i64,
    pub in_transit: i64,
    pub reorder_level: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StockItem {
    pub product: Product,
    /// Only locations where something is recorded; a missing location means zero.
    pub by_location: Vec<LocationQty>,
    pub total_on_hand: i64,
    pub total_damaged: i64,
    pub total_in_transit: i64,
    /// True when some location is at or below its reorder level.
    pub low: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StockPage {
    pub items: Vec<StockItem>,
    pub total: i64,
}

#[derive(Debug, Clone, Default)]
pub struct StockQuery {
    pub search: Option<String>,
    pub category_id: Option<String>,
    pub brand_id: Option<String>,
    pub company_id: Option<String>,
    /// Show only this location's quantities.
    pub location_id: Option<String>,
    pub low_only: bool,
    pub limit: i64,
    pub offset: i64,
}

/// `None` means every location; `Some(json array)` is the list the person may see.
fn visible_json(access: &AccessContext) -> Option<String> {
    if access.all_locations {
        None
    } else {
        Some(serde_json::to_string(&access.location_ids).unwrap_or_else(|_| "[]".into()))
    }
}

/// Products that match the search and the filters, with their quantities at the locations the caller may see.
/// Active products are listed; an inactive product is listed only while it still holds stock.
pub fn stock_levels(conn: &Connection, tenant: &str, access: &AccessContext, q: &StockQuery) -> Result<StockPage, UserError> {
    if let Some(l) = &q.location_id {
        if !access.all_locations && !access.location_ids.contains(l) {
            return Err(UserError::Forbidden("location_denied"));
        }
    }
    let visible = visible_json(access);
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(60).collect::<String>());
    let limit = if q.limit <= 0 { 50 } else { q.limit.min(200) };
    let offset = q.offset.max(0);
    let stock_filter = catalog::FILTER.replace(
        "(?6=1 OR p.is_active=1)",
        "(p.is_active=1 OR EXISTS (SELECT 1 FROM inventory_balances z WHERE z.tenant_id=p.tenant_id AND z.product_id=p.id AND (z.on_hand<>0 OR z.damaged<>0 OR z.in_transit<>0)))",
    );
    let low_clause = "AND (?9=0 OR EXISTS (SELECT 1 FROM inventory_balances ib WHERE ib.tenant_id=p.tenant_id AND ib.product_id=p.id
                       AND ib.reorder_level IS NOT NULL AND ib.on_hand <= ib.reorder_level
                       AND (?10 IS NULL OR ib.location_id IN (SELECT value FROM json_each(?10))) AND (?11 IS NULL OR ib.location_id=?11)))";
    let where_sql = format!("{stock_filter} {low_clause}");
    let args = params![tenant, search, q.category_id, q.brand_id, q.company_id, 0, limit, offset, q.low_only as i64, visible, q.location_id];
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {} WHERE {where_sql}", catalog::FROM), args, |r| r.get(0))?;
    let mut st = conn.prepare(&format!("SELECT {} FROM {} WHERE {where_sql} ORDER BY lower(p.name), p.sku LIMIT ?7 OFFSET ?8", catalog::PCOLS, catalog::FROM))?;
    let products: Vec<Product> = st.query_map(args, catalog::prod_from_row)?.collect::<Result<_, _>>()?;

    let ids = serde_json::to_string(&products.iter().map(|p| p.id.clone()).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into());
    let mut by: BTreeMap<String, Vec<LocationQty>> = BTreeMap::new();
    let mut st = conn.prepare(
        "SELECT product_id, location_id, on_hand, damaged, in_transit, reorder_level FROM inventory_balances
         WHERE tenant_id=?1 AND product_id IN (SELECT value FROM json_each(?2))
           AND (?3 IS NULL OR location_id IN (SELECT value FROM json_each(?3))) AND (?4 IS NULL OR location_id=?4)
           AND (on_hand<>0 OR damaged<>0 OR in_transit<>0 OR reorder_level IS NOT NULL)
         ORDER BY location_id",
    )?;
    let rows = st.query_map(params![tenant, ids, visible, q.location_id], |r| {
        Ok((r.get::<_, String>(0)?, LocationQty { location_id: r.get(1)?, on_hand: r.get(2)?, damaged: r.get(3)?, in_transit: r.get(4)?, reorder_level: r.get(5)? }))
    })?;
    for row in rows {
        let (p, l) = row?;
        by.entry(p).or_default().push(l);
    }
    let items = products
        .into_iter()
        .map(|product| {
            let by_location = by.remove(&product.id).unwrap_or_default();
            StockItem {
                total_on_hand: by_location.iter().map(|l| l.on_hand).sum(),
                total_damaged: by_location.iter().map(|l| l.damaged).sum(),
                total_in_transit: by_location.iter().map(|l| l.in_transit).sum(),
                low: by_location.iter().any(|l| l.reorder_level.is_some_and(|lv| l.on_hand <= lv)),
                by_location,
                product,
            }
        })
        .collect();
    Ok(StockPage { items, total })
}

// ---------- movement history ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MovementRow {
    pub id: String,
    pub location_id: String,
    pub product_id: String,
    pub bucket: String,
    pub qty_delta: i64,
    pub movement_type: String,
    pub source_type: String,
    pub source_id: String,
    pub actor_name: Option<String>,
    pub occurred_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MovementPage {
    pub items: Vec<MovementRow>,
    pub total: i64,
}

#[derive(Debug, Clone, Default)]
pub struct MovementQuery {
    pub product_id: Option<String>,
    pub location_id: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// Newest first, limited to the locations the caller may see.
pub fn movements(conn: &Connection, tenant: &str, access: &AccessContext, q: &MovementQuery) -> Result<MovementPage, UserError> {
    if let Some(l) = &q.location_id {
        if !access.all_locations && !access.location_ids.contains(l) {
            return Err(UserError::Forbidden("location_denied"));
        }
    }
    let visible = visible_json(access);
    let limit = if q.limit <= 0 { 50 } else { q.limit.min(200) };
    let filter = "m.tenant_id=?1 AND (?2 IS NULL OR m.product_id=?2) AND (?3 IS NULL OR m.location_id=?3)
                  AND (?4 IS NULL OR m.location_id IN (SELECT value FROM json_each(?4)))";
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM stock_movements m WHERE {filter}"), params![tenant, q.product_id, q.location_id, visible], |r| r.get(0))?;
    let mut st = conn.prepare(&format!(
        "SELECT m.id, m.location_id, m.product_id, m.bucket, m.qty_delta, m.movement_type, m.source_type, m.source_id, u.full_name, m.occurred_at
         FROM stock_movements m LEFT JOIN users u ON u.tenant_id=m.tenant_id AND u.id=m.actor_id
         WHERE {filter} ORDER BY m.occurred_at DESC, m.rowid DESC LIMIT ?5 OFFSET ?6"
    ))?;
    let rows = st.query_map(params![tenant, q.product_id, q.location_id, visible, limit, q.offset.max(0)], |r| {
        Ok(MovementRow { id: r.get(0)?, location_id: r.get(1)?, product_id: r.get(2)?, bucket: r.get(3)?, qty_delta: r.get(4)?, movement_type: r.get(5)?, source_type: r.get(6)?, source_id: r.get(7)?, actor_name: r.get(8)?, occurred_at: r.get(9)? })
    })?;
    Ok(MovementPage { items: rows.collect::<Result<_, _>>()?, total })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brands;
    use crate::catalog::{create_product, set_product_active, ProductInput};
    use crate::master::testutil::*;
    use crate::stock::{bucket_balance, ON_HAND};

    const OWNER: &[&str] = &["*"];

    /// Locations main/g1/g2 and products p1..p3 (ids are the codes in lower case).
    fn shop() -> Connection {
        let c = setup();
        c.execute_batch(
            "INSERT INTO stock_locations(id,tenant_id,name,code) VALUES ('main','t1','Main Shop','MAIN'),('g1','t1','Godown 1','G1'),('g2','t1','Godown 2','G2');
             INSERT INTO products(id,tenant_id,sku,name) VALUES ('p1','t1','BP-100','Brake pad'),('p2','t1','OF-10','Oil filter'),('p3','t1','SP-01','Spark plug');",
        )
        .unwrap();
        c
    }

    fn me() -> Principal {
        actor(OWNER, &[])
    }

    fn recv(c: &mut Connection, loc: &str, prod: &str, qty: i64, key: &str) -> Result<OpOutcome, UserError> {
        receive_stock(c, "t1", &me(), Some("d1"), &ReceiveRequest { location_id: loc, product_id: prod, qty, note: None, reference: None }, key, T0)
    }

    fn adj(c: &mut Connection, kind: AdjustKind, loc: &str, prod: &str, qty: i64, key: &str) -> Result<OpOutcome, UserError> {
        adjust_stock(c, "t1", &me(), Some("d1"), &AdjustRequest { kind, location_id: loc, product_id: prod, qty, reason: "stock check" }, key, T0)
    }

    fn bal(c: &Connection, loc: &str, prod: &str, b: &str) -> i64 {
        bucket_balance(c, "t1", loc, prod, b).unwrap()
    }

    fn n(c: &Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn restocking_adds_stock_and_records_who_where_and_how_many() {
        let mut c = shop();
        let out = receive_stock(&mut c, "t1", &me(), Some("d1"), &ReceiveRequest { location_id: "g1", product_id: "p1", qty: 12, note: Some(" first delivery "), reference: Some("BILL-77") }, "key-restock-1", T0).unwrap();
        assert!(!out.duplicate);
        assert_eq!((out.operation.kind.as_str(), out.operation.qty, out.operation.note.as_deref(), out.operation.reference.as_deref()), ("receive", 12, Some("first delivery"), Some("BILL-77")));
        assert_eq!(bal(&c, "g1", "p1", ON_HAND), 12);
        let (ty, src_type, src_id): (String, String, String) =
            c.query_row("SELECT movement_type, source_type, source_id FROM stock_movements", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((ty.as_str(), src_type.as_str(), src_id.as_str()), ("purchase", "receive", out.operation.id.as_str()));
        assert!(audit(&c).iter().any(|a| a.0 == "stock.receive"));
        let ob = outbox(&c);
        assert!(ob.iter().any(|o| o.0 == "stock_movement" && o.1 == "post") && ob.iter().any(|o| o.0 == "stock_operation" && o.1 == "insert"));
    }

    #[test]
    fn a_retry_with_the_same_key_posts_nothing_twice_and_a_clashing_key_is_refused() {
        let mut c = shop();
        let first = recv(&mut c, "g1", "p1", 5, "key-retry-001").unwrap();
        let again = recv(&mut c, "g1", "p1", 5, "key-retry-001").unwrap();
        assert!(again.duplicate);
        assert_eq!(again.operation, first.operation);
        assert_eq!(bal(&c, "g1", "p1", ON_HAND), 5, "stock was added once");
        assert_eq!(n(&c, "stock_movements"), 1);
        assert_eq!(n(&c, "stock_operations"), 1);
        // the same key for a different request is a mistake, not a retry
        assert!(matches!(recv(&mut c, "g1", "p1", 6, "key-retry-001"), Err(UserError::Conflict(_))));
        assert!(matches!(recv(&mut c, "g2", "p1", 5, "key-retry-001"), Err(UserError::Conflict(_))));
        assert!(matches!(adj(&mut c, AdjustKind::Correction, "g1", "p1", 5, "key-retry-001"), Err(UserError::Conflict(_))));
        for bad in ["short", "has spaces in it", ""] {
            assert!(matches!(recv(&mut c, "g1", "p1", 1, bad), Err(UserError::BadInput(_))), "{bad:?}");
        }
        assert_eq!(bal(&c, "g1", "p1", ON_HAND), 5);
    }

    #[test]
    fn a_retry_still_succeeds_after_stock_has_since_changed() {
        let mut c = shop();
        adj(&mut c, AdjustKind::Correction, "main", "p1", 5, "key-seed-0001").unwrap();
        let sub = adj(&mut c, AdjustKind::Correction, "main", "p1", -5, "key-minus-005").unwrap();
        assert!(!sub.duplicate);
        // the first answer was lost; the retry must not now fail with "not enough stock"
        let retry = adj(&mut c, AdjustKind::Correction, "main", "p1", -5, "key-minus-005").unwrap();
        assert!(retry.duplicate);
        assert_eq!(bal(&c, "main", "p1", ON_HAND), 0);
    }

    #[test]
    fn restocking_validates_quantity_product_and_location() {
        let mut c = shop();
        for q in [0, -3, 1_000_001] {
            assert!(matches!(recv(&mut c, "g1", "p1", q, &format!("key-q-{q:>8}")), Err(UserError::BadInput(_))), "{q}");
        }
        assert!(matches!(recv(&mut c, "nowhere", "p1", 1, "key-unknown-loc"), Err(UserError::BadInput(_))));
        assert!(matches!(recv(&mut c, "g1", "ghost", 1, "key-unknown-prd"), Err(UserError::BadInput(_))));
        c.execute("UPDATE stock_locations SET is_active=0 WHERE id='g2'", []).unwrap();
        assert!(matches!(recv(&mut c, "g2", "p1", 1, "key-inactive-lo"), Err(UserError::Conflict(_))));
        let p = crate::catalog::get_product(&c, "t1", "p3").unwrap().unwrap();
        set_product_active(&mut c, "t1", &me(), None, "p3", p.revision, false, T0).unwrap();
        assert!(matches!(recv(&mut c, "g1", "p3", 1, "key-inactive-pr"), Err(UserError::Conflict(_))), "cannot restock a product that is switched off");
        assert_eq!(n(&c, "stock_movements"), 0, "every refusal left the ledger alone");
        assert_eq!(n(&c, "stock_operations"), 0);
    }

    #[test]
    fn people_can_only_work_in_locations_they_may_use() {
        let mut c = shop();
        let limited = actor(&["inventory.adjust"], &["g1"]);
        let r = |c: &mut Connection, loc: &str| receive_stock(c, "t1", &limited, None, &ReceiveRequest { location_id: loc, product_id: "p1", qty: 1, note: None, reference: None }, &format!("key-scope-{loc}-1"), T0);
        assert!(r(&mut c, "g1").is_ok());
        assert!(matches!(r(&mut c, "main"), Err(UserError::Forbidden("location_denied"))));
        let a = adjust_stock(&mut c, "t1", &limited, None, &AdjustRequest { kind: AdjustKind::Correction, location_id: "g2", product_id: "p1", qty: 1, reason: "found" }, "key-scope-adj-01", T0);
        assert!(matches!(a, Err(UserError::Forbidden("location_denied"))));
        assert_eq!(n(&c, "stock_movements"), 1);
    }

    #[test]
    fn adjustments_work_and_never_take_a_bucket_below_zero() {
        let mut c = shop();
        recv(&mut c, "main", "p1", 10, "key-seed-main1").unwrap();
        adj(&mut c, AdjustKind::Correction, "main", "p1", 3, "key-corr-up-01").unwrap();
        adj(&mut c, AdjustKind::Correction, "main", "p1", -2, "key-corr-dn-01").unwrap();
        assert_eq!(bal(&c, "main", "p1", ON_HAND), 11);
        adj(&mut c, AdjustKind::Damage, "main", "p1", 4, "key-damage-001").unwrap();
        assert_eq!((bal(&c, "main", "p1", ON_HAND), bal(&c, "main", "p1", "damaged")), (7, 4));
        adj(&mut c, AdjustKind::WriteOff, "main", "p1", 3, "key-writeoff-1").unwrap();
        assert_eq!((bal(&c, "main", "p1", ON_HAND), bal(&c, "main", "p1", "damaged")), (7, 1));

        let (ops, moves, audits, queued) = (n(&c, "stock_operations"), n(&c, "stock_movements"), n(&c, "audit_logs"), n(&c, "sync_outbox"));
        let short = |r: Result<OpOutcome, UserError>| match r {
            Err(UserError::Short { have, need, .. }) => (have, need),
            other => panic!("expected a shortage, got {other:?}"),
        };
        assert_eq!(short(adj(&mut c, AdjustKind::Correction, "main", "p1", -8, "key-too-many-1")), (7, 8));
        assert_eq!(short(adj(&mut c, AdjustKind::Damage, "main", "p1", 8, "key-too-many-2")), (7, 8));
        assert_eq!(short(adj(&mut c, AdjustKind::WriteOff, "main", "p1", 2, "key-too-many-3")), (1, 2));
        assert_eq!((n(&c, "stock_operations"), n(&c, "stock_movements"), n(&c, "audit_logs"), n(&c, "sync_outbox")), (ops, moves, audits, queued), "a refused adjustment leaves no trace");
    }

    #[test]
    fn a_damage_that_fails_halfway_leaves_nothing_behind() {
        let mut c = shop();
        recv(&mut c, "main", "p1", 2, "key-seed-main2").unwrap();
        assert!(adj(&mut c, AdjustKind::Damage, "main", "p1", 3, "key-damage-big1").is_err());
        assert_eq!((bal(&c, "main", "p1", ON_HAND), bal(&c, "main", "p1", "damaged")), (2, 0));
    }

    #[test]
    fn every_adjustment_needs_a_reason_and_inactive_products_can_still_be_adjusted() {
        let mut c = shop();
        recv(&mut c, "main", "p1", 5, "key-seed-main3").unwrap();
        for reason in ["", "  ", "ab"] {
            let r = adjust_stock(&mut c, "t1", &me(), None, &AdjustRequest { kind: AdjustKind::Correction, location_id: "main", product_id: "p1", qty: -1, reason }, "key-reason-0001", T0);
            assert!(matches!(r, Err(UserError::BadInput(_))), "{reason:?}");
        }
        let p = crate::catalog::get_product(&c, "t1", "p1").unwrap().unwrap();
        set_product_active(&mut c, "t1", &me(), None, "p1", p.revision, false, T0).unwrap();
        adj(&mut c, AdjustKind::Correction, "main", "p1", -5, "key-inactive-adj").unwrap();
        assert_eq!(bal(&c, "main", "p1", ON_HAND), 0);
    }

    #[test]
    fn reorder_levels_flag_low_stock_and_can_be_cleared() {
        let mut c = shop();
        recv(&mut c, "main", "p1", 4, "key-seed-main4").unwrap();
        recv(&mut c, "main", "p2", 40, "key-seed-main5").unwrap();
        set_reorder_level(&mut c, "t1", &me(), None, "main", "p1", Some(5), T0).unwrap();
        set_reorder_level(&mut c, "t1", &me(), None, "main", "p2", Some(5), T0).unwrap();
        let low = stock_levels(&c, "t1", &me().access(), &StockQuery { low_only: true, ..Default::default() }).unwrap();
        assert_eq!(low.items.iter().map(|i| i.product.id.as_str()).collect::<Vec<_>>(), ["p1"]);
        assert!(low.items[0].low);
        let all = stock_levels(&c, "t1", &me().access(), &StockQuery::default()).unwrap();
        assert_eq!(all.items.iter().filter(|i| i.low).count(), 1);
        set_reorder_level(&mut c, "t1", &me(), None, "main", "p1", None, T0).unwrap();
        assert_eq!(stock_levels(&c, "t1", &me().access(), &StockQuery { low_only: true, ..Default::default() }).unwrap().total, 0);
        // a level can be set before any stock exists, and bad levels are refused
        set_reorder_level(&mut c, "t1", &me(), None, "g2", "p3", Some(2), T0).unwrap();
        assert!(matches!(set_reorder_level(&mut c, "t1", &me(), None, "g2", "p3", Some(-1), T0), Err(UserError::BadInput(_))));
        assert!(matches!(set_reorder_level(&mut c, "t1", &me(), None, "g2", "ghost", Some(1), T0), Err(UserError::BadInput(_))));
        let level: Option<i64> = c.query_row("SELECT reorder_level FROM inventory_balances WHERE location_id='g2' AND product_id='p3'", [], |r| r.get(0)).unwrap();
        assert_eq!(level, Some(2));
        assert!(audit(&c).iter().any(|a| a.0 == "stock.reorder_level"));
    }

    #[test]
    fn stock_levels_group_by_location_and_total_what_the_caller_can_see() {
        let mut c = shop();
        recv(&mut c, "main", "p1", 5, "key-lvl-main-p1").unwrap();
        recv(&mut c, "g1", "p1", 20, "key-lvl-g1-p1a").unwrap();
        recv(&mut c, "g2", "p1", 7, "key-lvl-g2-p1a").unwrap();
        adj(&mut c, AdjustKind::Damage, "g1", "p1", 2, "key-lvl-damage1").unwrap();
        c.execute("INSERT INTO inventory_balances(tenant_id,location_id,product_id,in_transit) VALUES ('t1','main','p2',3)", []).unwrap();

        let all = stock_levels(&c, "t1", &me().access(), &StockQuery::default()).unwrap();
        assert_eq!(all.total, 3, "zero-stock products are listed too, so they can be restocked");
        let p1 = all.items.iter().find(|i| i.product.id == "p1").unwrap();
        assert_eq!((p1.total_on_hand, p1.total_damaged, p1.total_in_transit), (30, 2, 0));
        assert_eq!(p1.by_location.iter().map(|l| (l.location_id.as_str(), l.on_hand)).collect::<Vec<_>>(), [("g1", 18), ("g2", 7), ("main", 5)]);
        assert_eq!(all.items.iter().find(|i| i.product.id == "p2").unwrap().total_in_transit, 3);
        assert!(all.items.iter().find(|i| i.product.id == "p3").unwrap().by_location.is_empty());

        // someone limited to the shop sees only the shop's quantities, and cannot ask about the godowns
        let shop_only = actor(&["inventory.view"], &["main"]).access();
        let mine = stock_levels(&c, "t1", &shop_only, &StockQuery::default()).unwrap();
        let p1 = mine.items.iter().find(|i| i.product.id == "p1").unwrap();
        assert_eq!((p1.total_on_hand, p1.by_location.len()), (5, 1), "the godowns' stock is not revealed in the total either");
        assert!(matches!(stock_levels(&c, "t1", &shop_only, &StockQuery { location_id: Some("g1".into()), ..Default::default() }), Err(UserError::Forbidden("location_denied"))));
        let one = stock_levels(&c, "t1", &me().access(), &StockQuery { location_id: Some("g1".into()), ..Default::default() }).unwrap();
        assert_eq!(one.items.iter().find(|i| i.product.id == "p1").unwrap().total_on_hand, 18, "a location filter shows that location only");
        let nobody = actor(&["inventory.view"], &[]).access();
        assert!(stock_levels(&c, "t1", &nobody, &StockQuery::default()).unwrap().items.iter().all(|i| i.by_location.is_empty()));
    }

    #[test]
    fn stock_search_uses_the_product_search_and_hides_dead_products() {
        let mut c = shop();
        let co = brands::create_company(&mut c, "t1", &me(), None, "Atlas Honda", T0).unwrap();
        let br = brands::create_brand(&mut c, "t1", &me(), None, "Honda Genuine", Some(&co.id), T0).unwrap();
        let hp = create_product(&mut c, "t1", &me(), None, &ProductInput { sku: "HG-1".into(), name: "Honda chain".into(), name_ur: Some("چین".into()), brand_id: Some(br.id.clone()), ..Default::default() }, T0).unwrap();
        recv(&mut c, "main", &hp.id, 3, "key-search-seed1").unwrap();
        let q = |c: &Connection, s: &str| stock_levels(c, "t1", &me().access(), &StockQuery { search: Some(s.into()), ..Default::default() }).unwrap();
        assert_eq!(q(&c, "genuine").items.len(), 1, "by brand");
        assert_eq!(q(&c, "چین").items.len(), 1, "by Urdu name");
        assert_eq!(q(&c, "atlas").items[0].total_on_hand, 3, "by company");
        let by_company = stock_levels(&c, "t1", &me().access(), &StockQuery { company_id: Some(co.id.clone()), ..Default::default() }).unwrap();
        assert_eq!(by_company.total, 1);
        // a switched-off product disappears from stock, unless it still holds stock
        set_product_active(&mut c, "t1", &me(), None, "p2", 1, false, T0).unwrap();
        set_product_active(&mut c, "t1", &me(), None, &hp.id, hp.revision, false, T0).unwrap();
        let ids: Vec<String> = stock_levels(&c, "t1", &me().access(), &StockQuery::default()).unwrap().items.into_iter().map(|i| i.product.id).collect();
        assert!(!ids.contains(&"p2".to_string()) && ids.contains(&hp.id), "{ids:?}");
        // paging
        let page = stock_levels(&c, "t1", &me().access(), &StockQuery { limit: 1, offset: 1, ..Default::default() }).unwrap();
        assert_eq!((page.items.len(), page.total), (1, 3));
    }

    #[test]
    fn movement_history_is_newest_first_filterable_and_limited_to_visible_locations() {
        let mut c = shop();
        recv(&mut c, "main", "p1", 5, "key-hist-main-01").unwrap();
        recv(&mut c, "g1", "p1", 9, "key-hist-g1-0001").unwrap();
        adj(&mut c, AdjustKind::Correction, "main", "p1", -1, "key-hist-adjust1").unwrap();
        recv(&mut c, "main", "p2", 2, "key-hist-main-02").unwrap();
        let all = movements(&c, "t1", &me().access(), &MovementQuery::default()).unwrap();
        assert_eq!(all.total, 4);
        assert_eq!(all.items.first().unwrap().product_id, "p2", "newest first");
        assert_eq!(all.items.last().unwrap().qty_delta, 5);
        assert_eq!(all.items[0].actor_name.as_deref(), Some("User One"), "who did it");
        let p1 = movements(&c, "t1", &me().access(), &MovementQuery { product_id: Some("p1".into()), ..Default::default() }).unwrap();
        assert_eq!(p1.total, 3);
        let shop_only = actor(&["inventory.view"], &["main"]).access();
        let mine = movements(&c, "t1", &shop_only, &MovementQuery { product_id: Some("p1".into()), ..Default::default() }).unwrap();
        assert_eq!(mine.total, 2, "the godown's movement is not shown");
        assert!(matches!(movements(&c, "t1", &shop_only, &MovementQuery { location_id: Some("g1".into()), ..Default::default() }), Err(UserError::Forbidden("location_denied"))));
        let paged = movements(&c, "t1", &me().access(), &MovementQuery { limit: 2, offset: 2, ..Default::default() }).unwrap();
        assert_eq!((paged.items.len(), paged.total), (2, 4));
    }

    #[test]
    fn other_shops_stock_is_invisible() {
        let mut c = shop();
        c.execute_batch(
            "INSERT INTO tenants(id,name) VALUES ('t2','Other');
             INSERT INTO stock_locations(id,tenant_id,name,code) VALUES ('o1','t2','Other Loc','O1');
             INSERT INTO products(id,tenant_id,sku,name) VALUES ('op1','t2','OP-1','Other part');",
        )
        .unwrap();
        recv(&mut c, "main", "p1", 5, "key-iso-0000001").unwrap();
        receive_stock(&mut c, "t2", &me(), None, &ReceiveRequest { location_id: "o1", product_id: "op1", qty: 9, note: None, reference: None }, "key-iso-0000002", T0).unwrap();
        assert_eq!(stock_levels(&c, "t1", &me().access(), &StockQuery::default()).unwrap().total, 3);
        assert_eq!(movements(&c, "t1", &me().access(), &MovementQuery::default()).unwrap().total, 1);
        // a shop cannot post into another shop's location or product
        assert!(matches!(recv(&mut c, "o1", "p1", 1, "key-iso-0000003"), Err(UserError::BadInput(_))));
        assert!(matches!(recv(&mut c, "main", "op1", 1, "key-iso-0000004"), Err(UserError::BadInput(_))));
    }
}
