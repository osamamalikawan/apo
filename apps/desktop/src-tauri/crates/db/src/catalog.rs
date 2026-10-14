//! Product categories and products.
//!
//! * SKUs are upper-case (`A-Z 0-9 . _ / -`, up to 40 characters) and unique per shop.
//! * A barcode identifies exactly one product (unique index from migration 0005).
//! * Products and categories are deactivated or renamed, never deleted.
//! * Searching matches name, SKU (contains, ignoring case) or an exact barcode, so a scanner can be
//!   pointed straight at `search`.
//! * Prices are not here: they belong to the sales module's price lists (build step 5).

use crate::master::{check_revision, clean, clean_opt, code, enqueue};
use crate::users::{audit_entity, Principal, UserError};
use crate::DbError;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

// ---------- categories ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub revision: i64,
}

fn cat_from_row(r: &Row) -> rusqlite::Result<Category> {
    Ok(Category { id: r.get(0)?, name: r.get(1)?, revision: r.get(2)? })
}

fn cat_json(c: &Category) -> serde_json::Value {
    serde_json::to_value(c).unwrap_or(serde_json::Value::Null)
}

pub fn list_categories(conn: &Connection, tenant: &str) -> Result<Vec<Category>, DbError> {
    let mut st = conn.prepare("SELECT id,name,revision FROM product_categories WHERE tenant_id=?1 ORDER BY lower(name)")?;
    let rows = st.query_map([tenant], cat_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn get_category(c: &Connection, tenant: &str, id: &str) -> Result<Option<Category>, rusqlite::Error> {
    c.query_row("SELECT id,name,revision FROM product_categories WHERE tenant_id=?1 AND id=?2", params![tenant, id], cat_from_row).optional()
}

fn category_name_taken(c: &Connection, tenant: &str, name: &str, except: Option<&str>) -> Result<bool, rusqlite::Error> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM product_categories WHERE tenant_id=?1 AND lower(name)=lower(?2) AND id<>COALESCE(?3,''))",
        params![tenant, name, except],
        |r| r.get(0),
    )
}

