import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { HEARTBEAT_TOKEN } from "../auto-reply/tokens.js";
import { resolveStateDir } from "../config/paths.js";
import { validateAffectContractFromGestalt } from "../vericore/affect.js";

const HEARTBEAT_TURN_LOG_FILENAME = "heartbeat-turns.jsonl";
const DEFAULT_SCAN_LIMIT = 64;
export const DEFAULT_ENERGY_SCAN_LIMIT = 256;
export const DEFAULT_HEARTBEAT_RECENT_TURN_LIMIT = 5;
export const DEFAULT_HEARTBEAT_TRACE_LIMIT = 32;
export const DEFAULT_ENERGY_CONVERSATION_GAP_MS = 30 * 60 * 1000;
const MAX_USER_TEXT_CHARS = 220;
const MAX_ASSISTANT_TEXT_CHARS = 320;
const AFFECT_GESTALT_FILENAME_PREFIX = "affect-gestalt-";
const AFFECT_GESTALT_VERSION = 1;

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
  affect?: AffectTrace;
};

export type EnergySignalSummary = {
  noopHeartbeatCount: number;
  actedHeartbeatCount: number;
  conversationBurstCount: number;
  conversationGapMs: number;
  energy: number;
};

export type AffectTrace = {
  Cn: number;
  C: number;
  Ct: number;
  I: number;
  J: number;
  A: number;
  S: number;
};

const AFFECT_KEYS = ["Cn", "C", "Ct", "I", "J", "A", "S"] as const;
const AFFECT_TERMINAL_RE = /^\u22c4\u27e8(.+)\u27e9$/;
const AFFECT_GAMMA = 0.69;

type AffectGestaltState = {
  version: number;
  gamma: number;
  gestalt: AffectTrace | null;
  lastTs: number | null;
  heartbeatOffset: number;
  auditOffsets: Record<string, number>;
};

type AffectDeltaResult = {
  nextOffset: number;
  lines: string[];
  truncated: boolean;
};

type AffectEntry = {
  ts: number;
  affect: AffectTrace;
  order: number;
};

type AffectGestaltReadResult = {
  exists: boolean;
  needsRewrite: boolean;
  state: AffectGestaltState;
};

let affectGestaltSyncQueue: Promise<void> = Promise.resolve();

export function parseAffectTrace(text: string): AffectTrace | undefined {
  const lines = text.split("\n");
  let terminalLine = "";
  for (let i = lines.length - 1; i >= 0; i--) {
    const trimmed = lines[i].trim();
    if (trimmed) {
      terminalLine = trimmed;
      break;
    }
  }
  if (!terminalLine) return undefined;
  const match = AFFECT_TERMINAL_RE.exec(terminalLine);
  if (!match?.[1]) return undefined;
  const parts = match[1].trim().split(/\s+/);
  if (parts.length !== AFFECT_KEYS.length) return undefined;
  const result: Record<string, number> = {};
  for (let i = 0; i < parts.length; i++) {
    const [key, valStr] = parts[i].split(":");
    if (key !== AFFECT_KEYS[i] || valStr === undefined) return undefined;
    const val = Number(valStr);
    if (!Number.isFinite(val) || val < -1 || val > 1) return undefined;
    result[key] = val;
  }
  return result as unknown as AffectTrace;
}

function createEmptyAffectGestaltState(gamma: number): AffectGestaltState {
  return {
    version: AFFECT_GESTALT_VERSION,
    gamma,
    gestalt: null,
    lastTs: null,
    heartbeatOffset: 0,
    auditOffsets: {},
  };
}

function isFiniteAffectValue(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= -1 && value <= 1;
}

function coerceAffectTrace(raw: unknown): AffectTrace | null {
  if (!raw || typeof raw !== "object") {
    return null;
  }
  const record = raw as Record<string, unknown>;
  for (const key of AFFECT_KEYS) {
    if (!isFiniteAffectValue(record[key])) {
      return null;
    }
  }
  return {
    Cn: record.Cn as number,
    C: record.C as number,
    Ct: record.Ct as number,
    I: record.I as number,
    J: record.J as number,
    A: record.A as number,
    S: record.S as number,
  };
}

function clampAffectValue(value: number): number {
  if (value > 1) return 1;
  if (value < -1) return -1;
  return value;
}

export function foldAffectGestalt(
  previous: AffectTrace | undefined,
  next: AffectTrace,
  gamma: number = AFFECT_GAMMA,
): AffectTrace {
  if (!previous) {
    return { ...next };
  }
  const alpha = 1 - gamma;
  const updated = {} as AffectTrace;
  for (const key of AFFECT_KEYS) {
    updated[key] = clampAffectValue(previous[key] * gamma + next[key] * alpha);
  }
  return updated;
}

