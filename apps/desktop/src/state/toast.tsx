import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

interface Toast { id: number; text: string; error: boolean }
type Notify = (text: string, error?: boolean) => void;
const Ctx = createContext<Notify>(() => {});

export function useToast(): Notify {
  return useContext(Ctx);
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<Toast[]>([]);
  const notify = useCallback<Notify>((text, error = false) => {
    const id = Date.now() + Math.random();
    setItems((xs) => [...xs, { id, text, error }]);
    setTimeout(() => setItems((xs) => xs.filter((x) => x.id !== id)), error ? 8000 : 4000);
  }, []);
  const value = useMemo(() => notify, [notify]);
  return (
    <Ctx.Provider value={value}>
      {children}
      <div id="toasts" aria-live="polite">
        {items.map((t) => (
          <div key={t.id} className={"toast" + (t.error ? " err" : "")} role={t.error ? "alert" : "status"}>
            {t.text}
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}
