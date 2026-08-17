/**
 * The workspace pages.
 *
 * `activeNav` used to be a bare `string` threaded through the sidebar, the
 * overview page and a seven-deep ternary whose `else` branch was the Logs
 * page — so any typo or future rename silently rendered Logs instead of
 * failing. Naming the set makes both the router table and every `onNavigate`
 * caller exhaustive.
 */
export const NAV_IDS = [
  "Overview",
  "Games",
  "Scorebook",
  "Challenges",
  "Engines",
  "Logs",
  "Settings",
] as const;

export type NavId = (typeof NAV_IDS)[number];

export function isNavId(value: string): value is NavId {
  return (NAV_IDS as readonly string[]).includes(value);
}
