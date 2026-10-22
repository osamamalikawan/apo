import { useEffect, useRef, useState } from "react";

const MIN = 4;
const MAX = 8;

/** Tapping a key with the mouse must not move focus onto it, or a later Enter would press that key again. */
const keepFocus = (e: { preventDefault: () => void }) => e.preventDefault();

interface Props {
  /** Return true when the PIN was accepted. Any other result clears the digits for another try. */
  onSubmit: (pin: string) => Promise<boolean>;
  busy?: boolean;
  error?: string | null;
}

/** On-screen number pad for a PIN of 4 to 8 digits. Physical keys work too: digits, Backspace and Enter. */
export function PinPad({ onSubmit, busy = false, error = null }: Props) {
  const [digits, setDigits] = useState("");
  const [shake, setShake] = useState(0);
  const latest = useRef({ digits, busy });
  latest.current = { digits, busy };

  const push = (d: string) => setDigits((x) => (x.length < MAX ? x + d : x));
  const back = () => setDigits((x) => x.slice(0, -1));

  async function go() {
    const { digits: pin, busy: b } = latest.current;
    if (pin.length < MIN || b) return;
    const ok = await onSubmit(pin);
    if (!ok) {
      setDigits("");
      setShake((n) => n + 1);
    }
  }

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
      if (/^[0-9]$/.test(e.key)) push(e.key);
      else if (e.key === "Backspace") back();
      else if (e.key === "Enter") {
        if (t && t.tagName === "BUTTON") return; // a focused key handles its own Enter
        e.preventDefault();
        void go();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const slots = Math.max(MIN, digits.length);
  return (
    <div role="group" aria-label="PIN pad">
      <div key={shake} className={"pin-dots" + (shake ? " bad" : "")} aria-hidden="true">
        {Array.from({ length: slots }, (_, i) => (
          <i key={i} className={i < digits.length ? "on" : ""} />
        ))}
      </div>
      <p className={"pin-msg" + (error ? " bad" : "")} role={error ? "alert" : "status"}>
        {error ?? (digits.length ? `${digits.length} digit${digits.length === 1 ? "" : "s"} entered` : "4 to 8 digits")}
      </p>
      <div className="keys">
        {["1", "2", "3", "4", "5", "6", "7", "8", "9"].map((d) => (
          <button key={d} type="button" className="key" onMouseDown={keepFocus} onClick={() => push(d)}>
            {d}
          </button>
        ))}
        <button type="button" className="key" aria-label="Delete last digit" onMouseDown={keepFocus} onClick={back} disabled={!digits}>
          ⌫
        </button>
        <button type="button" className="key" onMouseDown={keepFocus} onClick={() => push("0")}>
          0
        </button>
        <button type="button" className="key go" aria-label="Sign in" onMouseDown={keepFocus} onClick={() => void go()} disabled={digits.length < MIN || busy}>
          ✓
        </button>
      </div>
    </div>
  );
}
