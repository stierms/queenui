import * as commands from "../../api/commands";
import { onDiagnostic, onLogsUpdated } from "../../api/events";
import type {
  DiagnosticEntry,
  DiagnosticFilter,
  ExportMode,
  LogFilter,
  LogMatch,
  LogPage,
  LogSearchBlock,
  LogSessionMatches,
  LogSessionSummary,
  LogsOverview,
} from "../../types";

/** A text query as both log search commands take it. */
export type LogSearchQuery = {
  text: string;
  regex: boolean;
  caseSensitive: boolean;
  limit: number;
};

/**
 * Every backend call the Logs page makes, gathered into one object so the
 * DEV preview (`?logs-preview`) can substitute the complete set without the
 * page knowing. Production passes nothing and gets the Tauri commands.
 */
export type LogsSource = {
  listSessions: (filter: LogFilter) => Promise<LogSessionSummary[]>;
  searchSessions: (
    filter: LogFilter,
    query: LogSearchQuery,
  ) => Promise<LogSessionMatches[]>;
  getPage: (
    sessionId: string,
    offset: number,
    limit: number,
  ) => Promise<LogPage>;
  getOutline: (sessionId: string) => Promise<LogSearchBlock[]>;
  searchSession: (
    sessionId: string,
    query: LogSearchQuery,
  ) => Promise<LogMatch[]>;
  exportSession: (
    sessionId: string,
    path: string,
    mode: ExportMode,
  ) => Promise<void>;
  deleteSession: (sessionId: string) => Promise<void>;
  clearSessions: () => Promise<number>;
  getOverview: () => Promise<LogsOverview>;
  getDiagnostics: (filter: DiagnosticFilter) => Promise<DiagnosticEntry[]>;
  clearDiagnostics: () => Promise<void>;
  subscribeLogs: (callback: () => void) => () => void;
  subscribeDiagnostics: (
    callback: (entry: DiagnosticEntry) => void,
  ) => () => void;
};

/** Arrow wrappers keep `vi.mock("../api/commands")` effective in tests. */
export const backendSource: LogsSource = {
  listSessions: (filter) => commands.listLogSessions(filter),
  searchSessions: (filter, query) => commands.searchLogSessions(filter, query),
  getPage: (sessionId, offset, limit) =>
    commands.getLogPage(sessionId, offset, limit),
  getOutline: (sessionId) => commands.getLogOutline(sessionId),
  searchSession: (sessionId, query) =>
    commands.searchLogSession(sessionId, query),
  exportSession: (sessionId, path, mode) =>
    commands.exportLogSession(sessionId, path, mode),
  deleteSession: (sessionId) => commands.deleteLogSession(sessionId),
  clearSessions: () => commands.clearLogSessions(),
  getOverview: () => commands.getLogsOverview(),
  getDiagnostics: (filter) => commands.getDiagnostics(filter),
  clearDiagnostics: () => commands.clearDiagnostics(),
  subscribeLogs: (callback) => onLogsUpdated(callback),
  subscribeDiagnostics: (callback) => onDiagnostic(callback),
};
