//! Master-data endpoints: locations, product categories, products, customers.
//! Every handler goes through `authz::check`. Updates quote `expected_revision` (the revision the
//! caller last read); a mismatch is 409 `stale_revision`. Records are deactivated, never deleted.
//!
//!   GET  /v1/locations                      any signed-in user, filtered to the locations they may use
//!   GET  /v1/locations/:id                  any signed-in user with access to that location
//!   POST /v1/locations                      locations.manage (needs locations.all; plan max_locations)
//!   PUT  /v1/locations/:id                  locations.manage, scoped to that location
//!   POST /v1/locations/:id/deactivate|activate
//!   GET  /v1/categories, POST, PUT /:id     products.view / products.manage
//!   GET  /v1/products?search=&category_id=&include_inactive=&limit=&offset=
//!   GET  /v1/products/:id, POST, PUT /:id, POST /:id/deactivate|activate
//!   GET  /v1/customers?search=&include_inactive=&limit=&offset=, GET /:id, POST, PUT /:id, POST /:id/deactivate|activate
//! `PUT` replaces the whole record: send every field, an omitted optional field is cleared.
//! Master-data edits do not need a password sign-in: a PIN session at the counter may do them.

use crate::authz::{self, Requirement};
use crate::{db, now_utc, user_err, ApiError, AppState, AuthedDevice};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Extension, Json, Router,
};
use dsaas_db::{
    catalog::{self, Category, Product, ProductInput, ProductPage, ProductQuery},
    customers::{self, Customer, CustomerInput, CustomerPage, CustomerQuery},
    locations::{self, Location, LocationInput},
    users::{Principal, Scope},
};
use serde::Deserialize;

const SIGNED_IN: Requirement = Requirement::signed_in();
const LOCATIONS_WRITE: Requirement = Requirement::write("locations.manage");
const PRODUCTS_READ: Requirement = Requirement::read("products.view");
const PRODUCTS_WRITE: Requirement = Requirement::write("products.manage");
const CUSTOMERS_READ: Requirement = Requirement::read("customers.view");
const CUSTOMERS_WRITE: Requirement = Requirement::write("customers.manage");

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/locations", get(list_locations).post(create_location))
        .route("/v1/locations/:id", get(get_location).put(update_location))
        .route("/v1/locations/:id/deactivate", post(deactivate_location))
        .route("/v1/locations/:id/activate", post(activate_location))
        .route("/v1/categories", get(list_categories).post(create_category))
        .route("/v1/categories/:id", put(rename_category))
        .route("/v1/products", get(list_products).post(create_product))
        .route("/v1/products/:id", get(get_product).put(update_product))
        .route("/v1/products/:id/deactivate", post(deactivate_product))
        .route("/v1/products/:id/activate", post(activate_product))
        .route("/v1/customers", get(list_customers).post(create_customer))
        .route("/v1/customers/:id", get(get_customer).put(update_customer))
        .route("/v1/customers/:id/deactivate", post(deactivate_customer))
        .route("/v1/customers/:id/activate", post(activate_customer))
}

#[derive(Deserialize)]
struct Revision {
    expected_revision: i64,
}

fn scope_of(id: &str) -> Scope<'_> {
    Scope { location_id: Some(id), route_id: None }
}

// ---------- locations ----------

#[derive(Deserialize)]
struct LocationBody {
    #[serde(default)]
    expected_revision: i64,
    name: String,
    code: String,
    #[serde(default)]
    location_type: String,
    description: Option<String>,
    parent_location_id: Option<String>,
    responsible_user_id: Option<String>,
}

impl LocationBody {
    fn input(&self) -> LocationInput {
        LocationInput {
            name: self.name.clone(),
            code: self.code.clone(),
            location_type: self.location_type.clone(),
            description: self.description.clone(),
            parent_location_id: self.parent_location_id.clone(),
            responsible_user_id: self.responsible_user_id.clone(),
        }
    }
}