pub fn create_category(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    name: &str,
    now: &str,
) -> Result<Category, UserError> {
    let name = clean(name, 2, 60, "category name must be 2 to 60 characters")?;
    let tx = conn.transaction()?;
    if category_name_taken(&tx, tenant, &name, None)? {
        return Err(UserError::Conflict("a category with that name already exists"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO product_categories(id,tenant_id,name) VALUES (?1,?2,?3)", params![id, tenant, name])?;
    let cat = get_category(&tx, tenant, &id)?.ok_or(UserError::NotFound)?;
    let snap = cat_json(&cat);
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "category.create", "category", Some(&id), None, Some(&snap.to_string()), now)?;
    enqueue(&tx, tenant, "category", &id, "insert", &snap, None, cat.revision)?;
    tx.commit()?;
    Ok(cat)
}

pub fn rename_category(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    id: &str,
    expected_revision: i64,
    name: &str,
    now: &str,
) -> Result<Category, UserError> {
    let name = clean(name, 2, 60, "category name must be 2 to 60 characters")?;
    let tx = conn.transaction()?;
    let before = get_category(&tx, tenant, id)?;
    check_revision(before.as_ref().map(|c| c.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if category_name_taken(&tx, tenant, &name, Some(id))? {
        return Err(UserError::Conflict("a category with that name already exists"));
    }
    tx.execute("UPDATE product_categories SET name=?3, revision=revision+1 WHERE tenant_id=?1 AND id=?2", params![tenant, id, name])?;
    let after = get_category(&tx, tenant, id)?.ok_or(UserError::NotFound)?;
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "category.rename", "category", Some(id), Some(&cat_json(&before).to_string()), Some(&cat_json(&after).to_string()), now)?;
    enqueue(&tx, tenant, "category", id, "update", &cat_json(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

// ---------- products ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Product {
    pub id: String,
    pub sku: String,
    pub name: String,
    pub barcode: Option<String>,
    pub category_id: Option<String>,
    pub unit: String,
    pub is_active: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ProductInput {
    pub sku: String,
    pub name: String,
    pub barcode: Option<String>,
    pub category_id: Option<String>,
    /// Lower-case word such as "pcs", "set", "litre". Blank means "pcs".
    pub unit: String,
}

const PCOLS: &str = "id,sku,name,barcode,category_id,unit,is_active,revision";

fn prod_from_row(r: &Row) -> rusqlite::Result<Product> {
    Ok(Product {
        id: r.get(0)?,
        sku: r.get(1)?,
        name: r.get(2)?,
        barcode: r.get(3)?,
        category_id: r.get(4)?,
        unit: r.get(5)?,
        is_active: r.get::<_, i64>(6)? == 1,
        revision: r.get(7)?,
    })
}

fn prod_json(p: &Product) -> serde_json::Value {
    serde_json::to_value(p).unwrap_or(serde_json::Value::Null)
}

struct Clean {
    sku: String,
    name: String,
    barcode: Option<String>,
    category: Option<String>,
    unit: String,
}

fn validate(i: &ProductInput) -> Result<Clean, UserError> {
    let unit = if i.unit.trim().is_empty() { "pcs".to_string() } else { i.unit.trim().to_string() };
    if !(1..=12).contains(&unit.len()) || !unit.chars().all(|c| c.is_ascii_lowercase()) {
        return Err(UserError::BadInput("unit must be 1 to 12 lower-case letters"));
    }
    let barcode = clean_opt(i.barcode.as_deref(), 32, "barcode must be 4 to 32 letters, digits or dashes")?;
    if let Some(b) = &barcode {
        if b.len() < 4 || !b.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(UserError::BadInput("barcode must be 4 to 32 letters, digits or dashes"));
        }
    }
    Ok(Clean {
        sku: code(&i.sku, 1, 40, "._/-", "SKU must be 1 to 40 characters: A-Z, 0-9, dot, slash, dash, underscore")?,
        name: clean(&i.name, 2, 120, "name must be 2 to 120 characters")?,
        barcode,
        category: clean_opt(i.category_id.as_deref(), 64, "invalid category")?,
        unit,
    })
}

pub fn get_product(conn: &Connection, tenant: &str, id: &str) -> Result<Option<Product>, DbError> {
    Ok(conn
        .query_row(&format!("SELECT {PCOLS} FROM products WHERE tenant_id=?1 AND id=?2"), params![tenant, id], prod_from_row)
        .optional()?)
}

#[derive(Debug, Clone, Default)]
pub struct ProductQuery {
    pub search: Option<String>,
    pub category_id: Option<String>,
    pub include_inactive: bool,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProductPage {
    pub items: Vec<Product>,
    pub total: i64,
}

const FILTER: &str = "tenant_id=?1
    AND (?2 IS NULL OR instr(lower(name),lower(?2))>0 OR instr(lower(sku),lower(?2))>0 OR barcode=?2)
    AND (?3 IS NULL OR category_id=?3)
    AND (?4=1 OR is_active=1)";

/// Newest-name-first is not useful for a parts counter: results are ordered by name, then SKU.
pub fn search_products(conn: &Connection, tenant: &str, q: &ProductQuery) -> Result<ProductPage, DbError> {
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(60).collect::<String>());
    let limit = if q.limit <= 0 { 50 } else { q.limit.min(200) };
    let offset = q.offset.max(0);
    let inc = q.include_inactive as i64;
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM products WHERE {FILTER}"),
        params![tenant, search, q.category_id, inc],
        |r| r.get(0),
    )?;
    let mut st = conn.prepare(&format!("SELECT {PCOLS} FROM products WHERE {FILTER} ORDER BY lower(name), sku LIMIT ?5 OFFSET ?6"))?;
    let rows = st.query_map(params![tenant, search, q.category_id, inc, limit, offset], prod_from_row)?;
    Ok(ProductPage { items: rows.collect::<Result<Vec<_>, _>>()?, total })
}

fn sku_taken(c: &Connection, tenant: &str, sku: &str, except: Option<&str>) -> Result<bool, rusqlite::Error> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM products WHERE tenant_id=?1 AND sku=?2 AND id<>COALESCE(?3,''))",
        params![tenant, sku, except],
        |r| r.get(0),
    )
}

fn barcode_taken(c: &Connection, tenant: &str, barcode: &str, except: Option<&str>) -> Result<bool, rusqlite::Error> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM products WHERE tenant_id=?1 AND barcode=?2 AND id<>COALESCE(?3,''))",
        params![tenant, barcode, except],
        |r| r.get(0),
    )
}

