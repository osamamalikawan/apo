#!/usr/bin/env python3
"""Applies the real migration to an in-memory SQLite DB and checks the ledger rules.
Runs the SAME vectors as the TypeScript domain tests (packages/test-vectors)."""
import json, pathlib, sqlite3, sys, unittest, uuid

ROOT = pathlib.Path(__file__).resolve().parent.parent
MIGRATIONS = sorted((ROOT / "apps/desktop/src-tauri/crates/db/migrations").glob("*.sql"))
VECTORS = json.loads((ROOT / "packages/test-vectors/ledger-sequences.json").read_text())
T1, T2 = "tenant-1", "tenant-2"

def new_db():
    db = sqlite3.connect(":memory:", isolation_level=None)
    db.execute("PRAGMA foreign_keys = ON")
    for m in MIGRATIONS:
        db.executescript(m.read_text())
    return db

def seed(db, locs=("main", "g1", "g2"), tenant=T1):
    db.execute("INSERT OR IGNORE INTO tenants(id,name) VALUES (?,?)", (tenant, tenant))
    for l in locs:
        db.execute("INSERT INTO stock_locations(id,tenant_id,name,code) VALUES (?,?,?,?)", (f"{tenant}:{l}", tenant, l, f"{tenant}:{l}"))
    db.execute("INSERT INTO products(id,tenant_id,sku,name) VALUES (?,?,?,?)", (f"{tenant}:p1", tenant, "SKU1", "Product 1"))

