import { useSession } from "../state/session.tsx";
import { useHashRoute } from "../state/router.ts";
import { Tabs } from "../ui/bits.tsx";
import { Locations } from "./Locations.tsx";
import { Products } from "./Products.tsx";

type Tab = "products" | "locations";

export function Inventory() {
  const { canAny } = useSession();
  const { arg, navigate } = useHashRoute();
  const tabs: { id: Tab; label: string }[] = [];
  if (canAny(["products.view", "products.manage"])) tabs.push({ id: "products", label: "Products" });
  tabs.push({ id: "locations", label: "Locations" });
  const current = (tabs.find((t) => t.id === arg) ?? tabs[0]!).id;
  return (
    <>
      <Tabs tabs={tabs} current={current} label="Inventory sections" onPick={(id) => navigate(`inventory/${id}`)} />
      <p className="note" style={{ marginBottom: 14 }}>
        Stock levels, transfers and stock counts arrive with build step 4. Products and locations are ready.
      </p>
      {current === "products" ? <Products /> : <Locations />}
    </>
  );
}
