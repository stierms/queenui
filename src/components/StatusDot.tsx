import { botStatusLabel } from "../lib/format";
import { isBotStatus, type BotRuntime } from "../types";

/**
 * The one-glance state of a bot.
 *
 * `status` is `string` on the wire. The visual is chosen from the set this
 * build knows, so a status it has never heard of renders as the neutral dot
 * rather than interpolating a backend string into the class attribute — the
 * lesson `campaignEventClass` already learned. The accessible name stays the
 * raw value: the dot cannot say what it is, the label still can.
 */
export function StatusDot({ status }: { status: BotRuntime["status"] }) {
  const visual = isBotStatus(status) ? status : "unknown";
  return (
    <span
      role="img"
      className={`status-dot status-${visual}`}
      aria-label={botStatusLabel(status)}
    />
  );
}
