//! Customers.
//!
//! * A customer code is optional on creation: when blank the server assigns the next free `C00001`,
//!   `C00002`... Codes are upper-case (`A-Z 0-9 _ -`, 2 to 16 characters) and unique per shop.
//! * Customers are deactivated, never deleted (a trigger enforces it): sales and the accounts ledger
//!   will refer to them for ever.
//! * Credit limits, balances and routes are not here yet: they arrive with sales/accounts (step 5)
//!   and routes (step 10).

use crate::master::{check_revision, clean, clean_opt, code, enqueue};
use crate::users::{audit_entity, Principal, UserError};
use crate::DbError;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Customer {
    pub id: String,
    pub code: String,
    pub name: String,
    pub phone: Option<String>,
    pub address: Option<String>,
    pub notes: Option<String>,
    pub is_active: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Default)]
pub struct CustomerInput {
    /// Blank means "assign the next free code".
    pub code: String,
    pub name: String,
    pub phone: Option<String>,
    pub address: Option<String>,
    pub notes: Option<String>,
}

const COLS: &str = "id,code,name,phone,address,notes,is_active,revision";

fn from_row(r: &Row) -> rusqlite::Result<Customer> {
    Ok(Customer {
        id: r.get(0)?,
        code: r.get(1)?,
        name: r.get(2)?,
        phone: r.get(3)?,
        address: r.get(4)?,
        notes: r.get(5)?,
        is_active: r.get::<_, i64>(6)? == 1,
        revision: r.get(7)?,
    })
}

fn snapshot(c: &Customer) -> serde_json::Value {
    serde_json::to_value(c).unwrap_or(serde_json::Value::Null)
}

struct Clean {
    code: Option<String>,
    name: String,
    phone: Option<String>,
    address: Option<String>,
    notes: Option<String>,
}

fn validate(i: &CustomerInput) -> Result<Clean, UserError> {
    let code_in = i.code.trim();
    let code = if code_in.is_empty() {
        None
    } else {
        Some(code(code_in, 2, 16, "_-", "code must be 2 to 16 characters: A-Z, 0-9, dash, underscore")?)
    };
    let phone = clean_opt(i.phone.as_deref(), 20, "phone must be 5 to 20 characters")?;
    if let Some(p) = &phone {
        let ok = p.chars().count() >= 5 && p.chars().all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | ' ' | '(' | ')'));
        if !ok {
            return Err(UserError::BadInput("phone must be 5 to 20 characters: digits, spaces, + - ( )"));
        }
    }
    Ok(Clean {
        code,
        name: clean(&i.name, 2, 120, "name must be 2 to 120 characters")?,
        phone,
        address: clean_opt(i.address.as_deref(), 300, "address is too long")?,
        notes: clean_opt(i.notes.as_deref(), 1000, "notes are too long")?,
    })
}

pub fn get_customer(conn: &Connection, tenant: &str, id: &str) -> Result<Option<Customer>, DbError> {
    Ok(conn.query_row(&format!("SELECT {COLS} FROM customers WHERE tenant_id=?1 AND id=?2"), params![tenant, id], from_row).optional()?)
}

#[derive(Debug, Clone, Default)]
pub struct CustomerQuery {
    pub search: Option<String>,
    pub include_inactive: bool,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CustomerPage {
    pub items: Vec<Customer>,
    pub total: i64,
}

const FILTER: &str = "tenant_id=?1
    AND (?2 IS NULL OR instr(lower(name),lower(?2))>0 OR instr(lower(code),lower(?2))>0 OR instr(COALESCE(phone,''),?2)>0)
    AND (?3=1 OR is_active=1)";

pub fn search_customers(conn: &Connection, tenant: &str, q: &CustomerQuery) -> Result<CustomerPage, DbError> {
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(60).collect::<String>());
    let limit = if q.limit <= 0 { 50 } else { q.limit.min(200) };
    let inc = q.include_inactive as i64;
    let total: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM customers WHERE {FILTER}"), params![tenant, search, inc], |r| r.get(0))?;
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM customers WHERE {FILTER} ORDER BY lower(name), code LIMIT ?4 OFFSET ?5"))?;
    let rows = st.query_map(params![tenant, search, inc, limit, q.offset.max(0)], from_row)?;
    Ok(CustomerPage { items: rows.collect::<Result<Vec<_>, _>>()?, total })
}

