import { findItem } from "../nav.ts";

export function ComingSoon({ id }: { id: string }) {
  const item = findItem(id);
  if (!item?.soon) return null;
  return (
    <section className="panel" aria-labelledby="soon-title" style={{ maxWidth: 640 }}>
      <h2 id="soon-title">{item.label} is not built yet</h2>
      <p>
        This page exists in the prototype, but it needs parts of the shop server that are still to come. It is planned for <b>{item.soon.step.toLowerCase()}</b>.
      </p>
      <h3>What it will do</h3>
      <ul>
        {item.soon.what.map((w) => (
          <li key={w}>{w}</li>
        ))}
      </ul>
      <p className="sm">
        <a href="#/dashboard">Back to the dashboard</a>
      </p>
    </section>
  );
}
