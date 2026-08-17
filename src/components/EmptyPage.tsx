import type { ReactNode } from "react";
import { Plus } from "lucide-react";
import { Button } from "../ui/primitives";

export function EmptyPage({
  icon,
  title,
  copy,
  action,
  onAction,
}: {
  icon: ReactNode;
  title: string;
  copy: string;
  action?: string;
  onAction?: () => void;
}) {
  return (
    <section className="empty-view panel">
      <div className="empty-icon">{icon}</div>
      <h2>{title}</h2>
      <p>{copy}</p>
      {action && (
        <Button variant="primary" onClick={onAction}>
          <Plus size={17} />
          {action}
        </Button>
      )}
    </section>
  );
}