function roundAffectTrace(trace: AffectTrace): AffectTrace {
  return {
    Cn: round1(trace.Cn),
    C: round1(trace.C),
    Ct: round1(trace.Ct),
    I: round1(trace.I),
    J: round1(trace.J),
    A: round1(trace.A),
    S: round1(trace.S),
  };
}

function serializeAffectGestaltState(state: AffectGestaltState): string {
  return JSON.stringify(state);
}

async function readAffectGestaltState(
  filePath: string,
  gamma: number,
): Promise<AffectGestaltReadResult> {
  try {
    const raw = JSON.parse(await fs.readFile(filePath, "utf-8")) as Record<string, unknown>;
    const heartbeatOffset =
      typeof raw.heartbeatOffset === "number" && raw.heartbeatOffset >= 0
        ? raw.heartbeatOffset
        : 0;
    const auditOffsetsRaw =
      raw.auditOffsets && typeof raw.auditOffsets === "object"
        ? (raw.auditOffsets as Record<string, unknown>)
        : {};
    const auditOffsets: Record<string, number> = {};
    for (const [name, value] of Object.entries(auditOffsetsRaw)) {
      if (typeof value === "number" && value >= 0) {
        auditOffsets[name] = value;
      }
    }
    if (raw.version !== AFFECT_GESTALT_VERSION || raw.gamma !== gamma) {
      return {
        exists: true,
        needsRewrite: true,
        state: createEmptyAffectGestaltState(gamma),
      };
    }
    return {
      exists: true,
      needsRewrite: false,
      state: {
        version: AFFECT_GESTALT_VERSION,
        gamma,
        gestalt: coerceAffectTrace(raw.gestalt),
        lastTs: typeof raw.lastTs === "number" ? raw.lastTs : null,
        heartbeatOffset,
        auditOffsets,
      },
    };
  } catch {
    return {
      exists: false,
      needsRewrite: false,
      state: createEmptyAffectGestaltState(gamma),
    };
  }
}

async function writeAffectGestaltState(
  filePath: string,
  state: AffectGestaltState,
): Promise<void> {
  const tmpPath = `${filePath}.tmp`;
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(tmpPath, `${serializeAffectGestaltState(state)}\n`, "utf-8");
  await fs.rename(tmpPath, filePath);
}

async function readJsonlDelta(filePath: string, startOffset: number): Promise<AffectDeltaResult> {
  let handle: Awaited<ReturnType<typeof fs.open>> | undefined;
  try {
    handle = await fs.open(filePath, "r");
    const stat = await handle.stat();
    const truncated = stat.size < startOffset;
    const safeStart = truncated ? 0 : startOffset;
    if (stat.size <= safeStart) {
      return { nextOffset: safeStart, lines: [], truncated };
    }
    const readSize = stat.size - safeStart;
    const buffer = Buffer.alloc(readSize);
    const { bytesRead } = await handle.read(buffer, 0, readSize, safeStart);
    const view = buffer.subarray(0, bytesRead);
    const lastNewline = view.lastIndexOf(0x0a);
    if (lastNewline < 0) {
      return { nextOffset: safeStart, lines: [], truncated };
    }
    const lines = view
      .subarray(0, lastNewline + 1)
      .toString("utf-8")
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean);
    return {
      nextOffset: safeStart + lastNewline + 1,
      lines,
      truncated,
    };
  } catch {
    return { nextOffset: startOffset, lines: [], truncated: false };
  } finally {
    await handle?.close();
  }
}

export function resolveAffectGestaltPath(params: {
  historyRoot: string;
  statePath?: string;
}): string {
  const statePath = params.statePath ?? resolveStateDir(process.env);
  const digest = createHash("sha1")
    .update(path.resolve(params.historyRoot))
    .digest("hex")
    .slice(0, 12);
  return path.join(statePath, "logs", `${AFFECT_GESTALT_FILENAME_PREFIX}${digest}.json`);
}

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

function isMainTelegramSource(source: string): boolean {
  return source === "telegram" || source === "telegram_dm";
}

