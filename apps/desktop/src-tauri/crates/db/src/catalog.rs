//! Product categories and products.
//!
//! * A product's code name (`sku`) is upper-case (`A-Z 0-9 . _ / -`, up to 40 characters) and unique per shop.
//! * Each product has an English name, an optional Urdu name and model, an optional company and brand
//!   (a brand that belongs to a company fills the company in), and an optional scheme (buy N, get M free).
//! * A barcode identifies exactly one product (unique index from migration 0005).
//! * Products and categories are deactivated or renamed, never deleted.
//! * Searching matches name, SKU (contains, ignoring case) or an exact barcode, so a scanner can be
//!   pointed straight at `search`.
//! * Prices are not here: they belong to the sales module's price lists (build step 5).

use crate::brands;
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

/// A product. `sku` is its code name: the short unique code people type and scan. `name` is the English name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Product {
    pub id: String,
    pub sku: String,
    pub name: String,
    pub name_ur: Option<String>,
    pub model: Option<String>,
    pub barcode: Option<String>,
    pub category_id: Option<String>,
    pub company_id: Option<String>,
    pub brand_id: Option<String>,
    /// A scheme is "buy `scheme_buy`, get `scheme_free` free". Both are set or neither.
    pub scheme_buy: Option<i64>,
    pub scheme_free: Option<i64>,
    pub unit: String,
    pub is_active: bool,
    pub revision: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ProductInput {
    pub sku: String,
    pub name: String,
    pub name_ur: Option<String>,
    pub model: Option<String>,
    pub barcode: Option<String>,
    pub category_id: Option<String>,
    pub company_id: Option<String>,
    pub brand_id: Option<String>,
    pub scheme_buy: Option<i64>,
    pub scheme_free: Option<i64>,
    /// Lower-case word such as "pcs", "set", "litre". Blank means "pcs".
    pub unit: String,
}

pub(crate) const PCOLS: &str = "p.id,p.sku,p.name,p.name_ur,p.model,p.barcode,p.category_id,p.company_id,p.brand_id,p.scheme_buy,p.scheme_free,p.unit,p.is_active,p.revision";

pub(crate) fn prod_from_row(r: &Row) -> rusqlite::Result<Product> {
    Ok(Product {
        id: r.get(0)?,
        sku: r.get(1)?,
        name: r.get(2)?,
        name_ur: r.get(3)?,
        model: r.get(4)?,
        barcode: r.get(5)?,
        category_id: r.get(6)?,
        company_id: r.get(7)?,
        brand_id: r.get(8)?,
        scheme_buy: r.get(9)?,
        scheme_free: r.get(10)?,
        unit: r.get(11)?,
        is_active: r.get::<_, i64>(12)? == 1,
        revision: r.get(13)?,
    })
}

fn prod_json(p: &Product) -> serde_json::Value {
    serde_json::to_value(p).unwrap_or(serde_json::Value::Null)
}

struct Clean {
    sku: String,
    name: String,
    name_ur: Option<String>,
    model: Option<String>,
    barcode: Option<String>,
    category: Option<String>,
    company: Option<String>,
    brand: Option<String>,
    scheme: Option<(i64, i64)>,
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
    // Zero means "not set" (a blank number box), so 0/0 and blank/blank are both "no scheme".
    let (buy, free) = (i.scheme_buy.filter(|n| *n != 0), i.scheme_free.filter(|n| *n != 0));
    let scheme = match (buy, free) {
        (None, None) => None,
        (Some(b), Some(f)) if (1..=9999).contains(&b) && (1..=9999).contains(&f) => Some((b, f)),
        _ => return Err(UserError::BadInput("a scheme needs both a buy quantity and a free quantity, each from 1 to 9999")),
    };
    Ok(Clean {
        sku: code(&i.sku, 1, 40, "._/-", "code name must be 1 to 40 characters: A-Z, 0-9, dot, slash, dash, underscore")?,
        name: clean(&i.name, 2, 120, "English name must be 2 to 120 characters")?,
        name_ur: clean_opt(i.name_ur.as_deref(), 120, "Urdu name is too long")?,
        model: clean_opt(i.model.as_deref(), 60, "model is too long")?,
        barcode,
        category: clean_opt(i.category_id.as_deref(), 64, "invalid category")?,
        company: clean_opt(i.company_id.as_deref(), 64, "invalid company")?,
        brand: clean_opt(i.brand_id.as_deref(), 64, "invalid brand")?,
        scheme,
        unit,
    })
}