fn check_category(c: &Connection, tenant: &str, category: Option<&str>) -> Result<(), UserError> {
    if let Some(cat) = category {
        if get_category(c, tenant, cat)?.is_none() {
            return Err(UserError::BadInput("unknown category"));
        }
    }
    Ok(())
}

fn check_unique(tx: &Connection, tenant: &str, v: &Clean, except: Option<&str>) -> Result<(), UserError> {
    if sku_taken(tx, tenant, &v.sku, except)? {
        return Err(UserError::Conflict("a product with that SKU already exists"));
    }
    if let Some(b) = &v.barcode {
        if barcode_taken(tx, tenant, b, except)? {
            return Err(UserError::Conflict("that barcode already belongs to another product"));
        }
    }
    Ok(())
}

pub fn create_product(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    input: &ProductInput,
    now: &str,
) -> Result<Product, UserError> {
    let v = validate(input)?;
    let tx = conn.transaction()?;
    check_unique(&tx, tenant, &v, None)?;
    check_category(&tx, tenant, v.category.as_deref())?;
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO products(id,tenant_id,sku,name,barcode,category_id,unit,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![id, tenant, v.sku, v.name, v.barcode, v.category, v.unit, now],
    )?;
    let p = get_product(&tx, tenant, &id).map_err(UserError::from)?.ok_or(UserError::NotFound)?;
    let snap = prod_json(&p);
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "product.create", "product", Some(&id), None, Some(&snap.to_string()), now)?;
    enqueue(&tx, tenant, "product", &id, "insert", &snap, None, p.revision)?;
    tx.commit()?;
    Ok(p)
}