fn code_taken(c: &Connection, tenant: &str, code: &str, except: Option<&str>) -> Result<bool, rusqlite::Error> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM customers WHERE tenant_id=?1 AND code=?2 AND id<>COALESCE(?3,''))",
        params![tenant, code, except],
        |r| r.get(0),
    )
}

/// Next free `C00001`-style code. Runs inside the writer's transaction, so it cannot race.
fn next_code(tx: &Connection, tenant: &str) -> Result<String, rusqlite::Error> {
    let mut n: i64 = tx.query_row("SELECT COUNT(*) FROM customers WHERE tenant_id=?1", [tenant], |r| r.get(0))?;
    loop {
        n += 1;
        let candidate = format!("C{n:05}");
        if !code_taken(tx, tenant, &candidate, None)? {
            return Ok(candidate);
        }
    }
}

pub fn create_customer(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    input: &CustomerInput,
    now: &str,
) -> Result<Customer, UserError> {
    let v = validate(input)?;
    let tx = conn.transaction()?;
    let code = match v.code {
        Some(c) => {
            if code_taken(&tx, tenant, &c, None)? {
                return Err(UserError::Conflict("a customer with that code already exists"));
            }
            c
        }
        None => next_code(&tx, tenant)?,
    };
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO customers(id,tenant_id,code,name,phone,address,notes,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![id, tenant, code, v.name, v.phone, v.address, v.notes, now],
    )?;
    let cust = get_customer(&tx, tenant, &id).map_err(UserError::from)?.ok_or(UserError::NotFound)?;
    let snap = snapshot(&cust);
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "customer.create", "customer", Some(&id), None, Some(&snap.to_string()), now)?;
    enqueue(&tx, tenant, "customer", &id, "insert", &snap, None, cust.revision)?;
    tx.commit()?;
    Ok(cust)
}

