import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { describeError } from "../api/errors.ts";

interface Props {
  title: string;
  /** Label of the main button: say what it does ("Save customer"), not "Submit". */
  submitLabel: string;
  onClose: () => void;
  /** Return normally to close the dialog; throw to keep it open and show the problem. */
  onSubmit: () => Promise<void>;
  canSubmit?: boolean;
  wide?: boolean;
  children: ReactNode;
}

/** A modal form on the native <dialog> element: focus is trapped and Esc closes it. */
export function FormModal({ title, submitLabel, onClose, onSubmit, canSubmit = true, wide, children }: Props) {
  const ref = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const d = ref.current;
    if (d && !d.open) d.showModal();
  }, []);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await onSubmit();
      onClose();
    } catch (err) {
      setError(describeError(err));
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={ref}
      className={wide ? "wide" : undefined}
      aria-labelledby="modal-title"
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      <form className="mform" onSubmit={submit}>
        <header>
          <h2 id="modal-title">{title}</h2>
          <button type="button" className="x" aria-label="Close" onClick={onClose}>
            ×
          </button>
        </header>
        <div className="mbody">{children}</div>
        {error && (
          <p className="err" role="alert">
            {error}
          </p>
        )}
        <footer>
          <button type="button" className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn primary" disabled={busy || !canSubmit}>
            {submitLabel}
          </button>
        </footer>
      </form>
    </dialog>
  );
}

/** "Are you sure?" for things that cannot be undone from the screen. */
export function ConfirmModal(props: { title: string; confirmLabel: string; onClose: () => void; onConfirm: () => Promise<void>; children: ReactNode }) {
  return (
    <FormModal title={props.title} submitLabel={props.confirmLabel} onClose={props.onClose} onSubmit={props.onConfirm}>
      {props.children}
    </FormModal>
  );
}