async fn list_locations(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Vec<Location>>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let rows = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &SIGNED_IN, Scope::default()) {
            return Ok(Err(e));
        }
        Ok(Ok(locations::list_locations(c, &tenant, &p.access())?))
    })
    .await??;
    Ok(Json(rows))
}

async fn get_location(State(st): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Location>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let loc = db(&st, move |c| {
        // scope first: an inaccessible location and a missing one both answer 403, so ids cannot be probed
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &SIGNED_IN, scope_of(&id)) {
            return Ok(Err(e));
        }
        Ok(locations::get_location(c, &tenant, &id)?.ok_or(ApiError::NotFound))
    })
    .await??;
    Ok(Json(loc))
}

async fn create_location(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Json(body): Json<LocationBody>,
) -> Result<(StatusCode, Json<Location>), ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let loc = db(&st, move |c| {
        let granted = match authz::check(c, &tenant, &pk, &p, &LOCATIONS_WRITE, Scope::default()) {
            Ok(g) => g,
            Err(e) => return Ok(Err(e)),
        };
        let max = granted.entitlement.map(|e| e.limits.max_locations).unwrap_or(0);
        if locations::count_active(c, &tenant)? >= max {
            return Ok(Err(ApiError::LocationLimit));
        }
        let now = now_utc(c)?;
        Ok(locations::create_location(c, &tenant, &p, Some(&d.0), &body.input(), &now).map_err(user_err))
    })
    .await??;
    Ok((StatusCode::CREATED, Json(loc)))
}

async fn update_location(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(body): Json<LocationBody>,
) -> Result<Json<Location>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let loc = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &LOCATIONS_WRITE, scope_of(&id)) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(locations::update_location(c, &tenant, &p, Some(&d.0), &id, body.expected_revision, &body.input(), &now).map_err(user_err))
    })
    .await??;
    Ok(Json(loc))
}

async fn toggle_location(st: AppState, d: AuthedDevice, p: Principal, id: String, rev: i64, active: bool) -> Result<Json<Location>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let loc = db(&st, move |c| {
        let granted = match authz::check(c, &tenant, &pk, &p, &LOCATIONS_WRITE, scope_of(&id)) {
            Ok(g) => g,
            Err(e) => return Ok(Err(e)),
        };
        // bringing an inactive location back counts against the plan like creating one
        if active {
            let was_inactive = locations::get_location(c, &tenant, &id)?.map(|l| !l.is_active).unwrap_or(false);
            let max = granted.entitlement.map(|e| e.limits.max_locations).unwrap_or(0);
            if was_inactive && locations::count_active(c, &tenant)? >= max {
                return Ok(Err(ApiError::LocationLimit));
            }
        }
        let now = now_utc(c)?;
        Ok(locations::set_location_active(c, &tenant, &p, Some(&d.0), &id, rev, active, &now).map_err(user_err))
    })
    .await??;
    Ok(Json(loc))
}

async fn deactivate_location(State(st): State<AppState>, Extension(d): Extension<AuthedDevice>, Extension(p): Extension<Principal>, Path(id): Path<String>, Json(r): Json<Revision>) -> Result<Json<Location>, ApiError> {
    toggle_location(st, d, p, id, r.expected_revision, false).await
}

async fn activate_location(State(st): State<AppState>, Extension(d): Extension<AuthedDevice>, Extension(p): Extension<Principal>, Path(id): Path<String>, Json(r): Json<Revision>) -> Result<Json<Location>, ApiError> {
    toggle_location(st, d, p, id, r.expected_revision, true).await
}

// ---------- categories ----------

#[derive(Deserialize)]
struct CategoryBody {
    #[serde(default)]
    expected_revision: i64,
    name: String,
}

async fn list_categories(State(st): State<AppState>, Extension(p): Extension<Principal>) -> Result<Json<Vec<Category>>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let rows = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &PRODUCTS_READ, Scope::default()) {
            return Ok(Err(e));
        }
        Ok(Ok(catalog::list_categories(c, &tenant)?))
    })
    .await??;
    Ok(Json(rows))
}

