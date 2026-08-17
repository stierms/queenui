import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type KeyboardEvent,
} from "react";
import { errorText } from "../../lib/errors";
import type { LogHighlight } from "../LogViewer";
import type { LogMatch } from "../../types";
import { MATCH_LIMIT } from "./shared";
import type { LogsSource } from "./source";

/**
 * In-session search: the query, its toggles, the match cursor, and the
 * highlight the viewer paints.
 *
 * Lifted out of `SessionViewer` because it is eight pieces of state that only
 * ever move together, and every one of them resets on the same events. The
 * viewer keeps what is left: the outline, the header, and the window of lines.
 *
 * Enter means "next match" once a query has been run, and "run this query"
 * when the text changed — the same key doing the obvious thing in both states.
 */
export function useSessionSearch({
  source,
  sessionId,
  onJump,
}: {
  source: LogsSource;
  sessionId: string;
  /** Scroll the viewer; centring is what a search hit wants. */
  onJump: (line: number, align?: "start" | "center") => void;
}) {
  const [queryText, setQueryText] = useState("");
  const [submitted, setSubmitted] = useState("");
  const [regex, setRegex] = useState(false);
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [matches, setMatches] = useState<LogMatch[]>([]);
  const [matchIndex, setMatchIndex] = useState(0);
  const [searched, setSearched] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);

  useEffect(() => {
    if (!submitted) return;
    let stale = false;
    source
      .searchSession(sessionId, {
        text: submitted,
        regex,
        caseSensitive,
        limit: MATCH_LIMIT,
      })
      .then((found) => {
        if (stale) return;
        setMatches(found);
        setMatchIndex(0);
        setSearched(true);
        setSearchError(null);
        if (found.length > 0) onJump(found[0].lineIndex, "center");
      })
      .catch((error) => {
        console.error("search_log_session failed:", error);
        if (stale) return;
        // "No matches" would read as "your text isn't in this log"; an
        // invalid regex or a dead backend has to say so.
        setMatches([]);
        setSearched(true);
        setSearchError(errorText(error));
      });
    return () => {
      stale = true;
    };
  }, [source, sessionId, submitted, regex, caseSensitive, onJump]);

  const stepMatch = useCallback(
    (delta: number) => {
      if (matches.length === 0) return;
      const next = (matchIndex + delta + matches.length) % matches.length;
      setMatchIndex(next);
      onJump(matches[next].lineIndex, "center");
    },
    [matches, matchIndex, onJump],
  );

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "Enter") return;
    event.preventDefault();
    const value = queryText.trim();
    if (!value) {
      setSubmitted("");
      setMatches([]);
      setMatchIndex(0);
      setSearched(false);
      setSearchError(null);
      return;
    }
    if (value !== submitted) {
      setSubmitted(value);
      return;
    }
    stepMatch(event.shiftKey ? -1 : 1);
  }

  const highlight = useMemo<LogHighlight | null>(
    () => (submitted ? { text: submitted, regex, caseSensitive } : null),
    [submitted, regex, caseSensitive],
  );

  const matchSummary = matches.length
    ? `${matchIndex + 1} of ${matches.length}`
    : searchError
      ? // The reason used to live in a `title` only, so an invalid regex and
        // a dead backend were the same two words to anyone not hovering.
        `Search failed — ${searchError}`
      : searched
        ? "No matches"
        : "";

  return {
    queryText,
    setQueryText,
    regex,
    setRegex,
    caseSensitive,
    setCaseSensitive,
    onKeyDown,
    stepMatch,
    hasMatches: matches.length > 0,
    matchSummary,
    searchError,
    highlight,
  };
}
