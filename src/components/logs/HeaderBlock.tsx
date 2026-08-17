import { Copy, FileText } from "lucide-react";
import type { LogHeaderField } from "../../types";
import { Button } from "../../ui/primitives";

/** The session's recorded header fields, collapsed until asked for. */
export function HeaderBlock({
  fields,
  onCopy,
}: {
  fields: LogHeaderField[];
  onCopy: () => void;
}) {
  return (
    <details className="logs-header-block">
      <summary>
        <FileText size={14} />
        Session header
        <em>
          {fields.length} field{fields.length === 1 ? "" : "s"}
        </em>
      </summary>
      <div className="logs-header-body">
        <dl className="logs-header-grid">
          {fields.map((field) => (
            <div key={field.key}>
              <dt>{field.key}</dt>
              <dd title={field.value}>{field.value}</dd>
            </div>
          ))}
        </dl>
        <Button variant="secondary" onClick={onCopy}>
          <Copy size={14} />
          Copy header
        </Button>
      </div>
    </details>
  );
}
