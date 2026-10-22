// Shapes returned by the Shop Server (apps/desktop/src-tauri/crates/lan-api). Keep in step with its README.

export type SignInMethod = "password" | "pin";

export interface LoginUser { id: string; username: string; full_name: string }
export interface Whoami { device_id: string; tenant_id: string; tenant_name: string }

export interface LoginResult {
  token: string;
  expires_at: string;
  method: SignInMethod;
  user: LoginUser;
  permissions: string[];
}

export interface Me {
  user: LoginUser;
  method: SignInMethod;
  permissions: string[];
  all_locations: boolean;
  location_ids: string[];
}

export type LicenseMode = "full" | "grace" | "read_only" | "unlicensed";
export interface License {
  mode: LicenseMode;
  reason: string | null;
  can_write: boolean;
  revision: number | null;
  full_until: string | null;
  grace_until: string | null;
  limits: { max_devices: number; max_users: number; max_locations: number } | null;
}

export interface Location {
  id: string; name: string; code: string; location_type: string;
  description: string | null; parent_location_id: string | null; responsible_user_id: string | null;
  is_active: boolean; revision: number;
}
export interface Category { id: string; name: string; revision: number }
export interface Product {
  id: string; sku: string; name: string; barcode: string | null; category_id: string | null;
  unit: string; is_active: boolean; revision: number;
}
export interface Customer {
  id: string; code: string; name: string; phone: string | null; address: string | null; notes: string | null;
  is_active: boolean; revision: number;
}
export interface Page<T> { items: T[]; total: number }

export interface Role { id: string; name: string; permissions: string[]; member_count: number; protected: boolean }
export interface PermissionInfo { permission: string; description: string }
export interface UserRow {
  id: string; username: string; full_name: string; is_active: boolean; role_ids: string[]; location_ids: string[];
}