pub fn get_product(conn: &Connection, tenant: &str, id: &str) -> Result<Option<Product>, DbError> {
    Ok(conn
        .query_row(&format!("SELECT {PCOLS} FROM products p WHERE p.tenant_id=?1 AND p.id=?2"), params![tenant, id], prod_from_row)
        .optional()?)
}

#[derive(Debug, Clone, Default)]
pub struct ProductQuery {
    pub search: Option<String>,
    pub category_id: Option<String>,
    pub brand_id: Option<String>,
    pub company_id: Option<String>,
    pub include_inactive: bool,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProductPage {
    pub items: Vec<Product>,
    pub total: i64,
}

/// Text search covers the English and Urdu names, code name, model, brand, company and an exact barcode.
pub(crate) const FROM: &str = "products p LEFT JOIN brands b ON b.id = p.brand_id LEFT JOIN companies c ON c.id = p.company_id";
pub(crate) const FILTER: &str = "p.tenant_id=?1
    AND (?2 IS NULL OR instr(lower(p.name),lower(?2))>0 OR instr(lower(p.sku),lower(?2))>0
         OR instr(COALESCE(p.name_ur,''),?2)>0 OR instr(lower(COALESCE(p.model,'')),lower(?2))>0
         OR instr(lower(COALESCE(b.name,'')),lower(?2))>0 OR instr(lower(COALESCE(c.name,'')),lower(?2))>0
         OR p.barcode=?2)
    AND (?3 IS NULL OR p.category_id=?3)
    AND (?4 IS NULL OR p.brand_id=?4)
    AND (?5 IS NULL OR p.company_id=?5)
    AND (?6=1 OR p.is_active=1)";

/// Results are ordered by English name, then code name.
pub fn search_products(conn: &Connection, tenant: &str, q: &ProductQuery) -> Result<ProductPage, DbError> {
    let search = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(60).collect::<String>());
    let limit = if q.limit <= 0 { 50 } else { q.limit.min(200) };
    let offset = q.offset.max(0);
    let inc = q.include_inactive as i64;
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM {FROM} WHERE {FILTER}"),
        params![tenant, search, q.category_id, q.brand_id, q.company_id, inc],
        |r| r.get(0),
    )?;
    let mut st = conn.prepare(&format!("SELECT {PCOLS} FROM {FROM} WHERE {FILTER} ORDER BY lower(p.name), p.sku LIMIT ?7 OFFSET ?8"))?;
    let rows = st.query_map(params![tenant, search, q.category_id, q.brand_id, q.company_id, inc, limit, offset], prod_from_row)?;
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

/// Company and brand must exist and be active (unless the product already uses an inactive one). A brand that
/// belongs to a company fills in the product's company when it is blank, and may not be mixed with another company.
/// Returns the (company, brand) to store.
fn check_links(tx: &Connection, tenant: &str, v: &Clean, current: Option<&Product>) -> Result<(Option<String>, Option<String>), UserError> {
    let mut company = v.company.clone();
    if let Some(bid) = &v.brand {
        let brand = brands::get_brand(tx, tenant, bid)?.ok_or(UserError::BadInput("unknown brand"))?;
        if !brand.is_active && current.and_then(|p| p.brand_id.as_deref()) != Some(bid.as_str()) {
            return Err(UserError::BadInput("that brand is inactive"));
        }
        match (&company, &brand.company_id) {
            (None, Some(bc)) => company = Some(bc.clone()),
            (Some(c), Some(bc)) if c != bc => return Err(UserError::BadInput("that brand belongs to a different company")),
            _ => {}
        }
    }
    if let Some(cid) = &company {
        let co = brands::get_company(tx, tenant, cid)?.ok_or(UserError::BadInput("unknown company"))?;
        if !co.is_active && current.and_then(|p| p.company_id.as_deref()) != Some(cid.as_str()) {
            return Err(UserError::BadInput("that company is inactive"));
        }
    }
    Ok((company, v.brand.clone()))
}

