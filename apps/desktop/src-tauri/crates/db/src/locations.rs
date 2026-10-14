//! Stock locations (shops, godowns, vehicles...).
//!
//! * Codes are upper-case (`A-Z 0-9 _ -`, 2 to 12 characters) and unique per shop.
//! * Locations can be nested (`parent_location_id`) but never inside themselves.
//! * Locations are deactivated, never deleted. A location that still holds stock, or has active
//!   sub-locations, cannot be deactivated.
//! * Creating a location needs `locations.all`: a user limited to some locations must not be able to
//!   mint one they cannot use. The plan limit (`max_locations`) is enforced by the API layer.
//! * Who may *see* a location is decided by `list_locations` from the caller's access context.

use crate::master::{check_revision, clean, clean_opt, code, enqueue};
use crate::users::{audit_entity, has_permission, AccessContext, Principal, UserError};
use crate::DbError;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub code: String,
    pub location_type: String,
    pub description: Option<String>,
    pub parent_location_id: Option<String>,
    pub responsible_user_id: Option<String>,
    pub is_active: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Default)]
pub struct LocationInput {
    pub name: String,
    pub code: String,
    /// Lower-case word such as "storage", "shop" or "vehicle". Blank means "storage".
    pub location_type: String,
    pub description: Option<String>,
    pub parent_location_id: Option<String>,
    pub responsible_user_id: Option<String>,
}

const COLS: &str = "id,name,code,location_type,description,parent_location_id,responsible_user_id,is_active,revision";

fn from_row(r: &Row) -> rusqlite::Result<Location> {
    Ok(Location {
        id: r.get(0)?,
        name: r.get(1)?,
        code: r.get(2)?,
        location_type: r.get(3)?,
        description: r.get(4)?,
        parent_location_id: r.get(5)?,
        responsible_user_id: r.get(6)?,
        is_active: r.get::<_, i64>(7)? == 1,
        revision: r.get(8)?,
    })
}

struct Clean {
    name: String,
    code: String,
    ltype: String,
    description: Option<String>,
}

fn validate(i: &LocationInput) -> Result<Clean, UserError> {
    let ltype = if i.location_type.trim().is_empty() { "storage".to_string() } else { i.location_type.trim().to_string() };
    if !(2..=20).contains(&ltype.len()) || !ltype.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
        return Err(UserError::BadInput("location type must be 2 to 20 lower-case letters or underscores"));
    }
    Ok(Clean {
        name: clean(&i.name, 2, 80, "name must be 2 to 80 characters")?,
        code: code(&i.code, 2, 12, "_-", "code must be 2 to 12 characters: A-Z, 0-9, dash, underscore")?,
        ltype,
        description: clean_opt(i.description.as_deref(), 300, "description is too long")?,
    })
}

pub fn get_location(conn: &Connection, tenant: &str, id: &str) -> Result<Option<Location>, DbError> {
    Ok(conn
        .query_row(&format!("SELECT {COLS} FROM stock_locations WHERE tenant_id=?1 AND id=?2"), params![tenant, id], from_row)
        .optional()?)
}

pub fn count_active(conn: &Connection, tenant: &str) -> Result<i64, DbError> {
    Ok(conn.query_row("SELECT COUNT(*) FROM stock_locations WHERE tenant_id=?1 AND is_active=1", [tenant], |r| r.get(0))?)
}

/// Locations the caller may see: all of them with `locations.all` (or `*`), otherwise only the
/// ones assigned to them. Inactive locations are included; they carry `is_active = false`.
pub fn list_locations(conn: &Connection, tenant: &str, access: &AccessContext) -> Result<Vec<Location>, DbError> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM stock_locations WHERE tenant_id=?1 ORDER BY name, code"))?;
    let rows = st.query_map([tenant], from_row)?;
    let mut out = Vec::new();
    for row in rows {
        let l = row?;
        if access.all_locations || access.location_ids.iter().any(|x| *x == l.id) {
            out.push(l);
        }
    }
    Ok(out)
}

/// The parent must exist and be active, and walking up from it must never reach `self_id`.
fn check_parent(tx: &Connection, tenant: &str, parent: Option<&str>, self_id: Option<&str>) -> Result<(), UserError> {
    let Some(p) = parent else { return Ok(()) };
    let active: Option<i64> = tx
        .query_row("SELECT is_active FROM stock_locations WHERE tenant_id=?1 AND id=?2", params![tenant, p], |r| r.get(0))
        .optional()?;
    if active != Some(1) {
        return Err(UserError::BadInput("unknown or inactive parent location"));
    }
    let mut cur = Some(p.to_string());
    for _ in 0..100 {
        let Some(c) = cur else { return Ok(()) };
        if Some(c.as_str()) == self_id {
            return Err(UserError::BadInput("a location cannot be inside itself"));
        }
        cur = tx
            .query_row("SELECT parent_location_id FROM stock_locations WHERE tenant_id=?1 AND id=?2", params![tenant, c], |r| r.get(0))
            .optional()?
            .flatten();
    }
    Err(UserError::BadInput("location nesting is too deep"))
}

