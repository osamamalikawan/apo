import { useState, type FormEvent } from "react";
import { describeError } from "../api/errors.ts";
import { useSession } from "../state/session.tsx";
import { useToast } from "../state/toast.tsx";

/** Your own PIN and password. Both need your current password, and a PIN session cannot change them. */
export function Account() {
  const { api, me, signOut } = useSession();
  const toast = useToast();
  const viaPin = me?.method === "pin";

  const [pinPassword, setPinPassword] = useState("");
  const [pin, setPin] = useState("");
  const [pinError, setPinError] = useState<string | null>(null);

  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [pwError, setPwError] = useState<string | null>(null);

  async function savePin(e: FormEvent) {
    e.preventDefault();
    setPinError(null);
    try {
      await api.post("/v1/me/pin", { current_password: pinPassword, pin });
      setPin("");
      setPinPassword("");
      toast("PIN saved. You can now sign in with it.");
    } catch (err) {
      setPinError(describeError(err));
    }
  }

  async function savePassword(e: FormEvent) {
    e.preventDefault();
    setPwError(null);
    if (next !== again) {
      setPwError("The two new passwords are not the same.");
      return;
    }
    try {
      await api.post("/v1/me/password", { current_password: current, new_password: next });
      await signOut("Password changed. Sign in with your new password.");
    } catch (err) {
      setPwError(describeError(err));
    }
  }

  return (
    <div className="cols">
      <div>
        {viaPin && (
          <p className="note lock-note" role="status">
            You signed in with a PIN. Sign out and sign in with your password to change your PIN or password.
          </p>
        )}
        <section className="panel" aria-labelledby="pin-h">
          <h2 id="pin-h">Your PIN</h2>
          <p className="muted">A PIN of 4 to 8 digits lets you sign in quickly at the counter.</p>
          <form onSubmit={savePin} style={{ display: "flex", flexDirection: "column", gap: 12, marginTop: 10 }}>
            <label>
              Current password
              <input type="password" value={pinPassword} onChange={(e) => setPinPassword(e.target.value)} autoComplete="current-password" disabled={viaPin} required />
            </label>
            <label>
              New PIN
              <input type="password" inputMode="numeric" pattern="[0-9]{4,8}" maxLength={8} value={pin} onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))} autoComplete="off" disabled={viaPin} required />
            </label>
            {pinError && <p className="err inline" role="alert">{pinError}</p>}
            <div><button className="btn primary" disabled={viaPin || pin.length < 4 || !pinPassword}>Save PIN</button></div>
          </form>
        </section>
      </div>
      <section className="panel" aria-labelledby="pw-h">
        <h2 id="pw-h">Change password</h2>
        <p className="muted">Use at least 10 characters. You will be signed out everywhere afterwards.</p>
        <form onSubmit={savePassword} style={{ display: "flex", flexDirection: "column", gap: 12, marginTop: 10 }}>
          <label>
            Current password
            <input type="password" value={current} onChange={(e) => setCurrent(e.target.value)} autoComplete="current-password" disabled={viaPin} required />
          </label>
          <label>
            New password
            <input type="password" value={next} onChange={(e) => setNext(e.target.value)} autoComplete="new-password" disabled={viaPin} required />
          </label>
          <label>
            New password again
            <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} autoComplete="new-password" disabled={viaPin} required />
          </label>
          {pwError && <p className="err inline" role="alert">{pwError}</p>}
          <div><button className="btn primary" disabled={viaPin || next.length < 10 || !current}>Change password</button></div>
        </form>
      </section>
    </div>
  );
}
