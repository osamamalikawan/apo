import { useEffect, useState, type FormEvent } from "react";
import { describeError } from "../api/errors.ts";
import type { LoginUser } from "../api/types.ts";
import { useSession } from "../state/session.tsx";
import { PinPad } from "../ui/PinPad.tsx";

type Step = { kind: "pick" } | { kind: "pin"; user: LoginUser } | { kind: "password"; username: string };

function initialsOf(name: string): string {
  const w = name.split(/\s+/).filter(Boolean);
  return (w.length > 1 ? w[0]![0]! + w[1]![0]! : name.slice(0, 2)).toUpperCase();
}

/** Sign in: tap your name, then enter your PIN. A password form is there for first-time setup and as a fallback. */
export function Login() {
  const { api, signIn, tenantName, notice } = useSession();
  const [people, setPeople] = useState<LoginUser[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [step, setStep] = useState<Step>({ kind: "pick" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setLoadError(null);
    api
      .get<LoginUser[]>("/v1/login-users")
      .then((p) => alive && setPeople(p))
      .catch((e) => alive && setLoadError(describeError(e)));
    return () => {
      alive = false;
    };
  }, [api, attempt]);

  async function attemptSignIn(username: string, secret: string, method: "pin" | "password"): Promise<boolean> {
    setBusy(true);
    setError(null);
    try {
      await signIn(username, secret, method);
      return true;
    } catch (e) {
      setError(describeError(e));
      return false;
    } finally {
      setBusy(false);
    }
  }

  const go = (s: Step) => {
    setError(null);
    setStep(s);
  };

  let body;
  if (loadError) {
    body = (
      <>
        <p className="err inline" role="alert">
          {loadError}
        </p>
        <p style={{ marginTop: 14 }}>
          <button type="button" className="btn" onClick={() => setAttempt((n) => n + 1)}>
            Try again
          </button>
        </p>
      </>
    );
  } else if (people === null) {
    body = <p className="muted">Loading…</p>;
  } else if (step.kind === "pin") {
    const user = step.user;
    body = (
      <>
        <div className="who">
          <span className="avatar" aria-hidden="true">{initialsOf(user.full_name)}</span>
          <b>{user.full_name}</b>
        </div>
        <PinPad busy={busy} error={error} onSubmit={(pin) => attemptSignIn(user.username, pin, "pin")} />
        <div className="signin-links">
          <button type="button" className="linkbtn" onClick={() => go({ kind: "pick" })}>
            Not you? Choose someone else
          </button>
          <button type="button" className="linkbtn" onClick={() => go({ kind: "password", username: user.username })}>
            Use my password
          </button>
        </div>
      </>
    );
  } else if (step.kind === "password" || people.length === 0) {
    const preset = step.kind === "password" ? step.username : "";
    body = (
      <PasswordForm
        preset={preset}
        busy={busy}
        error={error}
        intro={people.length === 0 ? "No one has set a PIN yet. Sign in with your password, then set a PIN under Settings." : null}
        onSubmit={(u, p) => attemptSignIn(u, p, "password")}
        onBack={people.length ? () => go({ kind: "pick" }) : null}
      />
    );
  } else {
    body = (
      <>
        <div className="people">
          {people.map((p) => (
            <button key={p.id} type="button" className="person" onClick={() => go({ kind: "pin", user: p })}>
              <span className="avatar" aria-hidden="true">{initialsOf(p.full_name)}</span>
              <span>{p.full_name}</span>
            </button>
          ))}
        </div>
        <div className="signin-links">
          <button type="button" className="linkbtn" onClick={() => go({ kind: "password", username: "" })}>
            Sign in with a password instead
          </button>
        </div>
      </>
    );
  }

  const heading = step.kind === "pin" ? "Enter your PIN" : step.kind === "password" || people?.length === 0 ? "Sign in" : "Who is signing in?";
  return (
    <div className="signin">
      <div className="signin-card">
        <h1>{tenantName}</h1>
        <p className="sub">{heading}</p>
        {notice && !loadError && (
          <p className="note" role="status" style={{ marginBottom: 14 }}>
            {notice}
          </p>
        )}
        {body}
      </div>
    </div>
  );
}

function PasswordForm(props: {
  preset: string;
  busy: boolean;
  error: string | null;
  intro: string | null;
  onSubmit: (username: string, password: string) => Promise<boolean>;
  onBack: (() => void) | null;
}) {
  const [username, setUsername] = useState(props.preset);
  const [password, setPassword] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    const ok = await props.onSubmit(username.trim(), password);
    if (!ok) setPassword("");
  }
  return (
    <form onSubmit={submit}>
      {props.intro && <p className="note">{props.intro}</p>}
      <label>
        Username
        <input value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="username" autoFocus={!props.preset} required />
      </label>
      <label>
        Password
        <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="current-password" autoFocus={!!props.preset} required />
      </label>
      {props.error && (
        <p className="err inline" role="alert">
          {props.error}
        </p>
      )}
      <button className="btn primary" disabled={props.busy || !username.trim() || !password}>
        Sign in
      </button>
      {props.onBack && (
        <div className="signin-links">
          <button type="button" className="linkbtn" onClick={props.onBack}>
            Back to PIN sign-in
          </button>
        </div>
      )}
    </form>
  );
}