async fn create_category(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Json(body): Json<CategoryBody>,
) -> Result<(StatusCode, Json<Category>), ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let cat = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &PRODUCTS_WRITE, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(catalog::create_category(c, &tenant, &p, Some(&d.0), &body.name, &now).map_err(user_err))
    })
    .await??;
    Ok((StatusCode::CREATED, Json(cat)))
}

async fn rename_category(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(body): Json<CategoryBody>,
) -> Result<Json<Category>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let cat = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &PRODUCTS_WRITE, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(catalog::rename_category(c, &tenant, &p, Some(&d.0), &id, body.expected_revision, &body.name, &now).map_err(user_err))
    })
    .await??;
    Ok(Json(cat))
}

// ---------- products ----------

#[derive(Deserialize)]
struct ProductBody {
    #[serde(default)]
    expected_revision: i64,
    sku: String,
    name: String,
    barcode: Option<String>,
    category_id: Option<String>,
    #[serde(default)]
    unit: String,
}

impl ProductBody {
    fn input(&self) -> ProductInput {
        ProductInput { sku: self.sku.clone(), name: self.name.clone(), barcode: self.barcode.clone(), category_id: self.category_id.clone(), unit: self.unit.clone() }
    }
}

#[derive(Deserialize)]
struct ListParams {
    search: Option<String>,
    category_id: Option<String>,
    include_inactive: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_products(State(st): State<AppState>, Extension(p): Extension<Principal>, Query(q): Query<ListParams>) -> Result<Json<ProductPage>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let page = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &PRODUCTS_READ, Scope::default()) {
            return Ok(Err(e));
        }
        let query = ProductQuery {
            search: q.search,
            category_id: q.category_id,
            include_inactive: q.include_inactive.unwrap_or(false),
            limit: q.limit.unwrap_or(0),
            offset: q.offset.unwrap_or(0),
        };
        Ok(Ok(catalog::search_products(c, &tenant, &query)?))
    })
    .await??;
    Ok(Json(page))
}

async fn get_product(State(st): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Product>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let prod = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &PRODUCTS_READ, Scope::default()) {
            return Ok(Err(e));
        }
        Ok(catalog::get_product(c, &tenant, &id)?.ok_or(ApiError::NotFound))
    })
    .await??;
    Ok(Json(prod))
}

async fn create_product(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Json(body): Json<ProductBody>,
) -> Result<(StatusCode, Json<Product>), ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let prod = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &PRODUCTS_WRITE, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(catalog::create_product(c, &tenant, &p, Some(&d.0), &body.input(), &now).map_err(user_err))
    })
    .await??;
    Ok((StatusCode::CREATED, Json(prod)))
}

async fn update_product(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(body): Json<ProductBody>,
) -> Result<Json<Product>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let prod = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &PRODUCTS_WRITE, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(catalog::update_product(c, &tenant, &p, Some(&d.0), &id, body.expected_revision, &body.input(), &now).map_err(user_err))
    })
    .await??;
    Ok(Json(prod))
}

async fn toggle_product(st: AppState, d: AuthedDevice, p: Principal, id: String, rev: i64, active: bool) -> Result<Json<Product>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let prod = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &PRODUCTS_WRITE, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(catalog::set_product_active(c, &tenant, &p, Some(&d.0), &id, rev, active, &now).map_err(user_err))
    })
    .await??;
    Ok(Json(prod))
}

async fn deactivate_product(State(st): State<AppState>, Extension(d): Extension<AuthedDevice>, Extension(p): Extension<Principal>, Path(id): Path<String>, Json(r): Json<Revision>) -> Result<Json<Product>, ApiError> {
    toggle_product(st, d, p, id, r.expected_revision, false).await
}