pub fn update_product(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    id: &str,
    expected_revision: i64,
    input: &ProductInput,
    now: &str,
) -> Result<Product, UserError> {
    let v = validate(input)?;
    let tx = conn.transaction()?;
    let before = get_product(&tx, tenant, id).map_err(UserError::from)?;
    check_revision(before.as_ref().map(|p| p.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    check_unique(&tx, tenant, &v, Some(id))?;
    check_category(&tx, tenant, v.category.as_deref())?;
    tx.execute(
        "UPDATE products SET sku=?3, name=?4, barcode=?5, category_id=?6, unit=?7, revision=revision+1, updated_at=?8
         WHERE tenant_id=?1 AND id=?2",
        params![tenant, id, v.sku, v.name, v.barcode, v.category, v.unit, now],
    )?;
    let after = get_product(&tx, tenant, id).map_err(UserError::from)?.ok_or(UserError::NotFound)?;
    audit_entity(&tx, tenant, Some(&actor.user_id), device, "product.update", "product", Some(id), Some(&prod_json(&before).to_string()), Some(&prod_json(&after).to_string()), now)?;
    enqueue(&tx, tenant, "product", id, "update", &prod_json(&after), Some(before.revision), after.revision)?;
    tx.commit()?;
    Ok(after)
}

/// Stop or resume selling a product. Stock history stays; an inactive product simply drops out of
/// searches (unless `include_inactive`) and later out of new sales.
pub fn set_product_active(
    conn: &mut Connection,
    tenant: &str,
    actor: &Principal,
    device: Option<&str>,
    id: &str,
    expected_revision: i64,
    active: bool,
    now: &str,
) -> Result<Product, UserError> {
    let tx = conn.transaction()?;
    let before = get_product(&tx, tenant, id).map_err(UserError::from)?;
    check_revision(before.as_ref().map(|p| p.revision), expected_revision)?;
    let before = before.ok_or(UserError::NotFound)?;
    if before.is_active == active {
        return Ok(before);
    }
    tx.execute(
        "UPDATE products SET is_active=?3, revision=revision+1, updated_at=?4 WHERE tenant_id=?1 AND id=?2",
        params![tenant, id, active as i64, now],
    )?;
    let after = get_product(&tx, tenant, id).map_err(UserError::from)?.ok_or(UserError::NotFound)?;
    let action = if active { "product.activate" } else { "product.deactivate" };
    audit_entity(&tx, tenant, Some(&actor.user_id), device, action, "product", Some(id), Some(&prod_json(&before).to_string()), Some(&prod_json(&after).to_string()), now)?;
    enqueue(&tx, tenant, "product", id, if active { "update" } else { "archive" }, &prod_json(&after), Some(before.revision), after.revision)?;
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

    fn pin(sku: &str, name: &str) -> ProductInput {
        ProductInput { sku: sku.into(), name: name.into(), ..Default::default() }
    }

    fn add(c: &mut Connection, sku: &str, name: &str) -> Product {
        create_product(c, "t1", &mgr(), Some("d1"), &pin(sku, name), T0).unwrap()
    }

    #[test]
    fn categories_are_created_renamed_and_stay_unique() {
        let mut c = setup();
        let a = create_category(&mut c, "t1", &mgr(), None, "  Brake pads ", T0).unwrap();
        assert_eq!((a.name.as_str(), a.revision), ("Brake pads", 1));
        assert!(matches!(create_category(&mut c, "t1", &mgr(), None, "BRAKE PADS", T0), Err(UserError::Conflict(_))));
        assert!(matches!(create_category(&mut c, "t1", &mgr(), None, "x", T0), Err(UserError::BadInput(_))));
        let b = create_category(&mut c, "t1", &mgr(), None, "Filters", T0).unwrap();
        assert!(matches!(rename_category(&mut c, "t1", &mgr(), None, &b.id, 1, "brake pads", T0), Err(UserError::Conflict(_))));
        let r = rename_category(&mut c, "t1", &mgr(), None, &b.id, 1, "Oil filters", T0).unwrap();
        assert_eq!((r.name.as_str(), r.revision), ("Oil filters", 2));
        assert!(matches!(rename_category(&mut c, "t1", &mgr(), None, &b.id, 1, "Again", T0), Err(UserError::Stale)));
        assert!(matches!(rename_category(&mut c, "t1", &mgr(), None, "ghost", 1, "Whatever", T0), Err(UserError::NotFound)));
        let names: Vec<String> = list_categories(&c, "t1").unwrap().into_iter().map(|x| x.name).collect();
        assert_eq!(names, ["Brake pads", "Oil filters"]);
    }

    #[test]
    fn creating_a_product_normalizes_and_records_everything() {
        let mut c = setup();
        let cat = create_category(&mut c, "t1", &mgr(), None, "Brakes", T0).unwrap();
        let p = create_product(
            &mut c, "t1", &mgr(), Some("d1"),
            &ProductInput { sku: " bp-1234/a ".into(), name: " Brake pad front ".into(), barcode: Some(" 8901234567890 ".into()), category_id: Some(cat.id.clone()), unit: "set".into() },
            T0,
        )
        .unwrap();
        assert_eq!((p.sku.as_str(), p.name.as_str(), p.barcode.as_deref(), p.unit.as_str(), p.revision, p.is_active), ("BP-1234/A", "Brake pad front", Some("8901234567890"), "set", 1, true));
        assert_eq!(p.category_id.as_deref(), Some(cat.id.as_str()));
        assert_eq!(add(&mut c, "X1", "Plain part").unit, "pcs", "blank unit defaults to pcs");
        let a = audit(&c);
        assert!(a.contains(&("product.create".to_string(), "product".to_string())));
        assert!(outbox(&c).iter().any(|o| o.0 == "product" && o.1 == "insert" && o.3 == format!("product:{}:r1", p.id)));
    }

    #[test]
    fn invalid_and_duplicate_products_are_refused_without_side_effects() {
        let mut c = setup();
        let first = create_product(&mut c, "t1", &mgr(), None, &ProductInput { barcode: Some("111122223333".into()), ..pin("SKU-1", "First part") }, T0).unwrap();
        let mk = |c: &mut Connection, i: ProductInput| create_product(c, "t1", &mgr(), None, &i, T0);
        assert!(matches!(mk(&mut c, pin("sku-1", "Dup sku")), Err(UserError::Conflict(_))), "SKU is case-insensitive");
        assert!(matches!(mk(&mut c, ProductInput { barcode: Some("111122223333".into()), ..pin("SKU-2", "Dup barcode") }), Err(UserError::Conflict(_))));
        for bad in [pin("", "Name ok"), pin("has space", "Name ok"), pin("SKU-9", "x"), pin(&"A".repeat(41), "Name ok")] {
            assert!(matches!(mk(&mut c, bad), Err(UserError::BadInput(_))));
        }
        for b in ["123", "has space!", &"1".repeat(33)] {
            assert!(matches!(mk(&mut c, ProductInput { barcode: Some(b.into()), ..pin("SKU-3", "Name ok") }), Err(UserError::BadInput(_))), "{b}");
        }
        assert!(matches!(mk(&mut c, ProductInput { unit: "Pieces!".into(), ..pin("SKU-3", "Name ok") }), Err(UserError::BadInput(_))));
        assert!(matches!(mk(&mut c, ProductInput { category_id: Some("nope".into()), ..pin("SKU-3", "Name ok") }), Err(UserError::BadInput(_))));
        // blank barcodes are "no barcode", so several products may have none
        assert!(mk(&mut c, ProductInput { barcode: Some("  ".into()), ..pin("SKU-4", "No barcode one") }).is_ok());
        assert!(mk(&mut c, ProductInput { barcode: None, ..pin("SKU-5", "No barcode two") }).is_ok());
        let n: i64 = c.query_row("SELECT COUNT(*) FROM products", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(get_product(&c, "t1", &first.id).unwrap().unwrap().revision, 1);
    }

    #[test]
    fn updates_need_the_current_revision_and_keep_uniqueness() {
        let mut c = setup();
        let a = add(&mut c, "A-1", "Part A");
        let b = add(&mut c, "B-1", "Part B");
        let up = update_product(&mut c, "t1", &mgr(), None, &a.id, 1, &ProductInput { barcode: Some("55556666".into()), ..pin("A-1", "Part A renamed") }, T0).unwrap();
        assert_eq!((up.revision, up.name.as_str(), up.barcode.as_deref()), (2, "Part A renamed", Some("55556666")));
        assert!(matches!(update_product(&mut c, "t1", &mgr(), None, &a.id, 1, &pin("A-1", "Stale edit"), T0), Err(UserError::Stale)));
        assert!(matches!(update_product(&mut c, "t1", &mgr(), None, &b.id, 1, &pin("a-1", "Takes A's sku"), T0), Err(UserError::Conflict(_))));
        assert!(matches!(update_product(&mut c, "t1", &mgr(), None, &b.id, 1, &ProductInput { barcode: Some("55556666".into()), ..pin("B-1", "Part B") }, T0), Err(UserError::Conflict(_))));
        assert!(matches!(update_product(&mut c, "t1", &mgr(), None, "ghost", 1, &pin("Z-1", "Ghost part"), T0), Err(UserError::NotFound)));
        assert_eq!(get_product(&c, "t1", &b.id).unwrap().unwrap().revision, 1, "refused edits changed nothing");
        let ob: Vec<_> = outbox(&c).into_iter().filter(|o| o.3.contains(&a.id)).collect();
        assert_eq!((ob[1].1.as_str(), ob[1].2), ("update", Some(1)));
    }

    #[test]
    fn deactivating_hides_from_search_but_keeps_the_record() {
        let mut c = setup();
        let p = add(&mut c, "OLD-1", "Discontinued part");
        add(&mut c, "NEW-1", "Current part");
        let off = set_product_active(&mut c, "t1", &mgr(), None, &p.id, 1, false, T0).unwrap();
        assert_eq!((off.is_active, off.revision), (false, 2));
        let q = |inc: bool| ProductQuery { include_inactive: inc, ..Default::default() };
        assert_eq!(search_products(&c, "t1", &q(false)).unwrap().total, 1);
        assert_eq!(search_products(&c, "t1", &q(true)).unwrap().total, 2);
        assert!(get_product(&c, "t1", &p.id).unwrap().is_some());
        let again = set_product_active(&mut c, "t1", &mgr(), None, &p.id, 2, false, T0).unwrap();
        assert_eq!(again.revision, 2, "no-op");
        assert!(set_product_active(&mut c, "t1", &mgr(), None, &p.id, 2, true, T0).unwrap().is_active);
        assert!(matches!(set_product_active(&mut c, "t1", &mgr(), None, &p.id, 1, false, T0), Err(UserError::Stale)));
        let ops: Vec<String> = outbox(&c).into_iter().filter(|o| o.3.contains(&p.id)).map(|o| o.1).collect();
        assert_eq!(ops, ["insert", "archive", "update"]);
    }

    #[test]
    fn search_matches_name_sku_and_exact_barcode_and_pages() {
        let mut c = setup();
        let cat = create_category(&mut c, "t1", &mgr(), None, "Brakes", T0).unwrap();
        create_product(&mut c, "t1", &mgr(), None, &ProductInput { barcode: Some("8901234567890".into()), category_id: Some(cat.id.clone()), ..pin("BP-100", "Brake pad front") }, T0).unwrap();
        create_product(&mut c, "t1", &mgr(), None, &pin("BP-200", "Brake pad rear"), T0).unwrap();
        create_product(&mut c, "t1", &mgr(), None, &pin("OF-1", "Oil filter"), T0).unwrap();
        let s = |text: &str| search_products(&c, "t1", &ProductQuery { search: Some(text.into()), ..Default::default() }).unwrap();
        assert_eq!(s("BRAKE").total, 2, "name, ignoring case");
        assert_eq!(s("bp-2").items[0].sku, "BP-200", "sku fragment");
        assert_eq!(s("8901234567890").items.len(), 1, "exact barcode");
        assert_eq!(s("890123456789").items.len(), 0, "barcode must match exactly");
        assert_eq!(s("   ").total, 3, "blank search is no filter");
        assert_eq!(s("100%").total, 0, "wildcard characters are plain text");
        assert_eq!(search_products(&c, "t1", &ProductQuery { category_id: Some(cat.id.clone()), ..Default::default() }).unwrap().total, 1);
        let page = search_products(&c, "t1", &ProductQuery { limit: 2, offset: 0, ..Default::default() }).unwrap();
        assert_eq!((page.items.len() as i64, page.total), (2, 3));
        let names: Vec<_> = page.items.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Brake pad front", "Brake pad rear"], "ordered by name");
        assert_eq!(search_products(&c, "t1", &ProductQuery { limit: 2, offset: 2, ..Default::default() }).unwrap().items.len(), 1);
        assert_eq!(search_products(&c, "t1", &ProductQuery { limit: 9999, ..Default::default() }).unwrap().items.len(), 3, "limit is capped, not rejected");
    }

    #[test]
    fn products_are_invisible_to_other_tenants() {
        let mut c = setup();
        c.execute("INSERT INTO tenants(id,name) VALUES ('t2','Other')", []).unwrap();
        add(&mut c, "SKU-1", "Mine");
        create_product(&mut c, "t2", &mgr(), None, &pin("SKU-1", "Theirs"), T0).unwrap(); // same SKU is fine across shops
        assert_eq!(search_products(&c, "t1", &ProductQuery::default()).unwrap().total, 1);
        assert_eq!(search_products(&c, "t2", &ProductQuery::default()).unwrap().items[0].name, "Theirs");
    }
}
