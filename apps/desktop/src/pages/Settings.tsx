import { useSession } from "../state/session.tsx";
import { useHashRoute } from "../state/router.ts";
import { Tabs } from "../ui/bits.tsx";
import { Account } from "./Account.tsx";
import { Roles } from "./Roles.tsx";
import { Users } from "./Users.tsx";

type Tab = "account" | "users" | "roles";

export function Settings() {
  const { can } = useSession();
  const { arg, navigate } = useHashRoute();
  const tabs: { id: Tab; label: string }[] = [{ id: "account", label: "My account" }];
  if (can("users.manage")) tabs.push({ id: "users", label: "Users" });
  if (can("roles.manage")) tabs.push({ id: "roles", label: "Roles" });
  const current = (tabs.find((t) => t.id === arg) ?? tabs[0]!).id;
  return (
    <>
      <Tabs tabs={tabs} current={current} label="Settings sections" onPick={(id) => navigate(`settings/${id}`)} />
      {current === "account" ? <Account /> : current === "users" ? <Users /> : <Roles />}
    </>
  );
}