function isConversationSource(source: string): boolean {
  return (
    source === "telegram" ||
    source === "telegram_dm" ||
    source === "telegram_family" ||
    source === "moltbook" ||
    source === "family"
  );
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
      reserveMainTelegram: isMainTelegramSource(source) && sessionKey === mainSessionKey,
      noopHeartbeat: false,
      affect: parseAffectTrace(assistantText),
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
      noopHeartbeat: assistantText === HEARTBEAT_TOKEN || entry.status === "ok-affect",
      affect: parseAffectTrace(assistantText),
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

async function collectAffectEntries(params: {
  historyRoot: string;
  statePath: string;
  state: AffectGestaltState;
  fullScan: boolean;
}): Promise<{
  entries: AffectEntry[];
  nextState: AffectGestaltState;
  needsRebuild: boolean;
}> {
  const nextState: AffectGestaltState = {
    ...params.state,
    auditOffsets: { ...params.state.auditOffsets },
  };
  const entries: AffectEntry[] = [];
  let order = 0;
  let needsRebuild = false;

  const heartbeatDelta = await readJsonlDelta(
    resolveHeartbeatTurnLogPath(params.statePath),
    params.fullScan ? 0 : params.state.heartbeatOffset,
  );
  if (!params.fullScan && heartbeatDelta.truncated) {
    needsRebuild = true;
  }
  nextState.heartbeatOffset = heartbeatDelta.nextOffset;
  for (const line of heartbeatDelta.lines) {
    const parsed = parseHeartbeatTurnLine(line);
    if (!parsed?.affect) {
      continue;
    }
    if (
      !params.fullScan &&
      typeof params.state.lastTs === "number" &&
      parsed.ts < params.state.lastTs
    ) {
      needsRebuild = true;
    }
    entries.push({ ts: parsed.ts, affect: parsed.affect, order: order++ });
  }

  const auditDir = path.join(params.historyRoot, "audit");
  let auditNames: string[] = [];
  try {
    auditNames = (await fs.readdir(auditDir))
      .filter((name) => name.endsWith(".jsonl"))
      .sort();
  } catch {
    auditNames = [];
  }

  for (const name of auditNames) {
    const delta = await readJsonlDelta(
      path.join(auditDir, name),
      params.fullScan ? 0 : params.state.auditOffsets[name] ?? 0,
    );
    if (!params.fullScan && delta.truncated) {
      needsRebuild = true;
    }
    nextState.auditOffsets[name] = delta.nextOffset;
    for (const line of delta.lines) {
      const parsed = parseDriverAuditLine(line);
      if (!parsed?.affect) {
        continue;
      }
      if (
        !params.fullScan &&
        typeof params.state.lastTs === "number" &&
        parsed.ts < params.state.lastTs
      ) {
        needsRebuild = true;
      }
      entries.push({ ts: parsed.ts, affect: parsed.affect, order: order++ });
    }
  }

  return { entries, nextState, needsRebuild };
}

async function syncAffectGestaltNow(params: {
  historyRoot: string;
  statePath: string;
  gamma: number;
}): Promise<AffectTrace | undefined> {
  const filePath = resolveAffectGestaltPath(params);
  const loaded = await readAffectGestaltState(filePath, params.gamma);
  let baseState = loaded.state;
  let collected = await collectAffectEntries({
    historyRoot: params.historyRoot,
    statePath: params.statePath,
    state: baseState,
    fullScan: false,
  });

  if (collected.needsRebuild) {
    baseState = createEmptyAffectGestaltState(params.gamma);
    collected = await collectAffectEntries({
      historyRoot: params.historyRoot,
      statePath: params.statePath,
      state: baseState,
      fullScan: true,
    });
  }

  const sorted = collected.entries.sort((a, b) => a.ts - b.ts || a.order - b.order);
  let prevGestalt: AffectTrace | undefined = baseState.gestalt ?? undefined;
  let gestalt = prevGestalt;
  let lastObs: AffectTrace | undefined;
  let lastTs = baseState.lastTs;
  for (const entry of sorted) {
    prevGestalt = gestalt;
    gestalt = foldAffectGestalt(gestalt, entry.affect, params.gamma);
    lastObs = entry.affect;
    lastTs = entry.ts;
  }

  if (lastObs && gestalt) {
    validateAffectContractFromGestalt({
      prevGestalt: prevGestalt ?? null,
      observation: lastObs,
      result: gestalt,
    })
      .then((contract) => {
        if (!contract.checked) {
          console.error("[vericore] affect validator unavailable", contract.error);
        } else if (!contract.contract_ok) {
          console.error("[vericore] affect contract failed", contract);
        }
      })
      .catch(() => {});
  }

  const nextState: AffectGestaltState = {
    version: AFFECT_GESTALT_VERSION,
    gamma: params.gamma,
    gestalt: gestalt ?? null,
    lastTs,
    heartbeatOffset: collected.nextState.heartbeatOffset,
    auditOffsets: collected.nextState.auditOffsets,
  };

  if (
    loaded.needsRewrite ||
    !loaded.exists ||
    serializeAffectGestaltState(nextState) !== serializeAffectGestaltState(loaded.state)
  ) {
    await writeAffectGestaltState(filePath, nextState);
  }

  return gestalt ? roundAffectTrace(gestalt) : undefined;
}

export async function loadAffectGestalt(params: {
  historyRoot: string;
  statePath?: string;
  gamma?: number;
}): Promise<AffectTrace | undefined> {
  const statePath = params.statePath ?? resolveStateDir(process.env);
  const gamma = params.gamma ?? AFFECT_GAMMA;
  const task = affectGestaltSyncQueue.then(() =>
    syncAffectGestaltNow({
      historyRoot: params.historyRoot,
      statePath,
      gamma,
    }),
  );
  affectGestaltSyncQueue = task.then(
    () => undefined,
    () => undefined,
  );
  try {
    return await task;
  } catch {
    return undefined;
  }
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
    const status =
      entry.assistantText.trim() === HEARTBEAT_TOKEN || entry.status === "ok-affect"
        ? "ok"
        : "acted";
    const summary =
      status === "ok"
        ? "HEARTBEAT_OK"
        : truncateInline(entry.assistantText, 120);
    lines.push(`- ${formatTimestamp(entry.ts)} | ${status} | ${summary}`);
  }
  return lines.join("\n");
}

