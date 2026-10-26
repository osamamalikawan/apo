//! Companies and brands.
//!
//! A *company* is who makes or supplies the goods; a *brand* is what the product is sold as, and may belong to a
//! company (a brand without a company is fine). Names are unique within a shop ignoring case. Both are renamed
//! and deactivated, never deleted. A product can only be given an *active* company or brand, but products that
//! already use one that was later deactivated keep it.

use crate::master::{check_revision, clean, enqueue};
use crate::users::{audit_entity, Principal, UserError};
use crate::DbError;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Company {
    pub id: String,
    pub name: String,
    pub is_active: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Brand {
    pub id: String,
    pub name: String,
    pub company_id: Option<String>,
    pub is_active: bool,
    pub revision: i64,
}

fn company_row(r: &Row) -> rusqlite::Result<Company> {
    Ok(Company { id: r.get(0)?, name: r.get(1)?, is_active: r.get::<_, i64>(2)? == 1, revision: r.get(3)? })
}
fn brand_row(r: &Row) -> rusqlite::Result<Brand> {
    Ok(Brand { id: r.get(0)?, name: r.get(1)?, company_id: r.get(2)?, is_active: r.get::<_, i64>(3)? == 1, revision: r.get(4)? })
}
fn snap<T: Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

pub fn list_companies(conn: &Connection, tenant: &str) -> Result<Vec<Company>, DbError> {
    let mut st = conn.prepare("SELECT id,name,is_active,revision FROM companies WHERE tenant_id=?1 ORDER BY lower(name)")?;
    let rows = st.query_map([tenant], company_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn list_brands(conn: &Connection, tenant: &str) -> Result<Vec<Brand>, DbError> {
    let mut st = conn.prepare("SELECT id,name,company_id,is_active,revision FROM brands WHERE tenant_id=?1 ORDER BY lower(name)")?;
    let rows = st.query_map([tenant], brand_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_company(c: &Connection, tenant: &str, id: &str) -> Result<Option<Company>, rusqlite::Error> {
    c.query_row("SELECT id,name,is_active,revision FROM companies WHERE tenant_id=?1 AND id=?2", params![tenant, id], company_row).optional()
}
pub fn get_brand(c: &Connection, tenant: &str, id: &str) -> Result<Option<Brand>, rusqlite::Error> {
    c.query_row("SELECT id,name,company_id,is_active,revision FROM brands WHERE tenant_id=?1 AND id=?2", params![tenant, id], brand_row).optional()
}

fn company_name_taken(c: &Connection, tenant: &str, name: &str, except: Option<&str>) -> Result<bool, rusqlite::Error> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM companies WHERE tenant_id=?1 AND lower(name)=lower(?2) AND id<>COALESCE(?3,''))", params![tenant, name, except], |r| r.get(0))
}
fn brand_name_taken(c: &Connection, tenant: &str, name: &str, except: Option<&str>) -> Result<bool, rusqlite::Error> {
    c.query_row("SELECT EXISTS(SELECT 1 FROM brands WHERE tenant_id=?1 AND lower(name)=lower(?2) AND id<>COALESCE(?3,''))", params![tenant, name, except], |r| r.get(0))
}

pub fn create_company(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, name: &str, now: &str) -> Result<Company, UserError> {
    let name = clean(name, 2, 60, "company name must be 2 to 60 characters")?;
    let tx = conn.transaction()?;
    if company_name_taken(&tx, tenant, &name, None)? {
        return Err(UserError::Conflict("a company with that name already exists"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO companies(id,tenant_id,name,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)", params![id, tenant, name, now])?;
    let co = get_company(&tx, tenant, &id)?.ok_or(UserError::NotFound)?;
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "company.create", "company", Some(&id), None, Some(&snap(&co).to_string()), now)?;
    enqueue(&tx, tenant, "company", &id, "insert", &snap(&co), None, co.revision)?;
    tx.commit()?;
    Ok(co)
}

pub fn update_company(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, id: &str, expected_revision: i64, name: &str, now: &str) -> Result<Company, UserError> {
    let name = clean(name, 2, 60, "company name must be 2 to 60 characters")?;
    let tx = conn.transaction()?;
    let before = get_company(&tx, tenant, id)?;
    check_revision(before.as_ref().map(|c| c.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if company_name_taken(&tx, tenant, &name, Some(id))? {
        return Err(UserError::Conflict("a company with that name already exists"));
    }
    tx.execute("UPDATE companies SET name=?3, revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2", params![tenant, id, name, now])?;
    let after = get_company(&tx, tenant, id)?.ok_or(UserError::NotFound)?;
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "company.update", "company", Some(id), Some(&snap(&before).to_string()), Some(&snap(&after).to_string()), now)?;
    enqueue(&tx, tenant, "company", id, "update", &snap(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

pub fn set_company_active(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, id: &str, expected_revision: i64, active: bool, now: &str) -> Result<Company, UserError> {
    let tx = conn.transaction()?;
    let before = get_company(&tx, tenant, id)?;
    check_revision(before.as_ref().map(|c| c.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if before.is_active == active {
        return Ok(before);
    }
    tx.execute("UPDATE companies SET is_active=?3, revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2", params![tenant, id, active as i64, now])?;
    let after = get_company(&tx, tenant, id)?.ok_or(UserError::NotFound)?;
    let action = if active { "company.activate" } else { "company.deactivate" };
    audit_entity(&tx, tenant, Some(&actor.user_id), device, action, "company", Some(id), Some(&snap(&before).to_string()), Some(&snap(&after).to_string()), now)?;
    enqueue(&tx, tenant, "company", id, if active { "update" } else { "archive" }, &snap(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

fn check_company_for_brand(tx: &Connection, tenant: &str, company: Option<&str>, current: Option<&str>) -> Result<(), UserError> {
    let Some(c) = company else { return Ok(()) };
    let co = get_company(tx, tenant, c)?;
    match co {
        None => Err(UserError::BadInput("unknown company")),
        Some(co) if !co.is_active && Some(c) != current => Err(UserError::BadInput("that company is inactive")),
        Some(_) => Ok(()),
    }
}

fn clean_company_id(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

pub fn create_brand(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, name: &str, company_id: Option<&str>, now: &str) -> Result<Brand, UserError> {
    let name = clean(name, 2, 60, "brand name must be 2 to 60 characters")?;
    let company = clean_company_id(company_id);
    let tx = conn.transaction()?;
    if brand_name_taken(&tx, tenant, &name, None)? {
        return Err(UserError::Conflict("a brand with that name already exists"));
    }
    check_company_for_brand(&tx, tenant, company.as_deref(), None)?;
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO brands(id,tenant_id,name,company_id,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?5)", params![id, tenant, name, company, now])?;
    let b = get_brand(&tx, tenant, &id)?.ok_or(UserError::NotFound)?;
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "brand.create", "brand", Some(&id), None, Some(&snap(&b).to_string()), now)?;
    enqueue(&tx, tenant, "brand", &id, "insert", &snap(&b), None, b.revision)?;
    tx.commit()?;
    Ok(b)
}

pub fn update_brand(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, id: &str, expected_revision: i64, name: &str, company_id: Option<&str>, now: &str) -> Result<Brand, UserError> {
    let name = clean(name, 2, 60, "brand name must be 2 to 60 characters")?;
    let company = clean_company_id(company_id);
    let tx = conn.transaction()?;
    let before = get_brand(&tx, tenant, id)?;
    check_revision(before.as_ref().map(|b| b.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if brand_name_taken(&tx, tenant, &name, Some(id))? {
        return Err(UserError::Conflict("a brand with that name already exists"));
    }
    check_company_for_brand(&tx, tenant, company.as_deref(), before.company_id.as_deref())?;
    tx.execute("UPDATE brands SET name=?3, company_id=?4, revision=revision+1, updated_at=?5 WHERE tenant_id=?1 AND id=?2", params![tenant, id, name, company, now])?;
    let after = get_brand(&tx, tenant, id)?.ok_or(UserError::NotFound)?;
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "brand.update", "brand", Some(id), Some(&snap(&before).to_string()), Some(&snap(&after).to_string()), now)?;
    enqueue(&tx, tenant, "brand", id, "update", &snap(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

pub fn set_brand_active(conn: &mut Connection, tenant: &str, actor: &Principal, device: Option<&str>, id: &str, expected_revision: i64, active: bool, now: &str) -> Result<Brand, UserError> {
    let tx = conn.transaction()?;
    let before = get_brand(&tx, tenant, id)?;
    check_revision(before.as_ref().map(|b| b.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if before.is_active == active {
        return Ok(before);
    }
    tx.execute("UPDATE brands SET is_active=?3, revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2", params![tenant, id, active as i64, now])?;
    let after = get_brand(&tx, tenant, id)?.ok_or(UserError::NotFound)?;
    let action = if active { "brand.activate" } else { "brand.deactivate" };
    audit_entity(&tx, tenant, Some(&actor.user_id), device, action, "brand", Some(id), Some(&snap(&before).to_string()), Some(&snap(&after).to_string()), now)?;
    enqueue(&tx, tenant, "brand", id, if active { "update" } else { "archive" }, &snap(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::master::testutil::*;

    fn mgr() -> Principal {
        actor(&["products.manage"], &[])
    }

    #[test]
    fn companies_are_created_renamed_and_unique_ignoring_case() {
        let mut c = setup();
        let a = create_company(&mut c, "t1", &mgr(), Some("d1"), "  Atlas Honda ", T0).unwrap();
        assert_eq!((a.name.as_str(), a.revision, a.is_active), ("Atlas Honda", 1, true));
        assert!(matches!(create_company(&mut c, "t1", &mgr(), None, "ATLAS HONDA", T0), Err(UserError::Conflict(_))));
        assert!(matches!(create_company(&mut c, "t1", &mgr(), None, "x", T0), Err(UserError::BadInput(_))));
        let b = create_company(&mut c, "t1", &mgr(), None, "Suzuki", T0).unwrap();
        assert!(matches!(update_company(&mut c, "t1", &mgr(), None, &b.id, 1, "atlas honda", T0), Err(UserError::Conflict(_))));
        let r = update_company(&mut c, "t1", &mgr(), None, &b.id, 1, "Pak Suzuki", T0).unwrap();
        assert_eq!((r.name.as_str(), r.revision), ("Pak Suzuki", 2));
        assert!(matches!(update_company(&mut c, "t1", &mgr(), None, &b.id, 1, "Again", T0), Err(UserError::Stale)));
        assert!(matches!(update_company(&mut c, "t1", &mgr(), None, "ghost", 1, "Nobody", T0), Err(UserError::NotFound)));
        let names: Vec<String> = list_companies(&c, "t1").unwrap().into_iter().map(|x| x.name).collect();
        assert_eq!(names, ["Atlas Honda", "Pak Suzuki"]);
    }

    #[test]
    fn brands_may_belong_to_an_active_company_or_to_none() {
        let mut c = setup();
        let co = create_company(&mut c, "t1", &mgr(), None, "Atlas Honda", T0).unwrap();
        let b1 = create_brand(&mut c, "t1", &mgr(), None, "Honda Genuine", Some(&co.id), T0).unwrap();
        assert_eq!(b1.company_id.as_deref(), Some(co.id.as_str()));
        let b2 = create_brand(&mut c, "t1", &mgr(), None, "Local", None, T0).unwrap();
        assert_eq!(b2.company_id, None);
        let blank = create_brand(&mut c, "t1", &mgr(), None, "Blank company", Some("  "), T0).unwrap();
        assert_eq!(blank.company_id, None, "a blank company means none");
        assert!(matches!(create_brand(&mut c, "t1", &mgr(), None, "honda genuine", None, T0), Err(UserError::Conflict(_))));
        assert!(matches!(create_brand(&mut c, "t1", &mgr(), None, "Ghost brand", Some("nope"), T0), Err(UserError::BadInput(_))));
        set_company_active(&mut c, "t1", &mgr(), None, &co.id, 1, false, T0).unwrap();
        assert!(matches!(create_brand(&mut c, "t1", &mgr(), None, "New under inactive", Some(&co.id), T0), Err(UserError::BadInput(_))));
        // an existing brand keeps its (now inactive) company when it is renamed
        let r = update_brand(&mut c, "t1", &mgr(), None, &b1.id, 1, "Honda Genuine Parts", Some(&co.id), T0).unwrap();
        assert_eq!((r.name.as_str(), r.revision, r.company_id.as_deref()), ("Honda Genuine Parts", 2, Some(co.id.as_str())));
        // but it cannot be moved onto an inactive company it was not already under
        let other = create_company(&mut c, "t1", &mgr(), None, "Suzuki", T0).unwrap();
        set_company_active(&mut c, "t1", &mgr(), None, &other.id, 1, false, T0).unwrap();
        assert!(matches!(update_brand(&mut c, "t1", &mgr(), None, &b2.id, 1, "Local", Some(&other.id), T0), Err(UserError::BadInput(_))));
    }

    #[test]
    fn revisions_activation_audit_and_outbox() {
        let mut c = setup();
        let b = create_brand(&mut c, "t1", &mgr(), Some("d1"), "Denso", None, T0).unwrap();
        assert!(matches!(update_brand(&mut c, "t1", &mgr(), None, &b.id, 9, "Denso", None, T0), Err(UserError::Stale)));
        let off = set_brand_active(&mut c, "t1", &mgr(), None, &b.id, 1, false, T0).unwrap();
        assert_eq!((off.is_active, off.revision), (false, 2));
        assert_eq!(set_brand_active(&mut c, "t1", &mgr(), None, &b.id, 2, false, T0).unwrap().revision, 2, "no-op");
        assert!(set_brand_active(&mut c, "t1", &mgr(), None, &b.id, 2, true, T0).unwrap().is_active);
        let ops: Vec<String> = outbox(&c).into_iter().map(|o| o.1).collect();
        assert_eq!(ops, ["insert", "archive", "update"]);
        let acts: Vec<String> = audit(&c).into_iter().map(|a| a.0).collect();
        assert_eq!(acts, ["brand.create", "brand.deactivate", "brand.activate"]);
    }

    #[test]
    fn other_shops_cannot_see_or_use_them() {
        let mut c = setup();
        c.execute("INSERT INTO tenants(id,name) VALUES ('t2','Other')", []).unwrap();
        let mine = create_company(&mut c, "t1", &mgr(), None, "Mine", T0).unwrap();
        create_company(&mut c, "t2", &mgr(), None, "Mine", T0).unwrap(); // same name in another shop is fine
        assert_eq!(list_companies(&c, "t1").unwrap().len(), 1);
        assert!(get_company(&c, "t2", &mine.id).unwrap().is_none());
        assert!(matches!(create_brand(&mut c, "t2", &mgr(), None, "Theirs", Some(&mine.id), T0), Err(UserError::BadInput(_))));
    }
}
