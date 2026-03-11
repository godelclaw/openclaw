import fs from "node:fs/promises";
import path from "node:path";
import { HEARTBEAT_TOKEN } from "../auto-reply/tokens.js";
import { resolveStateDir } from "../config/paths.js";

const HEARTBEAT_TURN_LOG_FILENAME = "heartbeat-turns.jsonl";
const DEFAULT_SCAN_LIMIT = 64;
export const DEFAULT_HEARTBEAT_RECENT_TURN_LIMIT = 5;
export const DEFAULT_HEARTBEAT_TRACE_LIMIT = 32;
const MAX_USER_TEXT_CHARS = 220;
const MAX_ASSISTANT_TEXT_CHARS = 320;

type DriverAuditEntry = {
  ts?: number;
  channel?: string;
  kind?: string;
  session_key?: string;
  user_text?: string;
  assistant_text?: string;
};

export type HeartbeatTurnLogEntry = {
  ts: number;
  agentId?: string;
  sessionKey?: string;
  status: string;
  reason?: string;
  userText: string;
  assistantText: string;
};

export type RecentTurnWindowEntry = {
  ts: number;
  source: string;
  sessionKey?: string;
  userText: string;
  assistantText: string;
  reserveMainTelegram: boolean;
  noopHeartbeat: boolean;
};

export function resolveHeartbeatTurnLogPath(
  statePath: string = resolveStateDir(process.env),
): string {
  return path.join(statePath, "logs", HEARTBEAT_TURN_LOG_FILENAME);
}

export async function appendHeartbeatTurnLog(
  entry: HeartbeatTurnLogEntry,
  statePath: string = resolveStateDir(process.env),
): Promise<void> {
  const logPath = resolveHeartbeatTurnLogPath(statePath);
  try {
    await fs.mkdir(path.dirname(logPath), { recursive: true });
    await fs.appendFile(logPath, JSON.stringify(entry) + "\n", "utf-8");
  } catch {
    // Best-effort only; heartbeat logging must never break the run.
  }
}

async function readJsonlEntries(filePath: string): Promise<string[]> {
  try {
    const raw = await fs.readFile(filePath, "utf-8");
    return raw
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
  } catch {
    return [];
  }
}

function truncateInline(text: string, maxChars: number): string {
  const normalized = text.replace(/\s+/g, " ").trim();
  if (normalized.length <= maxChars) {
    return normalized;
  }
  return `${normalized.slice(0, maxChars - 1)}…`;
}

function parseDriverAuditLine(
  line: string,
  mainSessionKey?: string,
): RecentTurnWindowEntry | null {
  try {
    const entry = JSON.parse(line) as DriverAuditEntry;
    const userText = typeof entry.user_text === "string" ? entry.user_text.trim() : "";
    const assistantText =
      typeof entry.assistant_text === "string" ? entry.assistant_text.trim() : "";
    if (!userText || !assistantText) {
      return null;
    }
    if (entry.kind !== "driver_turn") {
      return null;
    }
    const source = typeof entry.channel === "string" ? entry.channel.trim().toLowerCase() : "";
    if (!source || source === "internal") {
      return null;
    }
    const sessionKey =
      typeof entry.session_key === "string" ? entry.session_key.trim() : undefined;
    return {
      ts: typeof entry.ts === "number" ? entry.ts * 1000 : 0,
      source,
      sessionKey,
      userText,
      assistantText,
      reserveMainTelegram: source === "telegram" && sessionKey === mainSessionKey,
      noopHeartbeat: false,
    };
  } catch {
    return null;
  }
}

function parseHeartbeatTurnLine(line: string): RecentTurnWindowEntry | null {
  try {
    const entry = JSON.parse(line) as HeartbeatTurnLogEntry;
    const assistantText =
      typeof entry.assistantText === "string" ? entry.assistantText.trim() : "";
    if (!assistantText) {
      return null;
    }
    const userText = typeof entry.userText === "string" ? entry.userText.trim() : "Heartbeat";
    return {
      ts: entry.ts,
      source: "heartbeat",
      sessionKey: entry.sessionKey?.trim() || undefined,
      userText,
      assistantText,
      reserveMainTelegram: false,
      noopHeartbeat: assistantText === HEARTBEAT_TOKEN,
    };
  } catch {
    return null;
  }
}

async function loadDriverAuditTurns(params: {
  historyRoot: string;
  mainSessionKey?: string;
  scanLimit: number;
}): Promise<RecentTurnWindowEntry[]> {
  const auditDir = path.join(params.historyRoot, "audit");
  let names: string[] = [];
  try {
    names = (await fs.readdir(auditDir))
      .filter((name) => name.endsWith(".jsonl"))
      .sort()
      .reverse();
  } catch {
    return [];
  }

  const entries: RecentTurnWindowEntry[] = [];
  for (const name of names) {
    const lines = await readJsonlEntries(path.join(auditDir, name));
    for (const line of lines.reverse()) {
      const parsed = parseDriverAuditLine(line, params.mainSessionKey);
      if (parsed) {
        entries.push(parsed);
      }
      if (entries.length >= params.scanLimit) {
        return entries;
      }
    }
  }
  return entries;
}

async function loadHeartbeatTurns(params: {
  statePath: string;
  scanLimit: number;
}): Promise<RecentTurnWindowEntry[]> {
  const lines = await readJsonlEntries(resolveHeartbeatTurnLogPath(params.statePath));
  const entries: RecentTurnWindowEntry[] = [];
  for (const line of lines.reverse()) {
    const parsed = parseHeartbeatTurnLine(line);
    if (parsed) {
      entries.push(parsed);
    }
    if (entries.length >= params.scanLimit) {
      break;
    }
  }
  return entries;
}