export function computeEnergy(params: {
  entries: RecentTurnWindowEntry[];
  conversationGapMs?: number;
}): number {
  return summarizeEnergySignals(params).energy;
}

export function summarizeEnergySignals(params: {
  entries: RecentTurnWindowEntry[];
  conversationGapMs?: number;
}): EnergySignalSummary {
  const conversationGapMs = Math.max(
    0,
    params.conversationGapMs ?? DEFAULT_ENERGY_CONVERSATION_GAP_MS,
  );
  const sorted = [...params.entries].sort((a, b) => a.ts - b.ts);
  let noopHeartbeatCount = 0;
  let actedHeartbeatCount = 0;
  let conversationBurstCount = 0;
  let lastConversationTs: number | undefined;
  for (const entry of sorted) {
    if (entry.source === "heartbeat") {
      if (entry.noopHeartbeat) {
        noopHeartbeatCount += 1;
      } else {
        actedHeartbeatCount += 1;
      }
      continue;
    }
    if (!isConversationSource(entry.source)) {
      continue;
    }
    if (
      typeof lastConversationTs !== "number" ||
      entry.ts - lastConversationTs >= conversationGapMs
    ) {
      conversationBurstCount += 1;
    }
    lastConversationTs = entry.ts;
  }
  return {
    noopHeartbeatCount,
    actedHeartbeatCount,
    conversationBurstCount,
    conversationGapMs,
    energy:
      noopHeartbeatCount - actedHeartbeatCount - conversationBurstCount,
  };
}

export function renderEnergyBlock(energy: number): string {
  return `(energy ${energy})`;
}

function round1(v: number): number {
  return Math.round(v * 10) / 10;
}

function formatAffectValue(v: number): string {
  if (v === 0) return "0";
  const abs = Math.abs(v).toFixed(1).replace(/^0/, "");
  return v < 0 ? `-${abs}` : abs;
}

/**
 * Reference fold for affect-bearing entries held in memory.
 *
 * Runtime prompt injection uses the persisted gestalt loader above so we only
 * fold new completed turns, but this pure fold remains useful for tests and
 * spot-checking exact behavior against a fixed entry set.
 */
export function aggregateAffectTraces(
  entries: RecentTurnWindowEntry[],
  gamma: number = AFFECT_GAMMA,
): AffectTrace | undefined {
  const withAffect = entries
    .filter((e): e is RecentTurnWindowEntry & { affect: AffectTrace } => !!e.affect)
    .sort((a, b) => a.ts - b.ts);
  if (withAffect.length === 0) return undefined;
  let gestalt: AffectTrace | undefined;
  for (const entry of withAffect) {
    gestalt = foldAffectGestalt(gestalt, entry.affect, gamma);
  }
  return gestalt ? roundAffectTrace(gestalt) : undefined;
}

function renderAffectAggregateFromTrace(trace: AffectTrace | undefined): string {
  if (!trace) return "";
  const parts = AFFECT_KEYS.map((key) => `${key}:${formatAffectValue(trace[key])}`);
  return `Affect trace aggregate:\n\u22c4\u27e8${parts.join(" ")}\u27e9`;
}

export function renderAffectAggregateBlock(
  entries: RecentTurnWindowEntry[],
  gamma: number = AFFECT_GAMMA,
): string {
  return renderAffectAggregateFromTrace(aggregateAffectTraces(entries, gamma));
}

export async function loadAffectAggregateBlock(params: {
  historyRoot: string;
  statePath?: string;
  gamma?: number;
}): Promise<string> {
  return renderAffectAggregateFromTrace(await loadAffectGestalt(params));
}