/// Replace a customer's details. A blank `code` keeps the current one.
pub fn update_customer(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    id: &str,
    expected_revision: i64,
    input: &CustomerInput,
    now: &str,
) -> Result<Customer, UserError> {
    let v = validate(input)?;
    let tx = conn.transaction()?;
    let before = get_customer(&tx, tenant, id).map_err(UserError::from)?;
    check_revision(before.as_ref().map(|c| c.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    let code = v.code.unwrap_or_else(|| before.code.clone());
    if code_taken(&tx, tenant, &code, Some(id))? {
        return Err(UserError::Conflict("a customer with that code already exists"));
    }
    tx.execute(
        "UPDATE customers SET code=?3, name=?4, phone=?5, address=?6, notes=?7, revision=revision+1, updated_at=?8
         WHERE tenant_id=?1 AND id=?2",
        params![tenant, id, code, v.name, v.phone, v.address, v.notes, now],
    )?;
    let after = get_customer(&tx, tenant, id).map_err(UserError::from)?.ok_or(UserError::NotFound)?;
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "customer.update", "customer", Some(id), Some(&snapshot(&before).to_string()), Some(&snapshot(&after).to_string()), now)?;
    enqueue(&tx, tenant, "customer", id, "update", &snapshot(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

pub fn set_customer_active(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    id: &str,
    expected_revision: i64,
    active: bool,
    now: &str,
) -> Result<Customer, UserError> {
    let tx = conn.transaction()?;
    let before = get_customer(&tx, tenant, id).map_err(UserError::from)?;
    check_revision(before.as_ref().map(|c| c.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if before.is_active == active {
        return Ok(before);
    }
    tx.execute(
        "UPDATE customers SET is_active=?3, revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2",
        params![tenant, id, active as i64, now],
    )?;
    let after = get_customer(&tx, tenant, id).map_err(UserError::from)?.ok_or(UserError::NotFound)?;
    let action = if active { "customer.activate" } else { "customer.deactivate" };
    audit_entity(&tx, tenant, Some(&actor.user_id), device, action, "customer", Some(id), Some(&snapshot(&before).to_string()), Some(&snapshot(&after).to_string()), now)?;
    enqueue(&tx, tenant, "customer", id, if active { "update" } else { "archive" }, &snapshot(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::master::testutil::*;

    fn mgr() -> Principal {
        actor(&["customers.manage"], &[])
    }

    fn cin(name: &str) -> CustomerInput {
        CustomerInput { name: name.into(), ..Default::default() }
    }

    fn add(c: &mut Connection, name: &str) -> Customer {
        create_customer(c, "t1", &mgr(), Some("d1"), &cin(name), T0).unwrap()
    }

    #[test]
    fn codes_are_assigned_in_sequence_when_blank_and_respect_manual_ones() {
        let mut c = setup();
        assert_eq!(add(&mut c, "Ali Traders").code, "C00001");
        assert_eq!(add(&mut c, "Bilal Motors").code, "C00002");
        let manual = create_customer(&mut c, "t1", &mgr(), None, &CustomerInput { code: " vip-7 ".into(), ..cin("Chaudhry Autos") }, T0).unwrap();
        assert_eq!(manual.code, "VIP-7");
        // a manual code that collides with the next automatic one makes the sequence skip it
        create_customer(&mut c, "t1", &mgr(), None, &CustomerInput { code: "C00004".into(), ..cin("Sneaky") }, T0).unwrap();
        assert_eq!(add(&mut c, "Dawood Spares").code, "C00005");
        assert!(matches!(create_customer(&mut c, "t1", &mgr(), None, &CustomerInput { code: "vip-7".into(), ..cin("Dup") }, T0), Err(UserError::Conflict(_))));
    }

    #[test]
    fn fields_are_validated_and_normalized() {
        let mut c = setup();
        let good = create_customer(
            &mut c, "t1", &mgr(), Some("d1"),
            &CustomerInput { phone: Some(" +92 300-1234567 ".into()), address: Some("Shop 5, Market".into()), notes: Some("Pays on Fridays".into()), ..cin("  Ali Traders ") },
            T0,
        )
        .unwrap();
        assert_eq!((good.name.as_str(), good.phone.as_deref(), good.revision, good.is_active), ("Ali Traders", Some("+92 300-1234567"), 1, true));
        let blank = create_customer(&mut c, "t1", &mgr(), None, &CustomerInput { phone: Some("  ".into()), address: Some("".into()), ..cin("No Phone") }, T0).unwrap();
        assert_eq!((blank.phone, blank.address), (None, None));
        let mk = |c: &mut Connection, i: CustomerInput| create_customer(c, "t1", &mgr(), None, &i, T0);
        assert!(matches!(mk(&mut c, cin("x")), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, CustomerInput { phone: Some("12".into()), ..cin("Ok Name") }), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, CustomerInput { phone: Some("call me maybe".into()), ..cin("Ok Name") }), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, CustomerInput { code: "a".into(), ..cin("Ok Name") }), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, CustomerInput { code: "has space".into(), ..cin("Ok Name") }), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, CustomerInput { notes: Some("n".repeat(1001)), ..cin("Ok Name") }), Err(UserError::BadInput(_))));
        let n: i64 = c.query_row("SELECT COUNT(*) FROM customers", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2, "refusals created nothing");
    }

    #[test]
    fn updates_need_the_current_revision_and_a_blank_code_keeps_the_old_one() {
        let mut c = setup();
        let a = add(&mut c, "Ali Traders");
        let b = add(&mut c, "Bilal Motors");
        let up = update_customer(&mut c, "t1", &mgr(), None, &a.id, 1, &CustomerInput { phone: Some("0300 1112223".into()), ..cin("Ali Traders & Sons") }, T0).unwrap();
        assert_eq!((up.revision, up.code.as_str(), up.name.as_str()), (2, "C00001", "Ali Traders & Sons"));
        assert!(matches!(update_customer(&mut c, "t1", &mgr(), None, &a.id, 1, &cin("Stale"), T0), Err(UserError::Stale)));
        assert!(matches!(update_customer(&mut c, "t1", &mgr(), None, &b.id, 1, &CustomerInput { code: "c00001".into(), ..cin("Bilal Motors") }, T0), Err(UserError::Conflict(_))));
        assert!(matches!(update_customer(&mut c, "t1", &mgr(), None, "ghost", 1, &cin("Nobody"), T0), Err(UserError::NotFound)));
        let renamed = update_customer(&mut c, "t1", &mgr(), None, &b.id, 1, &CustomerInput { code: "bm-1".into(), ..cin("Bilal Motors") }, T0).unwrap();
        assert_eq!(renamed.code, "BM-1");
        let ob: Vec<_> = outbox(&c).into_iter().filter(|o| o.3.contains(&a.id)).collect();
        assert_eq!((ob[1].1.as_str(), ob[1].2, ob[1].3.as_str()), ("update", Some(1), format!("customer:{}:r2", a.id).as_str()));
    }

    #[test]
    fn customers_are_deactivated_never_deleted() {
        let mut c = setup();
        let a = add(&mut c, "Ali Traders");
        assert!(c.execute("DELETE FROM customers WHERE id=?1", [&a.id]).is_err(), "trigger refuses deletes");
        let off = set_customer_active(&mut c, "t1", &mgr(), None, &a.id, 1, false, T0).unwrap();
        assert_eq!((off.is_active, off.revision), (false, 2));
        assert_eq!(set_customer_active(&mut c, "t1", &mgr(), None, &a.id, 2, false, T0).unwrap().revision, 2, "no-op");
        assert!(matches!(set_customer_active(&mut c, "t1", &mgr(), None, &a.id, 1, true, T0), Err(UserError::Stale)));
        assert!(set_customer_active(&mut c, "t1", &mgr(), None, &a.id, 2, true, T0).unwrap().is_active);
        let ops: Vec<String> = outbox(&c).into_iter().map(|o| o.1).collect();
        assert_eq!(ops, ["insert", "archive", "update"]);
        let acts: Vec<String> = audit(&c).into_iter().map(|a| a.0).collect();
        assert_eq!(acts, ["customer.create", "customer.deactivate", "customer.activate"]);
    }

    #[test]
    fn search_matches_name_code_and_phone_and_hides_inactive() {
        let mut c = setup();
        create_customer(&mut c, "t1", &mgr(), None, &CustomerInput { phone: Some("0300-1234567".into()), ..cin("Ali Traders") }, T0).unwrap();
        let b = add(&mut c, "Bilal Motors");
        add(&mut c, "Chaudhry Autos");
        set_customer_active(&mut c, "t1", &mgr(), None, &b.id, 1, false, T0).unwrap();
        let q = |s: &str, inc: bool| search_customers(&c, "t1", &CustomerQuery { search: Some(s.into()), include_inactive: inc, ..Default::default() }).unwrap();
        assert_eq!(q("ALI", false).total, 1, "name, ignoring case");
        assert_eq!(q("c00003", false).items[0].name, "Chaudhry Autos", "code");
        assert_eq!(q("1234567", false).items[0].name, "Ali Traders", "phone fragment");
        assert_eq!(q("bilal", false).total, 0, "inactive hidden");
        assert_eq!(q("bilal", true).total, 1);
        assert_eq!(q("", false).total, 2);
        assert_eq!(q("100%", false).total, 0);
        let names: Vec<_> = search_customers(&c, "t1", &CustomerQuery::default()).unwrap().items.into_iter().map(|x| x.name).collect();
        assert_eq!(names, ["Ali Traders", "Chaudhry Autos"]);
    }

    #[test]
    fn customers_are_invisible_to_other_tenants() {
        let mut c = setup();
        c.execute("INSERT INTO tenants(id,name) VALUES ('t2','Other')", []).unwrap();
        add(&mut c, "Mine");
        let theirs = create_customer(&mut c, "t2", &mgr(), None, &cin("Theirs"), T0).unwrap();
        assert_eq!(theirs.code, "C00001", "each shop numbers its own customers");
        assert_eq!(search_customers(&c, "t1", &CustomerQuery::default()).unwrap().total, 1);
        assert!(get_customer(&c, "t1", &theirs.id).unwrap().is_none());
    }
}