def post(db, loc, delta, key, bucket="on_hand", allow_negative=False, tenant=T1, mtype="test"):
    """Returns 'posted' | 'duplicate' | 'rejected' like the reference ledger."""
    try:
        db.execute("BEGIN")
        db.execute(
            "INSERT INTO stock_movements(id,tenant_id,location_id,product_id,bucket,qty_delta,movement_type,source_type,source_id,idempotency_key,allow_negative)"
            " VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (str(uuid.uuid4()), tenant, f"{tenant}:{loc}", f"{tenant}:p1", bucket, delta, mtype, "test", key, key, int(allow_negative)))
        db.execute("COMMIT")
        return "posted"
    except sqlite3.IntegrityError as e:
        db.execute("ROLLBACK")
        if "UNIQUE" in str(e) and "idempotency_key" in str(e): return "duplicate"
        if "insufficient stock" in str(e): return "rejected"
        raise
    except sqlite3.DatabaseError as e:
        db.execute("ROLLBACK")
        if "insufficient stock" in str(e): return "rejected"
        raise

def balance(db, loc, tenant=T1):
    r = db.execute("SELECT on_hand,damaged,in_transit FROM inventory_balances WHERE location_id=? AND product_id=?", (f"{tenant}:{loc}", f"{tenant}:p1")).fetchone()
    return dict(zip(("on_hand", "damaged", "in_transit"), r or (0, 0, 0)))

def ledger_sum(db, loc, tenant=T1):
    out = {}
    for b in ("on_hand", "damaged", "in_transit"):
        out[b] = db.execute("SELECT COALESCE(SUM(qty_delta),0) FROM stock_movements WHERE location_id=? AND bucket=?", (f"{tenant}:{loc}", b)).fetchone()[0]
    return out

class SharedVectors(unittest.TestCase):
    pass

def make_vector_test(seq):
    def t(self):
        db = new_db(); seed(db)
        for s in seq["steps"]:
            got = post(db, s["loc"], s["delta"], s["key"], s["bucket"], s.get("allow_negative", False))
            self.assertEqual(got, s["expect"], f"step {s['key']}")
        for loc, want in seq["final"].items():
            self.assertEqual(balance(db, loc), want)
            self.assertEqual(ledger_sum(db, loc), want, "balance must reconcile with ledger")
        db.close()
    return t
for _s in VECTORS:
    setattr(SharedVectors, f"test_{_s['name']}", make_vector_test(_s))

class SchemaRules(unittest.TestCase):
    def setUp(self):
        self.db = new_db(); seed(self.db)
        self.addCleanup(self.db.close)

    def test_ledger_is_append_only(self):
        post(self.db, "main", 10, "r1")
        with self.assertRaisesRegex(sqlite3.DatabaseError, "append-only"):
            self.db.execute("UPDATE stock_movements SET qty_delta = 999")
        with self.assertRaisesRegex(sqlite3.DatabaseError, "append-only"):
            self.db.execute("DELETE FROM stock_movements")
        self.assertEqual(balance(self.db, "main")["on_hand"], 10)

    def test_audit_log_is_append_only(self):
        self.db.execute("INSERT INTO audit_logs(id,tenant_id,action,entity) VALUES ('a1',?,?,?)", (T1, "x", "y"))
        with self.assertRaisesRegex(sqlite3.DatabaseError, "append-only"):
            self.db.execute("DELETE FROM audit_logs")

    def test_cross_tenant_reference_is_blocked(self):
        seed(self.db, locs=("main",), tenant=T2)
        # tenant-1 movement pointing at a tenant-2 location must violate the composite FK
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute(
                "INSERT INTO stock_movements(id,tenant_id,location_id,product_id,qty_delta,movement_type,source_type,source_id,idempotency_key)"
                " VALUES ('m',?,?,?,5,'t','t','s','k')", (T1, f"{T2}:main", f"{T1}:p1"))

    def test_same_idempotency_key_allowed_in_different_tenants(self):
        seed(self.db, locs=("main",), tenant=T2)
        self.assertEqual(post(self.db, "main", 5, "same-key", tenant=T1), "posted")
        self.assertEqual(post(self.db, "main", 5, "same-key", tenant=T2), "posted")

    def test_location_cannot_be_deleted_and_deactivation_keeps_stock(self):
        post(self.db, "g1", 60, "r1")
        with self.assertRaisesRegex(sqlite3.DatabaseError, "cannot be deleted"):
            self.db.execute("DELETE FROM stock_locations WHERE id=?", (f"{T1}:g1",))
        self.db.execute("UPDATE stock_locations SET is_active=0 WHERE id=?", (f"{T1}:g1",))
        self.assertEqual(balance(self.db, "g1")["on_hand"], 60)
        self.assertEqual(self.db.execute("SELECT COUNT(*) FROM stock_movements WHERE location_id=?", (f"{T1}:g1",)).fetchone()[0], 1)

    def test_total_stock_is_sum_across_locations(self):
        post(self.db, "main", 20, "a"); post(self.db, "g1", 100, "b"); post(self.db, "g2", 50, "c")
        total = self.db.execute("SELECT SUM(on_hand) FROM inventory_balances WHERE tenant_id=? AND product_id=?", (T1, f"{T1}:p1")).fetchone()[0]
        self.assertEqual(total, 170)

    def test_transfer_in_one_transaction_is_all_or_nothing(self):
        post(self.db, "g1", 10, "r1")
        db = self.db
        try:
            db.execute("BEGIN")
            db.execute("INSERT INTO stock_movements(id,tenant_id,location_id,product_id,qty_delta,movement_type,source_type,source_id,idempotency_key) VALUES ('o',?,?,?,-4,'transfer_out','transfer','t1','t1:out')", (T1, f"{T1}:g1", f"{T1}:p1"))
            db.execute("INSERT INTO stock_movements(id,tenant_id,location_id,product_id,qty_delta,movement_type,source_type,source_id,idempotency_key) VALUES ('i',?,?,?,4,'transfer_in','transfer','t1','r1')", (T1, f"{T1}:main", f"{T1}:p1"))  # key clash with r1 -> fails
            db.execute("COMMIT")
        except sqlite3.IntegrityError:
            db.execute("ROLLBACK")
        self.assertEqual(balance(db, "g1")["on_hand"], 10, "source must be unchanged after a failed transfer")
        self.assertEqual(balance(db, "main")["on_hand"], 0)

    def test_outbox_dedupes_on_idempotency_key(self):
        q = "INSERT INTO sync_outbox(id,tenant_id,entity,entity_id,op,payload_json,idempotency_key) VALUES (?,?,?,?,?,?,?)"
        self.db.execute(q, ("1", T1, "stock_movement", "m1", "post", "{}", "k1"))
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute(q, ("2", T1, "stock_movement", "m1", "post", "{}", "k1"))

    def test_state_survives_reopen_on_disk(self):
        import tempfile, os
        path = os.path.join(tempfile.mkdtemp(), "shop.db")
        db = sqlite3.connect(path, isolation_level=None)
        db.execute("PRAGMA foreign_keys = ON"); db.execute("PRAGMA journal_mode = WAL")
        for m in MIGRATIONS: db.executescript(m.read_text())
        seed(db); post(db, "main", 42, "persist1"); db.close()
        db2 = sqlite3.connect(path, isolation_level=None)
        self.addCleanup(db2.close)
        self.assertEqual(balance(db2, "main")["on_hand"], 42)
        self.assertEqual(post(db2, "main", 42, "persist1"), "duplicate", "retry after restart must not double-post")


    def test_pairing_expiry_arithmetic_and_attempts_column(self):
        r = self.db.execute("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', '2026-10-10T10:00:00.000Z', '+600 seconds')").fetchone()[0]
        self.assertEqual(r, "2026-10-10T10:10:00.000Z")
        self.db.execute("INSERT INTO users(id,tenant_id,username,full_name) VALUES ('u1',?,?,?)", (T1, "admin", "Admin"))
        self.db.execute("INSERT INTO device_pairings(id,tenant_id,code_hash,created_by,expires_at) VALUES ('p1',?,?,?,?)", (T1, "h", "u1", r))
        self.assertEqual(self.db.execute("SELECT attempts FROM device_pairings WHERE id='p1'").fetchone()[0], 0)
        # ISO strings compare correctly as text, which the expiry check relies on
        self.assertTrue("2026-10-10T10:05:00.000Z" < r < "2026-10-10T10:11:00.000Z")

    def test_device_kind_and_pairing_tenant_ownership(self):
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO devices(id,tenant_id,name,kind,credential_hash) VALUES ('d',?,?,?,?)", (T1, "x", "toaster", "h"))
        seed(self.db, locs=("main",), tenant=T2)
        self.db.execute("INSERT INTO users(id,tenant_id,username,full_name) VALUES ('u2',?,?,?)", (T2, "admin", "A2"))
        with self.assertRaises(sqlite3.IntegrityError):  # tenant-1 pairing created_by a tenant-2 user
            self.db.execute("INSERT INTO device_pairings(id,tenant_id,code_hash,created_by,expires_at) VALUES ('p',?,?,?,?)", (T1, "h", "u2", "2030-01-01T00:00:00.000Z"))


    def test_entitlement_revision_cannot_decrease_or_be_deleted(self):
        self.db.execute("INSERT INTO entitlement_current(tenant_id,revision,payload_b64,signature_b64,status,applied_at) VALUES (?,?,?,?,?,?)", (T1, 5, "p", "s", "active", "2026-10-10T00:00:00Z"))
        with self.assertRaisesRegex(sqlite3.DatabaseError, "cannot decrease"):
            self.db.execute("UPDATE entitlement_current SET revision = 4")
        self.db.execute("UPDATE entitlement_current SET revision = 6")  # upward is fine
        with self.assertRaisesRegex(sqlite3.DatabaseError, "cannot be deleted"):
            self.db.execute("DELETE FROM entitlement_current")

    def test_entitlement_is_tenant_owned(self):
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute("INSERT INTO entitlement_current(tenant_id,revision,payload_b64,signature_b64,status,applied_at) VALUES ('nope',1,'p','s','active','x')")

    def test_clock_high_water_only_moves_forward(self):
        self.assertEqual(self.db.execute("SELECT high_water FROM clock_guard").fetchone()[0], "1970-01-01T00:00:00Z")
        self.db.execute("UPDATE clock_guard SET high_water='2026-10-10T00:00:00Z'")
        with self.assertRaisesRegex(sqlite3.DatabaseError, "cannot decrease"):
            self.db.execute("UPDATE clock_guard SET high_water='2026-10-09T00:00:00Z'")
        with self.assertRaises(sqlite3.IntegrityError):  # only one row may exist
            self.db.execute("INSERT INTO clock_guard(id,high_water) VALUES (2,'x')")
        # the guarded update the Rust code uses is a no-op when time went backwards
        self.db.execute("UPDATE clock_guard SET high_water=? WHERE high_water < ?", ("2026-10-01T00:00:00Z",) * 2)
        self.assertEqual(self.db.execute("SELECT high_water FROM clock_guard").fetchone()[0], "2026-10-10T00:00:00Z")

    def test_seconds_precision_timestamp_format(self):
        v = self.db.execute("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')").fetchone()[0]
        self.assertRegex(v, r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")

    def test_foreign_key_check_is_clean(self):
        post(self.db, "main", 1, "k")
        self.assertEqual(self.db.execute("PRAGMA foreign_key_check").fetchall(), [])

if __name__ == "__main__":
    unittest.main(verbosity=1)
