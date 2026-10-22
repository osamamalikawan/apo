export interface NavItem {
  id: string;
  label: string;
  /** One letter shown in the collapsed rail. */
  badge: string;
  /** Visible to users holding ANY of these permissions. No list means everyone who is signed in. */
  any?: readonly string[];
  /** Pages from the prototype whose backend is not built yet. */
  soon?: { step: string; what: string[] };
}

export const READY: NavItem[] = [
  { id: "dashboard", label: "Dashboard", badge: "D" },
  { id: "customers", label: "Customers", badge: "C", any: ["customers.view", "customers.manage"] },
  { id: "inventory", label: "Inventory", badge: "I", any: ["inventory.view", "products.view", "products.manage", "locations.manage"] },
  { id: "settings", label: "Settings", badge: "E" },
];

export const SOON: NavItem[] = [
  { id: "sales", label: "Sales", badge: "S", soon: { step: "Build step 5", what: ["Invoices with brand discounts and packages such as buy 10, get 1 free", "Collect payments; the oldest unpaid bills are settled first", "Print thermal invoices"] } },
  { id: "returns", label: "Returns", badge: "U", soon: { step: "Build step 5", what: ["Return goods against an invoice", "Credit the customer and put the stock back"] } },
  { id: "dispatch", label: "Dispatch Vouchers", badge: "V", soon: { step: "Build steps 4 and 5", what: ["Send stock from a godown to the shop or a salesman", "Print dispatch vouchers"] } },
  { id: "routes", label: "Routes & Areas", badge: "R", soon: { step: "Build step 10", what: ["Cities, areas and routes", "Give each route a salesman and its customers"] } },
  { id: "planner", label: "Weekly Planner", badge: "P", soon: { step: "Build step 10", what: ["Drag routes onto weekdays", "See areas, customers and routes that no plan covers"] } },
  { id: "routeplan", label: "Route Plan", badge: "L", soon: { step: "Build step 10", what: ["Plan a day's route and the customers to visit"] } },
  { id: "planned", label: "Planned Routes", badge: "A", soon: { step: "Build step 10", what: ["Routes planned for the coming days"] } },
  { id: "completed", label: "Completed Routes", badge: "O", soon: { step: "Build step 10", what: ["Routes already driven, with customers visited and fuel cost"] } },
  { id: "team", label: "Sales Team", badge: "T", soon: { step: "Build step 10", what: ["Monthly targets for sales, collection and visits per salesman"] } },
  { id: "market", label: "Market Purchases", badge: "K", soon: { step: "Build step 5", what: ["Parts bought in the market, with cost, supplier and the invoice they were sold on"] } },
  { id: "expenses", label: "Expenses", badge: "X", soon: { step: "Build step 5", what: ["Record shop expenses by date and type"] } },
  { id: "vouchers", label: "Payment Vouchers", badge: "M", soon: { step: "Build step 5", what: ["Received (credit) and paid (debit) vouchers, with reversals"] } },
  { id: "sync", label: "Synchronization", badge: "Y", soon: { step: "Build step 6", what: ["What is waiting to go to the cloud, and any conflicts to resolve"] } },
];

export function visibleReady(canAny: (p: readonly string[]) => boolean): NavItem[] {
  return READY.filter((n) => !n.any || canAny(n.any));
}

export function findItem(id: string): NavItem | undefined {
  return [...READY, ...SOON].find((n) => n.id === id);
}