fn check_responsible(tx: &Connection, tenant: &str, user: Option<&str>) -> Result<(), UserError> {
    let Some(u) = user else { return Ok(()) };
    let active: Option<i64> = tx
        .query_row("SELECT is_active FROM users WHERE tenant_id=?1 AND id=?2", params![tenant, u], |r| r.get(0))
        .optional()?;
    if active != Some(1) {
        return Err(UserError::BadInput("unknown or inactive responsible user"));
    }
    Ok(())
}

fn code_taken(tx: &Connection, tenant: &str, code: &str, except: Option<&str>) -> Result<bool, rusqlite::Error> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM stock_locations WHERE tenant_id=?1 AND code=?2 AND id<>COALESCE(?3,''))",
        params![tenant, code, except],
        |r| r.get(0),
    )
}

fn snapshot(l: &Location) -> serde_json::Value {
    serde_json::to_value(l).unwrap_or(serde_json::Value::Null)
}

pub fn create_location(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    input: &LocationInput,
    now: &str,
) -> Result<Location, UserError> {
    if !has_permission(&actor.permissions, "locations.all") {
        return Err(UserError::Forbidden("needs_all_locations"));
    }
    let v = validate(input)?;
    let parent = clean_opt(input.parent_location_id.as_deref(), 64, "invalid parent location")?;
    let responsible = clean_opt(input.responsible_user_id.as_deref(), 64, "invalid responsible user")?;
    let tx = conn.transaction()?;
    if code_taken(&tx, tenant, &v.code, None)? {
        return Err(UserError::Conflict("a location with that code already exists"));
    }
    check_parent(&tx, tenant, parent.as_deref(), None)?;
    check_responsible(&tx, tenant, responsible.as_deref())?;
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO stock_locations(id,tenant_id,name,code,location_type,description,parent_location_id,responsible_user_id,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",
        params![id, tenant, v.name, v.code, v.ltype, v.description, parent, responsible, now],
    )?;
    let loc = get_location(&tx, tenant, &id).map_err(db_err)?.ok_or(UserError::NotFound)?;
    let snap = snapshot(&loc);
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "location.create", "location", Some(&id), None, Some(&snap.to_string()), now)?;
    enqueue(&tx, tenant, "location", &id, "insert", &snap, None, loc.revision)?;
    tx.commit()?;
    Ok(loc)
}

fn db_err(e: DbError) -> UserError {
    UserError::from(e)
}