export async function loadRecentHeartbeatTrace(params: {
  statePath?: string;
  scanLimit?: number;
}): Promise<HeartbeatTurnLogEntry[]> {
  const statePath = params.statePath ?? resolveStateDir(process.env);
  const lines = await readJsonlEntries(resolveHeartbeatTurnLogPath(statePath));
  const entries: HeartbeatTurnLogEntry[] = [];
  const scanLimit = Math.max(1, params.scanLimit ?? DEFAULT_HEARTBEAT_TRACE_LIMIT);
  for (const line of lines.reverse()) {
    try {
      const entry = JSON.parse(line) as HeartbeatTurnLogEntry;
      if (
        typeof entry.ts === "number" &&
        typeof entry.userText === "string" &&
        typeof entry.assistantText === "string"
      ) {
        entries.push(entry);
      }
    } catch {
      // Ignore malformed lines in best-effort trace loading.
    }
    if (entries.length >= scanLimit) {
      break;
    }
  }
  return entries.sort((a, b) => a.ts - b.ts);
}

function sameEntry(a: RecentTurnWindowEntry, b: RecentTurnWindowEntry): boolean {
  return (
    a.ts === b.ts &&
    a.source === b.source &&
    a.sessionKey === b.sessionKey &&
    a.userText === b.userText &&
    a.assistantText === b.assistantText
  );
}

export function selectRecentTurns(params: {
  entries: RecentTurnWindowEntry[];
  limit?: number;
  mainTelegramReserved?: boolean;
}): RecentTurnWindowEntry[] {
  const limit = Math.max(1, params.limit ?? DEFAULT_HEARTBEAT_RECENT_TURN_LIMIT);
  const sorted = [...params.entries].sort((a, b) => b.ts - a.ts);
  const picked: RecentTurnWindowEntry[] = [];
  if (params.mainTelegramReserved !== false) {
    const reserved = sorted.find((entry) => entry.reserveMainTelegram);
    if (reserved) {
      picked.push(reserved);
    }
  }
  const reservedHeartbeat = sorted.find(
    (entry) => entry.source === "heartbeat" && !entry.noopHeartbeat,
  );
  if (
    reservedHeartbeat &&
    picked.length < limit &&
    !picked.some((existing) => sameEntry(existing, reservedHeartbeat))
  ) {
    picked.push(reservedHeartbeat);
  }
  const preferredEntries = sorted.filter((entry) => !entry.noopHeartbeat);
  for (const entry of preferredEntries) {
    if (picked.length >= limit) {
      break;
    }
    if (picked.some((existing) => sameEntry(existing, entry))) {
      continue;
    }
    picked.push(entry);
  }
  return picked.sort((a, b) => a.ts - b.ts);
}

export async function loadRecentTurnCandidates(params: {
  historyRoot: string;
  statePath?: string;
  mainSessionKey?: string;
  limit?: number;
  scanLimit?: number;
}): Promise<RecentTurnWindowEntry[]> {
  const statePath = params.statePath ?? resolveStateDir(process.env);
  const scanLimit = Math.max(
    params.limit ?? DEFAULT_HEARTBEAT_RECENT_TURN_LIMIT,
    params.scanLimit ?? DEFAULT_SCAN_LIMIT,
  );
  const [driverTurns, heartbeatTurns] = await Promise.all([
    loadDriverAuditTurns({
      historyRoot: params.historyRoot,
      mainSessionKey: params.mainSessionKey,
      scanLimit,
    }),
    loadHeartbeatTurns({ statePath, scanLimit }),
  ]);
  return [...driverTurns, ...heartbeatTurns];
}

export async function loadRecentTurns(params: {
  historyRoot: string;
  statePath?: string;
  mainSessionKey?: string;
  limit?: number;
  scanLimit?: number;
}): Promise<RecentTurnWindowEntry[]> {
  return selectRecentTurns({
    entries: await loadRecentTurnCandidates(params),
    limit: params.limit,
  });
}

function formatTimestamp(ts: number): string {
  try {
    return new Date(ts).toISOString();
  } catch {
    return String(ts);
  }
}

export function renderRecentTurnsBlock(
  entries: RecentTurnWindowEntry[],
  limit: number = DEFAULT_HEARTBEAT_RECENT_TURN_LIMIT,
): string {
  if (entries.length === 0) {
    return "";
  }
  const selected = selectRecentTurns({ entries, limit });
  const lines = [
    "Recent completed turns (oldest first; includes one recent main Telegram turn when available). Keep continuity with them.",
  ];
  for (const entry of selected) {
    lines.push("");
    lines.push(`### ${formatTimestamp(entry.ts)} | ${entry.source}${entry.sessionKey ? ` | ${entry.sessionKey}` : ""}`);
    lines.push(`User: ${truncateInline(entry.userText, MAX_USER_TEXT_CHARS)}`);
    lines.push(`Assistant: ${truncateInline(entry.assistantText, MAX_ASSISTANT_TEXT_CHARS)}`);
  }
  return lines.join("\n");
}

export function renderHeartbeatTraceBlock(
  entries: HeartbeatTurnLogEntry[],
  limit: number = DEFAULT_HEARTBEAT_TRACE_LIMIT,
): string {
  if (entries.length === 0) {
    return "";
  }
  const selected = entries.slice(-Math.max(1, limit));
  const lines = ["Heartbeat trace (oldest first; compact daily continuity)."];
  for (const entry of selected) {
    const status = entry.assistantText.trim() === HEARTBEAT_TOKEN ? "ok" : "acted";
    const summary =
      status === "ok"
        ? "HEARTBEAT_OK"
        : truncateInline(entry.assistantText, 120);
    lines.push(`- ${formatTimestamp(entry.ts)} | ${status} | ${summary}`);
  }
  return lines.join("\n");
}