async fn activate_product(State(st): State<AppState>, Extension(d): Extension<AuthedDevice>, Extension(p): Extension<Principal>, Path(id): Path<String>, Json(r): Json<Revision>) -> Result<Json<Product>, ApiError> {
    toggle_product(st, d, p, id, r.expected_revision, true).await
}

// ---------- customers ----------

#[derive(Deserialize)]
struct CustomerBody {
    #[serde(default)]
    expected_revision: i64,
    #[serde(default)]
    code: String,
    name: String,
    phone: Option<String>,
    address: Option<String>,
    notes: Option<String>,
}

impl CustomerBody {
    fn input(&self) -> CustomerInput {
        CustomerInput { code: self.code.clone(), name: self.name.clone(), phone: self.phone.clone(), address: self.address.clone(), notes: self.notes.clone() }
    }
}

async fn list_customers(State(st): State<AppState>, Extension(p): Extension<Principal>, Query(q): Query<ListParams>) -> Result<Json<CustomerPage>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let page = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &CUSTOMERS_READ, Scope::default()) {
            return Ok(Err(e));
        }
        let query = CustomerQuery {
            search: q.search,
            include_inactive: q.include_inactive.unwrap_or(false),
            limit: q.limit.unwrap_or(0),
            offset: q.offset.unwrap_or(0),
        };
        Ok(Ok(customers::search_customers(c, &tenant, &query)?))
    })
    .await??;
    Ok(Json(page))
}

async fn get_customer(State(st): State<AppState>, Extension(p): Extension<Principal>, Path(id): Path<String>) -> Result<Json<Customer>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let cust = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &CUSTOMERS_READ, Scope::default()) {
            return Ok(Err(e));
        }
        Ok(customers::get_customer(c, &tenant, &id)?.ok_or(ApiError::NotFound))
    })
    .await??;
    Ok(Json(cust))
}

async fn create_customer(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Json(body): Json<CustomerBody>,
) -> Result<(StatusCode, Json<Customer>), ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let cust = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &CUSTOMERS_WRITE, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(customers::create_customer(c, &tenant, &p, Some(&d.0), &body.input(), &now).map_err(user_err))
    })
    .await??;
    Ok((StatusCode::CREATED, Json(cust)))
}

async fn update_customer(
    State(st): State<AppState>,
    Extension(d): Extension<AuthedDevice>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(body): Json<CustomerBody>,
) -> Result<Json<Customer>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let cust = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &CUSTOMERS_WRITE, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(customers::update_customer(c, &tenant, &p, Some(&d.0), &id, body.expected_revision, &body.input(), &now).map_err(user_err))
    })
    .await??;
    Ok(Json(cust))
}

async fn toggle_customer(st: AppState, d: AuthedDevice, p: Principal, id: String, rev: i64, active: bool) -> Result<Json<Customer>, ApiError> {
    let (tenant, pk) = (st.tenant_id.to_string(), st.license_pubkey);
    let cust = db(&st, move |c| {
        if let Err(e) = authz::check(c, &tenant, &pk, &p, &CUSTOMERS_WRITE, Scope::default()) {
            return Ok(Err(e));
        }
        let now = now_utc(c)?;
        Ok(customers::set_customer_active(c, &tenant, &p, Some(&d.0), &id, rev, active, &now).map_err(user_err))
    })
    .await??;
    Ok(Json(cust))
}

async fn deactivate_customer(State(st): State<AppState>, Extension(d): Extension<AuthedDevice>, Extension(p): Extension<Principal>, Path(id): Path<String>, Json(r): Json<Revision>) -> Result<Json<Customer>, ApiError> {
    toggle_customer(st, d, p, id, r.expected_revision, false).await
}

async fn activate_customer(State(st): State<AppState>, Extension(d): Extension<AuthedDevice>, Extension(p): Extension<Principal>, Path(id): Path<String>, Json(r): Json<Revision>) -> Result<Json<Customer>, ApiError> {
    toggle_customer(st, d, p, id, r.expected_revision, true).await
}