/// Replace a location's details. `expected_revision` must match what the caller last read.
pub fn update_location(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    id: &str,
    expected_revision: i64,
    input: &LocationInput,
    now: &str,
) -> Result<Location, UserError> {
    let v = validate(input)?;
    let parent = clean_opt(input.parent_location_id.as_deref(), 64, "invalid parent location")?;
    let responsible = clean_opt(input.responsible_user_id.as_deref(), 64, "invalid responsible user")?;
    let tx = conn.transaction()?;
    let before = get_location(&tx, tenant, id).map_err(db_err)?;
    check_revision(before.as_ref().map(|l| l.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if code_taken(&tx, tenant, &v.code, Some(id))? {
        return Err(UserError::Conflict("a location with that code already exists"));
    }
    if parent != before.parent_location_id {
        check_parent(&tx, tenant, parent.as_deref(), Some(id))?;
    }
    if responsible != before.responsible_user_id {
        check_responsible(&tx, tenant, responsible.as_deref())?;
    }
    tx.execute(
        "UPDATE stock_locations SET name=?3, code=?4, location_type=?5, description=?6, parent_location_id=?7,
                responsible_user_id=?8, revision=revision+1, updated_at=?9 WHERE tenant_id=?1 AND id=?2",
        params![tenant, id, v.name, v.code, v.ltype, v.description, parent, responsible, now],
    )?;
    let after = get_location(&tx, tenant, id).map_err(db_err)?.ok_or(UserError::NotFound)?;
    audit_entity(
        &tx, tenant, Some(&actor.user_id), device, "location.update", "location", Some(id),
        Some(&snapshot(&before).to_string()), Some(&snapshot(&after).to_string()), now,
    )?;
    enqueue(&tx, tenant, "location", id, "update", &snapshot(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

pub fn set_location_active(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    id: &str,
    expected_revision: i64,
    active: bool,
    now: &str,
) -> Result<Location, UserError> {
    let tx = conn.transaction()?;
    let before = get_location(&tx, tenant, id).map_err(db_err)?;
    check_revision(before.as_ref().map(|l| l.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if before.is_active == active {
        return Ok(before); // already in that state: nothing to do, nothing to log
    }
    if active {
        if let Some(p) = &before.parent_location_id {
            let parent_active: i64 =
                tx.query_row("SELECT is_active FROM stock_locations WHERE tenant_id=?1 AND id=?2", params![tenant, p], |r| r.get(0))?;
            if parent_active != 1 {
                return Err(UserError::Conflict("the parent location is inactive"));
            }
        }
    } else {
        let has_stock: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM inventory_balances WHERE tenant_id=?1 AND location_id=?2
                           AND (on_hand<>0 OR damaged<>0 OR in_transit<>0))",
            params![tenant, id],
            |r| r.get(0),
        )?;
        if has_stock {
            return Err(UserError::Conflict("the location still holds stock"));
        }
        let has_children: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM stock_locations WHERE tenant_id=?1 AND parent_location_id=?2 AND is_active=1)",
            params![tenant, id],
            |r| r.get(0),
        )?;
        if has_children {
            return Err(UserError::Conflict("the location has active sub-locations"));
        }
    }
    tx.execute(
        "UPDATE stock_locations SET is_active=?3, revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2",
        params![tenant, id, active as i64, now],
    )?;
    let after = get_location(&tx, tenant, id).map_err(db_err)?.ok_or(UserError::NotFound)?;
    let action = if active { "location.activate" } else { "location.deactivate" };
    audit_entity(
        &tx, tenant, Some(&actor.user_id), device, action, "location", Some(id),
        Some(&snapshot(&before).to_string()), Some(&snapshot(&after).to_string()), now,
    )?;
    enqueue(&tx, tenant, "location", id, if active { "update" } else { "archive" }, &snapshot(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::master::testutil::*;

    fn input(name: &str, code: &str) -> LocationInput {
        LocationInput { name: name.into(), code: code.into(), ..Default::default() }
    }

    fn admin() -> Principal {
        actor(&["locations.manage", "locations.all"], &[])
    }

    fn make(c: &mut Connection, name: &str, code: &str) -> Location {
        create_location(c, "t1", &admin(), Some("d1"), &input(name, code), T0).unwrap()
    }

    #[test]
    fn creating_a_location_normalizes_and_records_everything() {
        let mut c = setup();
        let l = create_location(
            &mut c, "t1", &admin(), Some("d1"),
            &LocationInput { name: "  Main Shop ".into(), code: " main-1 ".into(), location_type: "shop".into(), description: Some("Front".into()), ..Default::default() },
            T0,
        )
        .unwrap();
        assert_eq!((l.name.as_str(), l.code.as_str(), l.location_type.as_str(), l.revision, l.is_active), ("Main Shop", "MAIN-1", "shop", 1, true));
        assert_eq!(make(&mut c, "Godown", "G1").location_type, "storage", "blank type defaults to storage");
        assert_eq!(audit(&c)[0], ("location.create".to_string(), "location".to_string()));
        let ob = outbox(&c);
        assert_eq!((ob[0].0.as_str(), ob[0].1.as_str(), ob[0].2), ("location", "insert", None));
        assert_eq!(ob[0].3, format!("location:{}:r1", l.id));
    }

    #[test]
    fn validation_and_duplicates_are_refused_without_side_effects() {
        let mut c = setup();
        make(&mut c, "Main", "MAIN");
        let mk = |c: &mut Connection, i: LocationInput| create_location(c, "t1", &admin(), None, &i, T0);
        assert!(matches!(mk(&mut c, input("Other", "main")), Err(UserError::Conflict(_))), "code is case-insensitive");
        for (n, cd) in [("x", "OK"), ("Fine", "A"), ("Fine", "has space"), ("Fine", "TOO-LONG-CODE-1"), ("Fine", "bad!")] {
            assert!(matches!(mk(&mut c, input(n, cd)), Err(UserError::BadInput(_))), "{n}/{cd}");
        }
        let bad_type = LocationInput { location_type: "Shop!".into(), ..input("Fine", "OK") };
        assert!(matches!(mk(&mut c, bad_type), Err(UserError::BadInput(_))));
        let bad_parent = LocationInput { parent_location_id: Some("nope".into()), ..input("Fine", "OK") };
        assert!(matches!(mk(&mut c, bad_parent), Err(UserError::BadInput(_))));
        let bad_user = LocationInput { responsible_user_id: Some("ghost".into()), ..input("Fine", "OK") };
        assert!(matches!(mk(&mut c, bad_user), Err(UserError::BadInput(_))));
        let n: i64 = c.query_row("SELECT COUNT(*) FROM stock_locations", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(audit(&c).len(), 1);
    }

    #[test]
    fn only_users_with_all_locations_can_create_one() {
        let mut c = setup();
        let limited = actor(&["locations.manage"], &["x"]);
        assert!(matches!(create_location(&mut c, "t1", &limited, None, &input("Main", "MAIN"), T0), Err(UserError::Forbidden(_))));
        let owner = actor(&["*"], &[]);
        assert!(create_location(&mut c, "t1", &owner, None, &input("Main", "MAIN"), T0).is_ok());
    }

    #[test]
    fn updates_need_the_current_revision() {
        let mut c = setup();
        let l = make(&mut c, "Main", "MAIN");
        let a = admin();
        let u = update_location(&mut c, "t1", &a, None, &l.id, 1, &LocationInput { description: Some("Front".into()), ..input("Main shop", "MAIN") }, T0).unwrap();
        assert_eq!((u.revision, u.name.as_str(), u.description.as_deref()), (2, "Main shop", Some("Front")));
        // someone else still holding revision 1 is refused and changes nothing
        let stale = update_location(&mut c, "t1", &a, None, &l.id, 1, &input("Sneaky", "MAIN"), T0);
        assert!(matches!(stale, Err(UserError::Stale)));
        assert_eq!(get_location(&c, "t1", &l.id).unwrap().unwrap().name, "Main shop");
        assert!(matches!(update_location(&mut c, "t1", &a, None, "ghost", 1, &input("X Y", "XY"), T0), Err(UserError::NotFound)));
        let ob = outbox(&c);
        assert_eq!((ob[1].1.as_str(), ob[1].2, ob[1].3.as_str()), ("update", Some(1), format!("location:{}:r2", l.id).as_str()));
    }

    #[test]
    fn code_changes_stay_unique() {
        let mut c = setup();
        let a = make(&mut c, "A place", "AA");
        make(&mut c, "B place", "BB");
        let r = update_location(&mut c, "t1", &admin(), None, &a.id, 1, &input("A place", "bb"), T0);
        assert!(matches!(r, Err(UserError::Conflict(_))));
        assert!(update_location(&mut c, "t1", &admin(), None, &a.id, 1, &input("A place", "aa"), T0).is_ok(), "own code is fine");
    }

    #[test]
    fn nesting_is_allowed_but_never_circular() {
        let mut c = setup();
        let shop = make(&mut c, "Shop", "SHOP");
        let child = create_location(&mut c, "t1", &admin(), None, &LocationInput { parent_location_id: Some(shop.id.clone()), ..input("Back room", "BACK") }, T0).unwrap();
        assert_eq!(child.parent_location_id.as_deref(), Some(shop.id.as_str()));
        let grand = create_location(&mut c, "t1", &admin(), None, &LocationInput { parent_location_id: Some(child.id.clone()), ..input("Shelf", "SHELF") }, T0).unwrap();
        let mv = |c: &mut Connection, l: &Location, p: &str| {
            update_location(c, "t1", &admin(), None, &l.id, l.revision, &LocationInput { parent_location_id: Some(p.into()), ..input(&l.name, &l.code) }, T0)
        };
        assert!(matches!(mv(&mut c, &shop, &shop.id), Err(UserError::BadInput(_))), "inside itself");
        assert!(matches!(mv(&mut c, &shop, &grand.id), Err(UserError::BadInput(_))), "inside its own descendant");
        assert!(matches!(mv(&mut c, &shop, &child.id), Err(UserError::BadInput(_))));
        let other = make(&mut c, "Godown", "GD");
        assert!(mv(&mut c, &grand, &other.id).is_ok());
    }

    #[test]
    fn a_responsible_user_must_be_an_active_user() {
        let mut c = setup();
        let l = create_location(&mut c, "t1", &admin(), None, &LocationInput { responsible_user_id: Some("u1".into()), ..input("Main", "MAIN") }, T0).unwrap();
        assert_eq!(l.responsible_user_id.as_deref(), Some("u1"));
        c.execute("UPDATE users SET is_active=0 WHERE id='u1'", []).unwrap();
        let again = update_location(&mut c, "t1", &admin(), None, &l.id, 1, &LocationInput { responsible_user_id: Some("u1".into()), ..input("Main", "MAIN") }, T0);
        assert!(again.is_ok(), "unchanged responsible user is not re-validated");
        let bad = update_location(&mut c, "t1", &admin(), None, &l.id, 2, &LocationInput { responsible_user_id: Some("ghost".into()), ..input("Main", "MAIN") }, T0);
        assert!(matches!(bad, Err(UserError::BadInput(_))));
    }

    #[test]
    fn deactivation_is_blocked_by_stock_and_active_children_and_is_reversible() {
        let mut c = setup();
        let shop = make(&mut c, "Shop", "SHOP");
        let child = create_location(&mut c, "t1", &admin(), None, &LocationInput { parent_location_id: Some(shop.id.clone()), ..input("Back", "BACK") }, T0).unwrap();
        let off = |c: &mut Connection, l: &Location, rev: i64| set_location_active(c, "t1", &admin(), None, &l.id, rev, false, T0);
        assert!(matches!(off(&mut c, &shop, 1), Err(UserError::Conflict(_))), "active child");

        c.execute_batch(&format!(
            "INSERT INTO products(id,tenant_id,sku,name) VALUES ('p1','t1','SKU1','Part');
             INSERT INTO inventory_balances(tenant_id,location_id,product_id,on_hand) VALUES ('t1','{}','p1',5);",
            child.id
        ))
        .unwrap();
        assert!(matches!(off(&mut c, &child, 1), Err(UserError::Conflict(_))), "holds stock");
        c.execute("UPDATE inventory_balances SET on_hand=0", []).unwrap();
        let child_off = off(&mut c, &child, 1).unwrap();
        assert_eq!((child_off.is_active, child_off.revision), (false, 2));
        let shop_off = off(&mut c, &shop, 1).unwrap();
        assert!(!shop_off.is_active);
        // cannot bring the child back under an inactive parent; can once the parent is back
        let on = |c: &mut Connection, l: &Location, rev: i64| set_location_active(c, "t1", &admin(), None, &l.id, rev, true, T0);
        assert!(matches!(on(&mut c, &child_off, 2), Err(UserError::Conflict(_))));
        let shop_on = on(&mut c, &shop_off, 2).unwrap();
        assert!(shop_on.is_active);
        assert!(on(&mut c, &child_off, 2).unwrap().is_active);
        // asking for the state it is already in changes nothing
        let rev_before = get_location(&c, "t1", &shop.id).unwrap().unwrap().revision;
        let same = on(&mut c, &shop_on, rev_before).unwrap();
        assert_eq!(same.revision, rev_before);
        assert_eq!(count_active(&c, "t1").unwrap(), 2);
        let ops: Vec<String> = outbox(&c).into_iter().filter(|o| o.3.contains(&shop.id)).map(|o| o.1).collect();
        assert_eq!(ops, ["insert", "archive", "update"]);
        assert!(matches!(off(&mut c, &shop, 1), Err(UserError::Stale)));
    }

    #[test]
    fn locations_cannot_be_deleted_at_all() {
        let mut c = setup();
        let l = make(&mut c, "Main", "MAIN");
        assert!(c.execute("DELETE FROM stock_locations WHERE id=?1", [&l.id]).is_err());
    }

    #[test]
    fn listing_shows_only_what_the_caller_may_see() {
        let mut c = setup();
        let a = make(&mut c, "Alpha", "AAA");
        let b = make(&mut c, "Beta", "BBB");
        make(&mut c, "Gamma", "CCC");
        let names = |ctx: AccessContext| list_locations(&c, "t1", &ctx).unwrap().into_iter().map(|l| l.name).collect::<Vec<_>>();
        let ctx = |perms: &[&str], locs: &[&str]| actor(perms, locs).access();
        assert_eq!(names(ctx(&["locations.all"], &[])), ["Alpha", "Beta", "Gamma"]);
        assert_eq!(names(ctx(&["*"], &[])), ["Alpha", "Beta", "Gamma"]);
        assert_eq!(names(ctx(&["sales.view"], &[&a.id, &b.id])), ["Alpha", "Beta"]);
        assert!(names(ctx(&["sales.view"], &[])).is_empty(), "no assignment means no locations");
        set_location_active(&mut c, "t1", &admin(), None, &a.id, 1, false, T0).unwrap();
        let all = list_locations(&c, "t1", &ctx(&["*"], &[])).unwrap();
        assert!(!all.iter().find(|l| l.id == a.id).unwrap().is_active, "inactive ones are still listed, flagged");
    }
}