fn check_unique(tx: &Connection, tenant: &str, v: &Clean, except: Option<&str>) -> Result<(), UserError> {
    if sku_taken(tx, tenant, &v.sku, except)? {
        return Err(UserError::Conflict("a product with that code name already exists"));
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
    let (company, brand) = check_links(&tx, tenant, &v, None)?;
    let id = uuid::Uuid::new_v4().to_string();
    let (sb, sf) = v.scheme.map(|(b, f)| (Some(b), Some(f))).unwrap_or((None, None));
    tx.execute(
        "INSERT INTO products(id,tenant_id,sku,name,name_ur,model,barcode,category_id,company_id,brand_id,scheme_buy,scheme_free,unit,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)",
        params![id, tenant, v.sku, v.name, v.name_ur, v.model, v.barcode, v.category, company, brand, sb, sf, v.unit, now],
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
    let (company, brand) = check_links(&tx, tenant, &v, Some(&before))?;
    let (sb, sf) = v.scheme.map(|(b, f)| (Some(b), Some(f))).unwrap_or((None, None));
    tx.execute(
        "UPDATE products SET sku=?3, name=?4, name_ur=?5, model=?6, barcode=?7, category_id=?8, company_id=?9, brand_id=?10,
                scheme_buy=?11, scheme_free=?12, unit=?13, revision=revision+1, updated_at=?14
         WHERE tenant_id=?1 AND id=?2",
        params![tenant, id, v.sku, v.name, v.name_ur, v.model, v.barcode, v.category, company, brand, sb, sf, v.unit, now],
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
            &ProductInput { sku: " bp-1234/a ".into(), name: " Brake pad front ".into(), barcode: Some(" 8901234567890 ".into()), category_id: Some(cat.id.clone()), unit: "set".into(), ..Default::default() },
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

    // ---------- product details: Urdu name, model, company, brand, scheme ----------

    fn urdu() -> String {
        "بریک پیڈ فرنٹ".to_string()
    }

    fn full(sku: &str, name: &str) -> ProductInput {
        ProductInput { sku: sku.into(), name: name.into(), ..Default::default() }
    }

    #[test]
    fn a_product_keeps_its_urdu_name_model_company_brand_and_scheme() {
        let mut c = setup();
        let co = brands::create_company(&mut c, "t1", &mgr(), None, "Atlas Honda", T0).unwrap();
        let br = brands::create_brand(&mut c, "t1", &mgr(), None, "Honda Genuine", Some(&co.id), T0).unwrap();
        let p = create_product(
            &mut c, "t1", &mgr(), None,
            &ProductInput { name_ur: Some(format!("  {}  ", urdu())), model: Some(" CD 70 ".into()), company_id: Some(co.id.clone()), brand_id: Some(br.id.clone()), scheme_buy: Some(10), scheme_free: Some(1), ..full("bp-1", "Brake pad front") },
            T0,
        )
        .unwrap();
        assert_eq!(p.name_ur.as_deref(), Some(urdu().as_str()));
        assert_eq!((p.model.as_deref(), p.company_id.as_deref(), p.brand_id.as_deref()), (Some("CD 70"), Some(co.id.as_str()), Some(br.id.as_str())));
        assert_eq!((p.scheme_buy, p.scheme_free), (Some(10), Some(1)));
        let again = get_product(&c, "t1", &p.id).unwrap().unwrap();
        assert_eq!(again, p);
        // blank optional text means "none"
        let plain = create_product(&mut c, "t1", &mgr(), None, &ProductInput { name_ur: Some("  ".into()), model: Some("".into()), ..full("pl-1", "Plain part") }, T0).unwrap();
        assert_eq!((plain.name_ur, plain.model, plain.scheme_buy, plain.scheme_free), (None, None, None, None));
        let snap = outbox(&c).len();
        assert!(snap >= 4, "company, brand and both products were queued");
    }

    #[test]
    fn a_scheme_needs_both_numbers_or_neither() {
        let mut c = setup();
        let mk = |c: &mut Connection, b: Option<i64>, f: Option<i64>, sku: &str| create_product(c, "t1", &mgr(), None, &ProductInput { scheme_buy: b, scheme_free: f, ..full(sku, "Scheme part") }, T0);
        assert!(mk(&mut c, Some(5), Some(1), "S1").is_ok());
        assert!(mk(&mut c, None, None, "S2").is_ok());
        let zeros = mk(&mut c, Some(0), Some(0), "S3").unwrap();
        assert_eq!((zeros.scheme_buy, zeros.scheme_free), (None, None), "0 and 0 are blank number boxes, not a scheme");
        for (b, f) in [(Some(5), None), (None, Some(1)), (Some(5), Some(0)), (Some(0), Some(1)), (Some(-1), Some(1)), (Some(10000), Some(1)), (Some(5), Some(10000))] {
            assert!(matches!(mk(&mut c, b, f, "S9"), Err(UserError::BadInput(_))), "{b:?}/{f:?}");
        }
    }

    #[test]
    fn the_database_itself_refuses_half_a_scheme() {
        let mut c = setup();
        let p = add(&mut c, "SCH-1", "Scheme part");
        assert!(c.execute("UPDATE products SET scheme_buy=5 WHERE id=?1", [&p.id]).is_err(), "buy without free");
        assert!(c.execute("UPDATE products SET scheme_free=1 WHERE id=?1", [&p.id]).is_err(), "free without buy");
        assert!(c.execute("UPDATE products SET scheme_buy=5, scheme_free=1 WHERE id=?1", [&p.id]).is_ok());
        assert!(c.execute("UPDATE products SET scheme_buy=0, scheme_free=1 WHERE id=?1", [&p.id]).is_err());
    }

    #[test]
    fn a_brand_that_belongs_to_a_company_fills_the_company_in_and_cannot_be_mixed() {
        let mut c = setup();
        let honda = brands::create_company(&mut c, "t1", &mgr(), None, "Atlas Honda", T0).unwrap();
        let suzuki = brands::create_company(&mut c, "t1", &mgr(), None, "Pak Suzuki", T0).unwrap();
        let genuine = brands::create_brand(&mut c, "t1", &mgr(), None, "Honda Genuine", Some(&honda.id), T0).unwrap();
        let local = brands::create_brand(&mut c, "t1", &mgr(), None, "Local", None, T0).unwrap();
        let p = create_product(&mut c, "t1", &mgr(), None, &ProductInput { brand_id: Some(genuine.id.clone()), ..full("A1", "Filled in") }, T0).unwrap();
        assert_eq!(p.company_id.as_deref(), Some(honda.id.as_str()), "company comes from the brand");
        let clash = create_product(&mut c, "t1", &mgr(), None, &ProductInput { brand_id: Some(genuine.id.clone()), company_id: Some(suzuki.id.clone()), ..full("A2", "Mixed") }, T0);
        assert!(matches!(clash, Err(UserError::BadInput(_))));
        // a brand with no company goes with any company
        let ok = create_product(&mut c, "t1", &mgr(), None, &ProductInput { brand_id: Some(local.id.clone()), company_id: Some(suzuki.id.clone()), ..full("A3", "Free mix") }, T0).unwrap();
        assert_eq!((ok.brand_id.as_deref(), ok.company_id.as_deref()), (Some(local.id.as_str()), Some(suzuki.id.as_str())));
        // company alone is fine
        assert!(create_product(&mut c, "t1", &mgr(), None, &ProductInput { company_id: Some(honda.id.clone()), ..full("A4", "Company only") }, T0).is_ok());
        for bad in [ProductInput { brand_id: Some("nope".into()), ..full("B1", "Ghost brand") }, ProductInput { company_id: Some("nope".into()), ..full("B2", "Ghost company") }] {
            assert!(matches!(create_product(&mut c, "t1", &mgr(), None, &bad, T0), Err(UserError::BadInput(_))));
        }
    }

    #[test]
    fn inactive_brands_and_companies_cannot_be_newly_chosen_but_existing_products_keep_them() {
        let mut c = setup();
        let br = brands::create_brand(&mut c, "t1", &mgr(), None, "Old Brand", None, T0).unwrap();
        let p = create_product(&mut c, "t1", &mgr(), None, &ProductInput { brand_id: Some(br.id.clone()), ..full("O1", "Uses old brand") }, T0).unwrap();
        brands::set_brand_active(&mut c, "t1", &mgr(), None, &br.id, 1, false, T0).unwrap();
        let blocked = create_product(&mut c, "t1", &mgr(), None, &ProductInput { brand_id: Some(br.id.clone()), ..full("O2", "New with old brand") }, T0);
        assert!(matches!(blocked, Err(UserError::BadInput(_))));
        let kept = update_product(&mut c, "t1", &mgr(), None, &p.id, 1, &ProductInput { brand_id: Some(br.id.clone()), model: Some("CG 125".into()), ..full("O1", "Uses old brand") }, T0).unwrap();
        assert_eq!((kept.brand_id.as_deref(), kept.model.as_deref()), (Some(br.id.as_str()), Some("CG 125")));
        let dropped = update_product(&mut c, "t1", &mgr(), None, &p.id, 2, &full("O1", "Uses old brand"), T0).unwrap();
        assert_eq!(dropped.brand_id, None, "clearing it is always allowed");
    }

    #[test]
    fn another_shops_brand_or_company_is_unknown_and_the_database_agrees() {
        let mut c = setup();
        c.execute("INSERT INTO tenants(id,name) VALUES ('t2','Other')", []).unwrap();
        let theirs = brands::create_brand(&mut c, "t2", &mgr(), None, "Theirs", None, T0).unwrap();
        let their_co = brands::create_company(&mut c, "t2", &mgr(), None, "Their Co", T0).unwrap();
        assert!(matches!(create_product(&mut c, "t1", &mgr(), None, &ProductInput { brand_id: Some(theirs.id.clone()), ..full("X1", "Cross brand") }, T0), Err(UserError::BadInput(_))));
        assert!(matches!(create_product(&mut c, "t1", &mgr(), None, &ProductInput { company_id: Some(their_co.id.clone()), ..full("X2", "Cross company") }, T0), Err(UserError::BadInput(_))));
        // even raw SQL cannot link across shops
        let raw = |col: &str, id: &str| c.execute(&format!("INSERT INTO products(id,tenant_id,sku,name,{col}) VALUES ('rawp','t1','RAW','Raw product',?1)"), [id]);
        assert!(raw("brand_id", &theirs.id).is_err());
        assert!(raw("company_id", &their_co.id).is_err());
    }

    #[test]
    fn search_finds_products_by_urdu_name_model_brand_and_company() {
        let mut c = setup();
        let honda = brands::create_company(&mut c, "t1", &mgr(), None, "Atlas Honda", T0).unwrap();
        let suzuki = brands::create_company(&mut c, "t1", &mgr(), None, "Pak Suzuki", T0).unwrap();
        let genuine = brands::create_brand(&mut c, "t1", &mgr(), None, "Honda Genuine", Some(&honda.id), T0).unwrap();
        let mk = |c: &mut Connection, i: ProductInput| create_product(c, "t1", &mgr(), None, &i, T0).unwrap();
        mk(&mut c, ProductInput { name_ur: Some(urdu()), model: Some("CD 70".into()), brand_id: Some(genuine.id.clone()), ..full("P1", "Brake pad front") });
        mk(&mut c, ProductInput { model: Some("GS 150".into()), company_id: Some(suzuki.id.clone()), ..full("P2", "Clutch plate") });
        mk(&mut c, full("P3", "Spark plug"));
        let s = |c: &Connection, t: &str| search_products(c, "t1", &ProductQuery { search: Some(t.into()), ..Default::default() }).unwrap();
        assert_eq!(s(&c, "پیڈ").items[0].sku, "P1", "Urdu name");
        assert_eq!(s(&c, "cd 70").items[0].sku, "P1", "model, ignoring case");
        assert_eq!(s(&c, "genuine").total, 1, "brand name");
        assert_eq!(s(&c, "suzuki").items[0].sku, "P2", "company name");
        assert_eq!(s(&c, "atlas").items[0].sku, "P1", "the company a brand brought along");
        assert_eq!(s(&c, "spark").total, 1);
        let by = |c: &Connection, brand: Option<&str>, company: Option<&str>| {
            search_products(c, "t1", &ProductQuery { brand_id: brand.map(Into::into), company_id: company.map(Into::into), ..Default::default() }).unwrap().total
        };
        assert_eq!(by(&c, Some(&genuine.id), None), 1);
        assert_eq!(by(&c, None, Some(&honda.id)), 1);
        assert_eq!(by(&c, None, Some(&suzuki.id)), 1);
        assert_eq!(by(&c, Some(&genuine.id), Some(&suzuki.id)), 0, "filters combine");
        assert_eq!(by(&c, None, None), 3);
    }
}
