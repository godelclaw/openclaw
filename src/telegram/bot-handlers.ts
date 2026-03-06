import type { Message, ReactionTypeEmoji } from "@grammyjs/types";
import { resolveAgentDir, resolveDefaultAgentId } from "../agents/agent-scope.js";
import { hasControlCommand } from "../auto-reply/command-detection.js";
import {
  createInboundDebouncer,
  resolveInboundDebounceMs,
} from "../auto-reply/inbound-debounce.js";
import { buildCommandsPaginationKeyboard } from "../auto-reply/reply/commands-info.js";
import {
  buildModelsProviderData,
  formatModelsAvailableHeader,
} from "../auto-reply/reply/commands-models.js";
import { resolveStoredModelOverride } from "../auto-reply/reply/model-selection.js";
import { listSkillCommandsForAgents } from "../auto-reply/skill-commands.js";
import { buildCommandsMessagePaginated } from "../auto-reply/status.js";
import { shouldDebounceTextInbound } from "../channels/inbound-debounce-policy.js";
import { resolveChannelConfigWrites } from "../channels/plugins/config-writes.js";
import { loadConfig } from "../config/config.js";
import { writeConfigFile } from "../config/io.js";
import { loadSessionStore, resolveStorePath } from "../config/sessions.js";
import type { DmPolicy } from "../config/types.base.js";
import type {
  TelegramDirectConfig,
  TelegramGroupConfig,
  TelegramTopicConfig,
} from "../config/types.js";
import { danger, logVerbose, warn } from "../globals.js";
import { enqueueSystemEvent } from "../infra/system-events.js";
import { MediaFetchError } from "../media/fetch.js";
import { readChannelAllowFromStore } from "../pairing/pairing-store.js";
import { resolveAgentRoute } from "../routing/resolve-route.js";
import { resolveThreadSessionKeys } from "../routing/session-key.js";
import {
  runVeriCoreMemoryQuery,
  runVeriCoreMemoryRefine,
  runVeriCoreMemorySetTier,
  runVeriCoreMemoryStatus,
  runVeriCoreMindlockApprove,
  runVeriCoreMindlockList,
  runVeriCoreMindlockPending,
  runVeriCoreMindlockReject,
  runVeriCoreMindlockStatus,
  runVeriCoreStimulusRoute,
  runVeriCoreStimulusRun,
  type VeriCoreMemoryQueryResult,
  type VeriCoreMemoryRefineResult,
  type VeriCoreMemorySetTierResult,
  type VeriCoreMemoryStatus,
  type VeriCoreMindlockDecisionResult,
  type VeriCoreMindlockListResult,
  type VeriCoreMindlockPendingResult,
  type VeriCoreMindlockStatusResult,
  type VeriCoreRoutePreference,
} from "../vericore/impetus.js";
import { withTelegramApiErrorLogging } from "./api-logging.js";
import {
  isSenderAllowed,
  normalizeDmAllowFromWithStore,
  type NormalizedAllowFrom,
} from "./bot-access.js";
import type { TelegramMediaRef } from "./bot-message-context.js";
import { RegisterTelegramHandlerParams } from "./bot-native-commands.js";
import {
  MEDIA_GROUP_TIMEOUT_MS,
  type MediaGroupEntry,
  type TelegramUpdateKeyContext,
} from "./bot-updates.js";
import { resolveMedia } from "./bot/delivery.js";
import {
  buildTelegramGroupPeerId,
  buildTelegramParentPeer,
  resolveTelegramForumThreadId,
  resolveTelegramGroupAllowFromContext,
} from "./bot/helpers.js";
import type { TelegramContext } from "./bot/types.js";
import { enforceTelegramDmAccess } from "./dm-access.js";
import {
  evaluateTelegramGroupBaseAccess,
  evaluateTelegramGroupPolicyAccess,
} from "./group-access.js";
import { migrateTelegramGroupConfig } from "./group-migration.js";
import { resolveTelegramInlineButtonsScope } from "./inline-buttons.js";
import {
  buildModelsKeyboard,
  buildProviderKeyboard,
  calculateTotalPages,
  getModelsPageSize,
  parseModelCallbackData,
  resolveModelSelection,
  type ProviderInfo,
} from "./model-buttons.js";
import { buildInlineKeyboard } from "./send.js";
import { wasSentByBot } from "./sent-message-cache.js";

function isMediaSizeLimitError(err: unknown): boolean {
  const errMsg = String(err);
  return errMsg.includes("exceeds") && errMsg.includes("MB limit");
}

function isRecoverableMediaGroupError(err: unknown): boolean {
  return err instanceof MediaFetchError || isMediaSizeLimitError(err);
}

function resolveMinReplyIntervalSeconds(
  groupConfig?: TelegramGroupConfig | TelegramDirectConfig,
): number {
  if (!groupConfig || !("minReplyIntervalSeconds" in groupConfig)) {
    return 0;
  }
  return groupConfig.minReplyIntervalSeconds ?? 0;
}

function isMemoryStatusCommand(command?: string | null): boolean {
  if (!command) {
    return false;
  }
  const normalized = command.trim().toLowerCase();
  return (
    normalized === "memory-status" ||
    normalized === "memory_status" ||
    normalized === "memorystatus"
  );
}

function isMemoryQueryCommand(command?: string | null): boolean {
  if (!command) {
    return false;
  }
  const normalized = command.trim().toLowerCase();
  return (
    normalized === "memory-query" ||
    normalized === "memory_query" ||
    normalized === "memoryquery"
  );
}

function isMemoryRefineCommand(command?: string | null): boolean {
  if (!command) {
    return false;
  }
  const normalized = command.trim().toLowerCase();
  return (
    normalized === "memory-refine" ||
    normalized === "memory_refine" ||
    normalized === "memoryrefine"
  );
}

function parseMemoryQueryText(text?: string): string {
  const body = (text ?? "").trim();
  if (!body.startsWith("/")) {
    return body;
  }
  const firstSpace = body.search(/\s/);
  if (firstSpace < 0) {
    return "";
  }
  return body.slice(firstSpace + 1).trim();
}

function parseMemoryRefineEmbedFlag(text?: string): boolean {
  const body = parseMemoryQueryText(text);
  if (!body) {
    return false;
  }
  return /(^|\s)(--embed|embed)(\s|$)/i.test(body);
}

function isMemoryPromoteCommand(command?: string | null): boolean {
  if (!command) {
    return false;
  }
  const normalized = command.trim().toLowerCase();
  return (
    normalized === "memory-promote" ||
    normalized === "memory_promote" ||
    normalized === "memorypromote"
  );
}

function isMindlockCommand(command?: string | null): boolean {
  if (!command) {
    return false;
  }
  const normalized = command.trim().toLowerCase();
  return normalized === "mindlock";
}

function isMindlockReviewCommand(command?: string | null): boolean {
  if (!command) {
    return false;
  }
  const normalized = command.trim().toLowerCase();
  return normalized === "review";
}

function isMindlockApproveCommand(command?: string | null): boolean {
  if (!command) {
    return false;
  }
  const normalized = command.trim().toLowerCase();
  return normalized === "a";
}

function isMindlockRejectCommand(command?: string | null): boolean {
  if (!command) {
    return false;
  }
  const normalized = command.trim().toLowerCase();
  return normalized === "reject" || normalized === "r";
}

function parseMindlockArtifactArgs(text?: string): {
  artifactId?: string;
  reason?: string;
  error?: string;
} {
  const body = parseMemoryQueryText(text);
  const trimmed = body.trim();
  if (!trimmed) {
    return { error: "Missing artifact id." };
  }

  const firstSpace = trimmed.search(/\s/);
  if (firstSpace < 0) {
    return { artifactId: trimmed };
  }

  const artifactId = trimmed.slice(0, firstSpace).trim();
  const reason = trimmed.slice(firstSpace + 1).trim();
  return { artifactId, reason: reason || undefined };
}

function parseMindlockMenuArgs(text?: string): {
  box?: "in" | "out" | "pending" | "rejected";
  limit?: number;
  error?: string;
} {
  const body = parseMemoryQueryText(text).trim();
  if (!body) {
    return {};
  }

  const parts = body.split(/\s+/).filter(Boolean);
  const first = (parts[0] ?? "").toLowerCase();
  const normalized = first === "pending-zar" ? "pending" : first;

  if (!["in", "out", "pending", "rejected"].includes(normalized)) {
    return { error: "Usage: /mindlock [in|out|pending|rejected] [limit]" };
  }

  let limit: number | undefined;
  if (parts.length >= 2) {
    const parsed = Number(parts[1]);
    if (!Number.isInteger(parsed) || parsed <= 0) {
      return { error: "Limit must be a positive integer." };
    }
    limit = parsed;
  }

  return { box: normalized as "in" | "out" | "pending" | "rejected", limit };
}

function normalizeMemoryTierToken(raw: string): "public" | "family" | "private" | "top_secret" | undefined {
  const normalized = raw.trim().toLowerCase();
  if (normalized === "public") {
    return "public";
  }
  if (normalized === "family") {
    return "family";
  }
  if (normalized === "private") {
    return "private";
  }
  if (normalized === "top_secret" || normalized === "top-secret" || normalized === "topsecret") {
    return "top_secret";
  }
  return undefined;
}

function parseMemoryPromoteArgs(text?: string): {
  memoryId?: number;
  tier?: "public" | "family" | "private" | "top_secret";
  operatorApproved: boolean;
  error?: string;
} {
  const body = parseMemoryQueryText(text);
  const parts = body.split(/\s+/).filter(Boolean);
  const usage = "Usage: /memory-promote <id> <public|family|private|top_secret> [--approve]";
  if (parts.length < 2) {
    return { operatorApproved: false, error: usage };
  }

  const memoryId = Number(parts[0]);
  if (!Number.isInteger(memoryId) || memoryId <= 0) {
    return { operatorApproved: false, error: "Invalid memory id: " + parts[0] + ". " + usage };
  }

  const tier = normalizeMemoryTierToken(parts[1]);
  if (!tier) {
    return { operatorApproved: false, error: "Invalid tier: " + parts[1] + ". " + usage };
  }

  const operatorApproved = parts.some((part) => {
    const token = part.trim().toLowerCase();
    return token === "--approve" || token === "--operator-approved" || token === "approve";
  });

  return { memoryId, tier, operatorApproved };
}

function formatMemorySetTierMessage(result: VeriCoreMemorySetTierResult): string {
  const changed = result.changed ? "yes" : "no (already set)";
  return [
    "VeriCore memory tier updated:",
    "- id: " + result.id,
    "- tier: " + result.tier,
    "- changed: " + changed,
    "- operator approved: " + (result.operator_approved ? "yes" : "no"),
  ].join("\n");
}

function formatMemoryStatusMessage(status: VeriCoreMemoryStatus): string {
  const byTier = Object.entries(status.memory.by_tier)
    .map(([tier, count]) => `${tier}: ${count}`)
    .join(", ");
  const byType = Object.entries(status.memory.by_type)
    .map(([kind, count]) => `${kind}: ${count}`)
    .join(", ");

  const missingEmbeddings = status.memory.missing_embeddings ?? 0;
  const missingSourceDates = status.memory.missing_source_dates ?? 0;

  return [
    "VeriCore memory status:",
    `- items: ${status.memory.total_items}`,
    `- by tier: ${byTier || "(none)"}`,
    `- by type: ${byType || "(none)"}`,
    `- missing embeddings: ${missingEmbeddings}`,
    `- missing source dates: ${missingSourceDates}`,
    `- memory file: ${status.memory.file}`,
    `- history root: ${status.history.root}`,
    `- history daily/week/merged: ${status.history.daily_files}/${status.history.weekly_files}/${status.history.daily_merged_files}`,
  ].join("\n");
}

function formatMemoryQueryMessage(result: VeriCoreMemoryQueryResult): string {
  if (!result.hits.length) {
    return `No memory hits for: "${result.query}"`;
  }

  const lines = result.hits.slice(0, 8).map((hit, index) => {
    const datePart = hit.source_date ? ` | ${hit.source_date}` : "";
    const categories = hit.categories?.length ? ` [${hit.categories.slice(0, 3).join(", ")}]` : "";
    return `${index + 1}. (${hit.score.toFixed(2)}) ${hit.summary}${categories} (${hit.type}${datePart})`;
  });

  return [
    `Memory query: "${result.query}"`,
    `- tier: ${result.tier}`,
    `- hits: ${result.hits.length}`,
    ...lines,
  ].join("\n");
}

function formatMemoryRefineMessage(result: VeriCoreMemoryRefineResult): string {
  return [
    "VeriCore memory refine complete:",
    `- items before/after: ${result.refine.before_items} -> ${result.refine.after_items}`,
    `- removed (dedupe): ${result.refine.removed_items}`,
    `- merged groups: ${result.refine.merged_groups}`,
    `- source dates filled: ${result.refine.source_dates_filled}`,
    `- embedded items: ${result.refine.embedded_items} (requested: ${result.refine.embed_requested ? "yes" : "no"})`,
    `- total items now: ${result.memory.total_items}`,
  ].join("\n");
}

function formatMindlockPendingMessage(result: VeriCoreMindlockPendingResult): string {
  if (!result.items.length) {
    return "Mindlock pending review queue is empty.";
  }

  const lines = result.items.slice(0, 10).flatMap((item, index) => {
    const target = item.target_path ? ` -> ${item.target_path}` : "";
    const main = `${index + 1}. id=${item.id}${target} (${item.size_bytes} bytes)`;
    if (item.reviewer_assessment?.status === "pending") {
      return [main, `   \u{1F50D} Reviewer: pending`];
    } else if (item.reviewer_assessment?.verdict && item.reviewer_assessment?.reason) {
      return [main, `   \u{1F50D} Reviewer: ${item.reviewer_assessment.verdict} \u2014 ${item.reviewer_assessment.reason}`];
    }
    return [main];
  });

  const more = result.items.length > 10 ? `...and ${result.items.length - 10} more` : "";

  const asOf =
    typeof result.as_of_ts === "number"
      ? `As of: ${new Date(result.as_of_ts * 1000).toISOString()}`
      : undefined;

  return [
    `Mindlock pending review (${result.count}):`,
    asOf,
    ...lines,
    more,
    "Approve: /a <id|index> [reason]",
    "Reject: /reject <id|index> [reason] (alias: /r)",
    "Note: /approve is the legacy exec-approval command.",
  ]
    .filter((line): line is string => typeof line === "string" && line.trim().length > 0)
    .join("\n");
}

function formatMindlockStatusMessage(result: VeriCoreMindlockStatusResult): string {
  const asOf =
    typeof result.as_of_ts === "number"
      ? `As of: ${new Date(result.as_of_ts * 1000).toISOString()}`
      : undefined;

  return [
    "Mindlock status:",
    asOf,
    `- in: ${result.counts.in}`,
    `- out: ${result.counts.out}`,
    `- pending: ${result.counts.pending}`,
    `- rejected: ${result.counts.rejected}`,
    "",
    "Views: /mindlock in|out|pending|rejected [limit]",
    "Approve: /a <id|index> [reason]",
    "Reject: /reject <id|index> [reason] (alias: /r)",
  ]
    .filter((line) => line !== undefined && line.trim().length > 0)
    .join("\n");
}

function formatMindlockListMessage(result: VeriCoreMindlockListResult): string {
  if (!result.items.length) {
    return `Mindlock ${result.box} is empty.`;
  }

  const lines = result.items.slice(0, 20).flatMap((item, index) => {
    const target = item.target_path ? ` -> ${item.target_path}` : "";
    const main = `${index + 1}. id=${item.id}${target} (${item.size_bytes} bytes)`;
    if (item.reviewer_assessment?.status === "pending") {
      return [main, `   \u{1F50D} Reviewer: pending`];
    } else if (item.reviewer_assessment?.verdict && item.reviewer_assessment?.reason) {
      return [main, `   \u{1F50D} Reviewer: ${item.reviewer_assessment.verdict} \u2014 ${item.reviewer_assessment.reason}`];
    }
    return [main];
  });

  const asOf =
    typeof result.as_of_ts === "number"
      ? `As of: ${new Date(result.as_of_ts * 1000).toISOString()}`
      : undefined;

  return [
    `Mindlock ${result.box} (${result.count}):`,
    asOf,
    ...lines,
  ]
    .filter((line) => line !== undefined && line.trim().length > 0)
    .join("\n");
}

function formatMindlockDecisionMessage(result: VeriCoreMindlockDecisionResult): string {
  const lines = [
    `Mindlock ${result.status}: ${result.id}`,
    `- source: ${result.source}`,
  ];

  if (result.target) {
    lines.push(`- target: ${result.target}`);
  }
  if (result.archived_artifact) {
    lines.push(`- archive: ${result.archived_artifact}`);
  }
  if (result.rejected_artifact) {
    lines.push(`- rejected: ${result.rejected_artifact}`);
  }

  return lines.join("\n");
}

function hasInboundMedia(msg: Message): boolean {
  return (
    Boolean(msg.media_group_id) ||
    (Array.isArray(msg.photo) && msg.photo.length > 0) ||
    Boolean(msg.video ?? msg.video_note ?? msg.document ?? msg.audio ?? msg.voice ?? msg.sticker)
  );
}

function hasReplyTargetMedia(msg: Message): boolean {
  const externalReply = (msg as Message & { external_reply?: Message }).external_reply;
  const replyTarget = msg.reply_to_message ?? externalReply;
  return Boolean(replyTarget && hasInboundMedia(replyTarget));
}

function extractVeriCoreContentFromMessage(msg: Message): string {
  const parts: string[] = [];
  const text = (msg.text ?? msg.caption ?? "").trim();
  if (text) {
    parts.push(text);
  }

  const replyTarget = msg.reply_to_message;
  const replyText = (replyTarget?.text ?? replyTarget?.caption ?? "").trim();
  if (replyText) {
    parts.push(`[Reply context]\n${replyText}`);
  }

  if (!text) {
    if (msg.voice) {
      parts.push("<media:voice>");
    } else if (msg.audio) {
      parts.push("<media:audio>");
    } else if (msg.video_note) {
      parts.push("<media:video_note>");
    } else if (msg.video) {
      parts.push("<media:video>");
    } else if (msg.photo?.length) {
      parts.push("<media:photo>");
    } else if (msg.document) {
      parts.push("<media:document>");
    } else if (msg.sticker) {
      parts.push("<media:sticker>");
    }
  }

  if (msg.location) {
    parts.push(`[Location] lat=${msg.location.latitude}, lon=${msg.location.longitude}`);
  }

  return parts.join("\n\n").trim();
}

function resolveInboundMediaFileId(msg: Message): string | undefined {
  return (
    msg.sticker?.file_id ??
    msg.photo?.[msg.photo.length - 1]?.file_id ??
    msg.video?.file_id ??
    msg.video_note?.file_id ??
    msg.document?.file_id ??
    msg.audio?.file_id ??
    msg.voice?.file_id
  );
}

export const registerTelegramHandlers = ({
  cfg,
  accountId,
  bot,
  opts,
  runtime,
  mediaMaxBytes,
  telegramCfg,
  allowFrom,
  groupAllowFrom,
  resolveGroupPolicy,
  resolveTelegramGroupConfig,
  shouldSkipUpdate,
  processMessage,
  logger,
}: RegisterTelegramHandlerParams) => {
  const DEFAULT_TEXT_FRAGMENT_MAX_GAP_MS = 1500;
  const TELEGRAM_TEXT_FRAGMENT_START_THRESHOLD_CHARS = 4000;
  const TELEGRAM_TEXT_FRAGMENT_MAX_GAP_MS =
    typeof opts.testTimings?.textFragmentGapMs === "number" &&
    Number.isFinite(opts.testTimings.textFragmentGapMs)
      ? Math.max(10, Math.floor(opts.testTimings.textFragmentGapMs))
      : DEFAULT_TEXT_FRAGMENT_MAX_GAP_MS;
  const TELEGRAM_TEXT_FRAGMENT_MAX_ID_GAP = 1;
  const TELEGRAM_TEXT_FRAGMENT_MAX_PARTS = 12;
  const TELEGRAM_TEXT_FRAGMENT_MAX_TOTAL_CHARS = 50_000;
  const mediaGroupTimeoutMs =
    typeof opts.testTimings?.mediaGroupFlushMs === "number" &&
    Number.isFinite(opts.testTimings.mediaGroupFlushMs)
      ? Math.max(10, Math.floor(opts.testTimings.mediaGroupFlushMs))
      : MEDIA_GROUP_TIMEOUT_MS;

  const mediaGroupBuffer = new Map<string, MediaGroupEntry>();
  let mediaGroupProcessing: Promise<void> = Promise.resolve();

  type TextFragmentEntry = {
    key: string;
    messages: Array<{ msg: Message; ctx: TelegramContext; receivedAtMs: number }>;
    timer: ReturnType<typeof setTimeout>;
  };
  const textFragmentBuffer = new Map<string, TextFragmentEntry>();
  let textFragmentProcessing: Promise<void> = Promise.resolve();

  const lastGroupReplyAtMs = new Map<string, number>();

  const debounceMs = resolveInboundDebounceMs({ cfg, channel: "telegram" });
  const FORWARD_BURST_DEBOUNCE_MS = 80;
  type TelegramDebounceLane = "default" | "forward";
  type TelegramDebounceEntry = {
    ctx: TelegramContext;
    msg: Message;
    allMedia: TelegramMediaRef[];
    storeAllowFrom: string[];
    debounceKey: string | null;
    debounceLane: TelegramDebounceLane;
    botUsername?: string;
  };

  const resolveTelegramDebounceLane = (msg: Message): TelegramDebounceLane => {
    const forwardMeta = msg as {
      forward_origin?: unknown;
      forward_from?: unknown;
      forward_from_chat?: unknown;
      forward_sender_name?: unknown;
      forward_date?: unknown;
    };
    return (forwardMeta.forward_origin ??
      forwardMeta.forward_from ??
      forwardMeta.forward_from_chat ??
      forwardMeta.forward_sender_name ??
      forwardMeta.forward_date)
      ? "forward"
      : "default";
  };
  const buildSyntheticTextMessage = (params: {
    base: Message;
    text: string;
    date?: number;
    from?: Message["from"];
  }): Message => ({
    ...params.base,
    ...(params.from ? { from: params.from } : {}),
    text: params.text,
    caption: undefined,
    caption_entities: undefined,
    entities: undefined,
    ...(params.date != null ? { date: params.date } : {}),
  });
  const buildSyntheticContext = (
    ctx: Pick<TelegramContext, "me"> & { getFile?: unknown },
    message: Message,
  ): TelegramContext => {
    const getFile =
      typeof ctx.getFile === "function"
        ? (ctx.getFile as TelegramContext["getFile"]).bind(ctx as object)
        : async () => ({});
    return { message, me: ctx.me, getFile };
  };

  type VeriCoreMode = "off" | "gate" | "driver";
  const resolveVeriCoreMode = (): VeriCoreMode => {
    const raw = (process.env.VERICORE_MODE ?? "off").trim().toLowerCase();
    if (raw === "gate" || raw === "driver") {
      return raw;
    }
    return "off";
  };
  const vericoreMode = resolveVeriCoreMode();
  const vericoreLogEnabled = (() => {
    const raw = (process.env.VERICORE_LOG ?? "").trim().toLowerCase();
    return raw === "1" || raw === "true" || raw === "yes" || raw === "on";
  })();

  const resolveMindlockOwnerTelegramIds = (): Set<string> => {
    const ids = new Set<string>();

    const envId = (process.env.ZAR_TELEGRAM_ID ?? "").trim();
    if (/^\d+$/.test(envId)) {
      ids.add(envId);
    }

    const configuredOwners = cfg.commands?.ownerAllowFrom ?? [];
    for (const raw of configuredOwners) {
      const token = String(raw ?? "").trim();
      if (!token) {
        continue;
      }

      if (/^\d+$/.test(token)) {
        ids.add(token);
        continue;
      }

      const lower = token.toLowerCase();
      if (lower.startsWith("telegram:") || lower.startsWith("tg:")) {
        const value = token.slice(token.indexOf(":") + 1).trim();
        if (/^\d+$/.test(value)) {
          ids.add(value);
        }
      }
    }

    if (!ids.size) {
      ids.add("181832275");
    }

    return ids;
  };

  const mindlockOwnerTelegramIds = resolveMindlockOwnerTelegramIds();

  const isMindlockOwnerSender = (msg: Message): boolean => {
    const senderId = msg.from?.id != null ? String(msg.from.id) : "";
    return senderId.length > 0 && mindlockOwnerTelegramIds.has(senderId);
  };

  const resolveVeriCoreChannelForMessage = (msg: Message): string => {
    const isPrivate = msg.chat.type === "private";
    if (isPrivate) {
      return "telegram_dm";
    }

    const isGroupish =
      msg.chat.type === "group" || msg.chat.type === "supergroup" || msg.chat.type === "channel";
    if (!isGroupish) {
      return "telegram_public";
    }

    const messageThreadId = (msg as { message_thread_id?: number }).message_thread_id;
    const isForum = (msg.chat as { is_forum?: boolean }).is_forum === true;
    const resolvedThreadId = resolveTelegramForumThreadId({
      isForum,
      messageThreadId,
    });
    const peerId = buildTelegramGroupPeerId(msg.chat.id, resolvedThreadId);
    const parentPeer = buildTelegramParentPeer({
      isGroup: true,
      resolvedThreadId,
      chatId: msg.chat.id,
    });
    const route = resolveAgentRoute({
      cfg,
      channel: "telegram",
      accountId,
      peer: {
        kind: "group",
        id: peerId,
      },
      parentPeer,
    });
    return route.agentId.toLowerCase().includes("family") ? "telegram_family" : "telegram_public";
  };

  const resolveVeriCoreRoutePreference = (): VeriCoreRoutePreference => {
    if (vericoreMode === "driver") {
      return "driver";
    }
    if (vericoreMode === "gate") {
      return "gate";
    }
    return "off";
  };

  const resolveVeriCoreSessionKeyForMessage = (msg: Message): string => {
    const isGroup =
      msg.chat.type === "group" || msg.chat.type === "supergroup" || msg.chat.type === "channel";
    const messageThreadId = (msg as { message_thread_id?: number }).message_thread_id;
    const isForum = (msg.chat as { is_forum?: boolean }).is_forum === true;
    const resolvedThreadId = resolveTelegramForumThreadId({
      isForum,
      messageThreadId,
    });

    const peerId = isGroup
      ? buildTelegramGroupPeerId(msg.chat.id, resolvedThreadId)
      : String(msg.chat.id);
    const parentPeer = buildTelegramParentPeer({
      isGroup,
      resolvedThreadId,
      chatId: msg.chat.id,
    });
    const route = resolveAgentRoute({
      cfg,
      channel: "telegram",
      accountId,
      peer: {
        kind: isGroup ? "group" : "direct",
        id: peerId,
      },
      parentPeer,
    });

    const baseSessionKey = route.sessionKey;
    const dmThreadId =
      !isGroup && typeof messageThreadId === "number" ? String(messageThreadId) : undefined;
    const threadKeys =
      dmThreadId != null
        ? resolveThreadSessionKeys({ baseSessionKey, threadId: dmThreadId })
        : null;
    return threadKeys?.sessionKey ?? baseSessionKey;
  };

  const buildVeriCoreStimulusForMessage = (msg: Message) => ({
    channel: resolveVeriCoreChannelForMessage(msg),
    actor: msg.from?.id ? String(msg.from.id) : (msg.from?.username ?? "unknown"),
    content: extractVeriCoreContentFromMessage(msg),
    timestamp: typeof msg.date === "number" ? msg.date : Math.floor(Date.now() / 1000),
    session_key: resolveVeriCoreSessionKeyForMessage(msg),
    route_preference: resolveVeriCoreRoutePreference(),
  });

  const sendVeriCoreDriverResponse = async (msg: Message, responseText: string): Promise<void> => {
    const messageThreadId = (msg as { message_thread_id?: number }).message_thread_id;
    await withTelegramApiErrorLogging({
      operation: "sendMessage",
      runtime,
      fn: () =>
        bot.api.sendMessage(
          msg.chat.id,
          responseText,
          typeof messageThreadId === "number" ? { message_thread_id: messageThreadId } : undefined,
        ),
    });
  };

  const handleIngressWithVeriCore = async (params: {
    msg: Message;
    onFallback: () => Promise<void>;
  }): Promise<void> => {
    if (vericoreMode === "off") {
      await params.onFallback();
      return;
    }

    const stimulusInput = buildVeriCoreStimulusForMessage(params.msg);

    // If Telegram message content is effectively empty, fall back to OpenClaw's richer
    // inbound context pipeline instead of sending metadata-only stimuli to VeriCore.
    if (!stimulusInput.content.trim()) {
      await params.onFallback();
      return;
    }

    // Fast-path: /models -- show provider picker directly without LLM turn.
    {
      const text = (params.msg.text ?? params.msg.caption ?? "").trim();
      if (/^\/models(?:\s|$|@)/i.test(text)) {
        try {
          const modelData = await buildModelsProviderData(cfg);
          const { byProvider, providers } = modelData;
          if (providers.length === 0) {
            await sendVeriCoreDriverResponse(params.msg, "No model providers available.");
            return;
          }
          const providerInfos: ProviderInfo[] = providers.map((p) => ({
            id: p,
            count: byProvider.get(p)?.size ?? 0,
          }));
          const buttons = buildProviderKeyboard(providerInfos);
          const keyboard = buildInlineKeyboard(buttons);
          const messageThreadId = (params.msg as { message_thread_id?: number }).message_thread_id;
          await withTelegramApiErrorLogging({
            operation: "sendMessage",
            runtime,
            fn: () =>
              bot.api.sendMessage(params.msg.chat.id, "Select a provider:", {
                ...(typeof messageThreadId === "number" ? { message_thread_id: messageThreadId } : {}),
                ...(keyboard ? { reply_markup: keyboard } : {}),
              }),
          });
        } catch (err) {
          runtime.error?.(warn(`/models fast-path error: ${String(err)}`));
          await sendVeriCoreDriverResponse(params.msg, "Failed to load models.");
        }
        return;
      }
    }

    let route;
    try {
      route = await runVeriCoreStimulusRoute(stimulusInput);
    } catch (err) {
      runtime.error?.(warn(`vericore route error (failing open): ${String(err)}`));
      await params.onFallback();
      return;
    }

    logVerbose(
      `[vericore] route channel=${route.channel} context=${route.context} route=${route.route} allow=${route.allow}`,
    );

    if (vericoreLogEnabled) {
      logger.info(
        {
          mode: vericoreMode,
          allow: route.allow,
          route: route.route,
          channel: route.channel,
          context: route.context,
          reason: route.reason ?? null,
          command: route.command ?? null,
          isControl: route.is_control,
          sessionKey: route.session_key,
          chatId: params.msg.chat.id,
          messageId: params.msg.message_id,
        },
        "vericore ingress route",
      );
    }

    if (!route.allow) {
      logVerbose(`[vericore] route denied: ${route.reason ?? "unspecified"}`);
      return;
    }

    if (route.route === "control") {
      if (isMemoryQueryCommand(route.command)) {
        try {
          const query = parseMemoryQueryText(params.msg.text ?? params.msg.caption ?? "");
          if (!query) {
            await sendVeriCoreDriverResponse(
              params.msg,
              "Usage: /memory-query <your query text>",
            );
            return;
          }

          const result = await runVeriCoreMemoryQuery(query, stimulusInput);
          await sendVeriCoreDriverResponse(params.msg, formatMemoryQueryMessage(result));
          return;
        } catch (err) {
          runtime.error?.(warn(`vericore memory_query error (falling back): ${String(err)}`));
          await params.onFallback();
          return;
        }
      }

      if (isMemoryStatusCommand(route.command)) {
        try {
          const status = await runVeriCoreMemoryStatus();
          await sendVeriCoreDriverResponse(params.msg, formatMemoryStatusMessage(status));
          return;
        } catch (err) {
          runtime.error?.(warn(`vericore memory_status error (falling back): ${String(err)}`));
          await params.onFallback();
          return;
        }
      }

      if (isMemoryRefineCommand(route.command)) {
        try {
          const embed = parseMemoryRefineEmbedFlag(params.msg.text ?? params.msg.caption ?? "");
          const result = await runVeriCoreMemoryRefine(embed);
          await sendVeriCoreDriverResponse(params.msg, formatMemoryRefineMessage(result));
          return;
        } catch (err) {
          runtime.error?.(warn(`vericore memory_refine error (falling back): ${String(err)}`));
          await params.onFallback();
          return;
        }
      }

      if (isMemoryPromoteCommand(route.command)) {
        try {
          const parsed = parseMemoryPromoteArgs(params.msg.text ?? params.msg.caption ?? "");
          if (parsed.error || typeof parsed.memoryId === "undefined" || !parsed.tier) {
            await sendVeriCoreDriverResponse(
              params.msg,
              parsed.error ??
                "Usage: /memory-promote <id> <public|family|private|top_secret> [--approve]",
            );
            return;
          }

          const result = await runVeriCoreMemorySetTier(
            parsed.memoryId,
            parsed.tier,
            stimulusInput,
            parsed.operatorApproved,
          );
          await sendVeriCoreDriverResponse(params.msg, formatMemorySetTierMessage(result));
          return;
        } catch (err) {
          const message = "memory-promote failed: " + String(err);
          runtime.error?.(warn(message));
          await sendVeriCoreDriverResponse(params.msg, message);
          return;
        }
      }

      if (
        isMindlockCommand(route.command) ||
        isMindlockReviewCommand(route.command) ||
        isMindlockApproveCommand(route.command) ||
        isMindlockRejectCommand(route.command)
      ) {
        if (!isMindlockOwnerSender(params.msg)) {
          await sendVeriCoreDriverResponse(
            params.msg,
            "Mindlock review commands are owner-only.",
          );
          return;
        }

        if (isMindlockCommand(route.command)) {
          const parsed = parseMindlockMenuArgs(params.msg.text ?? params.msg.caption ?? "");
          if (parsed.error) {
            await sendVeriCoreDriverResponse(params.msg, parsed.error);
            return;
          }

          try {
            if (!parsed.box) {
              const status = await runVeriCoreMindlockStatus(stimulusInput);
              await sendVeriCoreDriverResponse(params.msg, formatMindlockStatusMessage(status));
              return;
            }

            const listing = await runVeriCoreMindlockList(
              parsed.box,
              stimulusInput,
              parsed.limit,
            );
            await sendVeriCoreDriverResponse(params.msg, formatMindlockListMessage(listing));
            return;
          } catch (err) {
            const message = "mindlock command failed: " + String(err);
            runtime.error?.(warn(message));
            await sendVeriCoreDriverResponse(params.msg, message);
            return;
          }
        }

        if (isMindlockReviewCommand(route.command)) {
          try {
            const result = await runVeriCoreMindlockPending(stimulusInput);
            await sendVeriCoreDriverResponse(params.msg, formatMindlockPendingMessage(result));
            return;
          } catch (err) {
            const message = "mindlock review failed: " + String(err);
            runtime.error?.(warn(message));
            await sendVeriCoreDriverResponse(params.msg, message);
            return;
          }
        }

        if (isMindlockApproveCommand(route.command)) {
          const parsed = parseMindlockArtifactArgs(params.msg.text ?? params.msg.caption ?? "");
          if (!parsed.artifactId) {
            await sendVeriCoreDriverResponse(
              params.msg,
              "Usage: /a <artifact_id|index> [reason]",
            );
            return;
          }

          try {
            const result = await runVeriCoreMindlockApprove(
              parsed.artifactId,
              stimulusInput,
              parsed.reason,
            );
            await sendVeriCoreDriverResponse(params.msg, formatMindlockDecisionMessage(result));
            return;
          } catch (err) {
            const message = "mindlock approve failed: " + String(err);
            runtime.error?.(warn(message));
            await sendVeriCoreDriverResponse(params.msg, message);
            return;
          }
        }

        if (isMindlockRejectCommand(route.command)) {
          const parsed = parseMindlockArtifactArgs(params.msg.text ?? params.msg.caption ?? "");
          if (!parsed.artifactId) {
            await sendVeriCoreDriverResponse(
              params.msg,
              "Usage: /reject <artifact_id|index> [reason] (alias: /r)",
            );
            return;
          }

          try {
            const result = await runVeriCoreMindlockReject(
              parsed.artifactId,
              stimulusInput,
              parsed.reason,
            );
            await sendVeriCoreDriverResponse(params.msg, formatMindlockDecisionMessage(result));
            return;
          } catch (err) {
            const message = "mindlock reject failed: " + String(err);
            runtime.error?.(warn(message));
            await sendVeriCoreDriverResponse(params.msg, message);
            return;
          }
        }
      }

      await params.onFallback();
      return;
    }

    if (route.route === "fallback" || vericoreMode !== "driver") {
      await params.onFallback();
      return;
    }

    try {
      const result = await runVeriCoreStimulusRun({
        ...stimulusInput,
        session_key: route.session_key,
        route_preference: "driver",
      });
      const decision = result.decision;

      if (!decision.allow) {
        logVerbose(`[vericore] driver denied: ${decision.reason ?? "unspecified"}`);
        if (vericoreLogEnabled) {
          logger.info(
            {
              mode: "driver",
              allow: false,
              channel: decision.channel,
              context: decision.context,
              reason: decision.reason ?? null,
              chatId: params.msg.chat.id,
              messageId: params.msg.message_id,
              sessionKey: route.session_key,
            },
            "vericore driver denied message",
          );
        }
        return;
      }

      const responseText = result.outcome?.response?.trim();
      if (!responseText) {
        logVerbose("[vericore] driver produced empty response; dropping message.");
        if (vericoreLogEnabled) {
          logger.warn(
            {
              mode: "driver",
              channel: decision.channel,
              context: decision.context,
              toolsUsed: result.outcome?.tools_used ?? [],
              promptTokens: result.outcome?.prompt_tokens ?? null,
              completionTokens: result.outcome?.completion_tokens ?? null,
              chatId: params.msg.chat.id,
              messageId: params.msg.message_id,
              sessionKey: route.session_key,
            },
            "vericore driver produced empty response",
          );
        }
        return;
      }

      if (vericoreLogEnabled) {
        logger.info(
          {
            mode: "driver",
            allow: true,
            channel: decision.channel,
            context: decision.context,
            toolsUsed: result.outcome?.tools_used ?? [],
            promptTokens: result.outcome?.prompt_tokens ?? null,
            completionTokens: result.outcome?.completion_tokens ?? null,
            responseChars: responseText.length,
            chatId: params.msg.chat.id,
            messageId: params.msg.message_id,
            sessionKey: route.session_key,
          },
          "vericore driver response ready",
        );
      }

      await sendVeriCoreDriverResponse(params.msg, responseText);
    } catch (err) {
      if (vericoreLogEnabled) {
        logger.warn(
          {
            mode: "driver",
            chatId: params.msg.chat.id,
            messageId: params.msg.message_id,
            error: String(err),
          },
          "vericore driver error; falling back to OpenClaw",
        );
      }
      runtime.error?.(warn(`vericore driver error (falling back to OpenClaw): ${String(err)}`));
      await params.onFallback();
    }
  };


  const inboundDebouncer = createInboundDebouncer<TelegramDebounceEntry>({
    debounceMs,
    resolveDebounceMs: (entry) =>
      entry.debounceLane === "forward" ? FORWARD_BURST_DEBOUNCE_MS : debounceMs,
    buildKey: (entry) => entry.debounceKey,
    shouldDebounce: (entry) => {
      const text = entry.msg.text ?? entry.msg.caption ?? "";
      const hasDebounceableText = shouldDebounceTextInbound({
        text,
        cfg,
        commandOptions: { botUsername: entry.botUsername },
      });
      if (entry.debounceLane === "forward") {
        // Forwarded bursts often split text + media into adjacent updates.
        // Debounce media-only forward entries too so they can coalesce.
        return hasDebounceableText || entry.allMedia.length > 0;
      }
      if (!hasDebounceableText) {
        return false;
      }
      return entry.allMedia.length === 0;
    },
    onFlush: async (entries) => {
      const last = entries.at(-1);
      if (!last) {
        return;
      }
      if (entries.length === 1) {

        const replyMedia = await resolveReplyMediaForMessage(last.ctx, last.msg);
        await handleIngressWithVeriCore({
          msg: last.msg,
          onFallback: async () => {
            await processMessage(last.ctx, last.allMedia, last.storeAllowFrom, undefined, replyMedia);
          },
        });

        return;
      }
      const combinedText = entries
        .map((entry) => entry.msg.text ?? entry.msg.caption ?? "")
        .filter(Boolean)
        .join("\n");
      const combinedMedia = entries.flatMap((entry) => entry.allMedia);
      if (!combinedText.trim() && combinedMedia.length === 0) {
        return;
      }
      const first = entries[0];
      const baseCtx = first.ctx;
      const syntheticMessage = buildSyntheticTextMessage({
        base: first.msg,
        text: combinedText,
        date: last.msg.date ?? first.msg.date,
      });
      const messageIdOverride = last.msg.message_id ? String(last.msg.message_id) : undefined;

      const syntheticCtx = buildSyntheticContext(baseCtx, syntheticMessage);
      const replyMedia = await resolveReplyMediaForMessage(baseCtx, syntheticMessage);
      await handleIngressWithVeriCore({
        msg: syntheticMessage,
        onFallback: async () => {
          await processMessage(
            syntheticCtx,
            combinedMedia,
            first.storeAllowFrom,
            messageIdOverride ? { messageIdOverride } : undefined,
            replyMedia,
          );
        },
      });

    },
    onError: (err) => {
      runtime.error?.(danger(`telegram debounce flush failed: ${String(err)}`));
    },
  });

  const resolveTelegramSessionState = (params: {
    chatId: number | string;
    isGroup: boolean;
    isForum: boolean;
    messageThreadId?: number;
    resolvedThreadId?: number;
  }): {
    agentId: string;
    sessionEntry: ReturnType<typeof loadSessionStore>[string];
    model?: string;
  } => {
    const resolvedThreadId =
      params.resolvedThreadId ??
      resolveTelegramForumThreadId({
        isForum: params.isForum,
        messageThreadId: params.messageThreadId,
      });
    const peerId = params.isGroup
      ? buildTelegramGroupPeerId(params.chatId, resolvedThreadId)
      : String(params.chatId);
    const parentPeer = buildTelegramParentPeer({
      isGroup: params.isGroup,
      resolvedThreadId,
      chatId: params.chatId,
    });
    const route = resolveAgentRoute({
      cfg,
      channel: "telegram",
      accountId,
      peer: {
        kind: params.isGroup ? "group" : "direct",
        id: peerId,
      },
      parentPeer,
    });
    const baseSessionKey = route.sessionKey;
    const dmThreadId = !params.isGroup ? params.messageThreadId : undefined;
    const threadKeys =
      dmThreadId != null
        ? resolveThreadSessionKeys({ baseSessionKey, threadId: `${params.chatId}:${dmThreadId}` })
        : null;
    const sessionKey = threadKeys?.sessionKey ?? baseSessionKey;
    const storePath = resolveStorePath(cfg.session?.store, { agentId: route.agentId });
    const store = loadSessionStore(storePath);
    const entry = store[sessionKey];
    const storedOverride = resolveStoredModelOverride({
      sessionEntry: entry,
      sessionStore: store,
      sessionKey,
    });
    if (storedOverride) {
      return {
        agentId: route.agentId,
        sessionEntry: entry,
        model: storedOverride.provider
          ? `${storedOverride.provider}/${storedOverride.model}`
          : storedOverride.model,
      };
    }
    const provider = entry?.modelProvider?.trim();
    const model = entry?.model?.trim();
    if (provider && model) {
      return {
        agentId: route.agentId,
        sessionEntry: entry,
        model: `${provider}/${model}`,
      };
    }
    const modelCfg = cfg.agents?.defaults?.model;
    return {
      agentId: route.agentId,
      sessionEntry: entry,
      model: typeof modelCfg === "string" ? modelCfg : modelCfg?.primary,
    };
  };

  const processMediaGroup = async (entry: MediaGroupEntry) => {
    try {
      entry.messages.sort((a, b) => a.msg.message_id - b.msg.message_id);

      const captionMsg = entry.messages.find((m) => m.msg.caption || m.msg.text);
      const primaryEntry = captionMsg ?? entry.messages[0];

      const allMedia: TelegramMediaRef[] = [];
      for (const { ctx } of entry.messages) {
        let media;
        try {
          media = await resolveMedia(ctx, mediaMaxBytes, opts.token, opts.proxyFetch);
        } catch (mediaErr) {
          if (!isRecoverableMediaGroupError(mediaErr)) {
            throw mediaErr;
          }
          runtime.log?.(
            warn(`media group: skipping photo that failed to fetch: ${String(mediaErr)}`),
          );
          continue;
        }
        if (media) {
          allMedia.push({
            path: media.path,
            contentType: media.contentType,
            stickerMetadata: media.stickerMetadata,
          });
        }
      }


      const storeAllowFrom = await loadStoreAllowFrom();
      const replyMedia = await resolveReplyMediaForMessage(primaryEntry.ctx, primaryEntry.msg);
      await handleIngressWithVeriCore({
        msg: primaryEntry.msg,
        onFallback: async () => {
          await processMessage(primaryEntry.ctx, allMedia, storeAllowFrom, undefined, replyMedia);
        },
      });

    } catch (err) {
      runtime.error?.(danger(`media group handler failed: ${String(err)}`));
    }
  };

  const flushTextFragments = async (entry: TextFragmentEntry) => {
    try {
      entry.messages.sort((a, b) => a.msg.message_id - b.msg.message_id);

      const first = entry.messages[0];
      const last = entry.messages.at(-1);
      if (!first || !last) {
        return;
      }

      const combinedText = entry.messages.map((m) => m.msg.text ?? "").join("");
      if (!combinedText.trim()) {
        return;
      }

      const syntheticMessage = buildSyntheticTextMessage({
        base: first.msg,
        text: combinedText,
        date: last.msg.date ?? first.msg.date,
      });

      const storeAllowFrom = await loadStoreAllowFrom();
      const baseCtx = first.ctx;


      const syntheticCtx = buildSyntheticContext(baseCtx, syntheticMessage);
      await handleIngressWithVeriCore({
        msg: syntheticMessage,
        onFallback: async () => {
          await processMessage(syntheticCtx, [], storeAllowFrom, {
            messageIdOverride: String(last.msg.message_id),
          });
        },

      });
    } catch (err) {
      runtime.error?.(danger(`text fragment handler failed: ${String(err)}`));
    }
  };

  const queueTextFragmentFlush = async (entry: TextFragmentEntry) => {
    textFragmentProcessing = textFragmentProcessing
      .then(async () => {
        await flushTextFragments(entry);
      })
      .catch(() => undefined);
    await textFragmentProcessing;
  };

  const runTextFragmentFlush = async (entry: TextFragmentEntry) => {
    textFragmentBuffer.delete(entry.key);
    await queueTextFragmentFlush(entry);
  };

  const scheduleTextFragmentFlush = (entry: TextFragmentEntry) => {
    clearTimeout(entry.timer);
    entry.timer = setTimeout(async () => {
      await runTextFragmentFlush(entry);
    }, TELEGRAM_TEXT_FRAGMENT_MAX_GAP_MS);
  };

  const loadStoreAllowFrom = async () =>
    readChannelAllowFromStore("telegram", process.env, accountId).catch(() => []);

  const resolveReplyMediaForMessage = async (
    ctx: TelegramContext,
    msg: Message,
  ): Promise<TelegramMediaRef[]> => {
    const replyMessage = msg.reply_to_message;
    if (!replyMessage || !hasInboundMedia(replyMessage)) {
      return [];
    }
    const replyFileId = resolveInboundMediaFileId(replyMessage);
    if (!replyFileId) {
      return [];
    }
    try {
      const media = await resolveMedia(
        {
          message: replyMessage,
          me: ctx.me,
          getFile: async () => await bot.api.getFile(replyFileId),
        },
        mediaMaxBytes,
        opts.token,
        opts.proxyFetch,
      );
      if (!media) {
        return [];
      }
      return [
        {
          path: media.path,
          contentType: media.contentType,
          stickerMetadata: media.stickerMetadata,
        },
      ];
    } catch (err) {
      logger.warn({ chatId: msg.chat.id, error: String(err) }, "reply media fetch failed");
      return [];
    }
  };

  const isAllowlistAuthorized = (
    allow: NormalizedAllowFrom,
    senderId: string,
    senderUsername: string,
  ) =>
    allow.hasWildcard ||
    (allow.hasEntries &&
      isSenderAllowed({
        allow,
        senderId,
        senderUsername,
      }));


  const shouldSkipGroupByCooldown = (params: {
    isGroup: boolean;
    chatId: string | number;
    resolvedThreadId?: number;
    groupConfig?: TelegramGroupConfig | TelegramDirectConfig;
    text: string;
    botUsername?: string;
  }): boolean => {
    if (!params.isGroup) {
      return false;
    }

    const cooldownSeconds = Math.max(
      0,
      Math.floor(resolveMinReplyIntervalSeconds(params.groupConfig)),
    );
    if (cooldownSeconds <= 0) {
      return false;
    }

    const isControl = hasControlCommand(params.text, cfg, {
      botUsername: params.botUsername,
    });
    if (isControl) {
      return false;
    }

    const nowMs = Date.now();
    const key =
      params.resolvedThreadId != null
        ? String(params.chatId) + ":topic:" + String(params.resolvedThreadId)
        : String(params.chatId);
    const lastMs = lastGroupReplyAtMs.get(key);
    if (typeof lastMs === "number" && nowMs - lastMs < cooldownSeconds * 1000) {
      return true;
    }

    lastGroupReplyAtMs.set(key, nowMs);
    return false;
  };

  const shouldSkipGroupMessage = (params: {
    isGroup: boolean;
    chatId: string | number;
    chatTitle?: string;
    resolvedThreadId?: number;
    senderId: string;
    senderUsername: string;
    effectiveGroupAllow: NormalizedAllowFrom;
    hasGroupAllowOverride: boolean;
    groupConfig?: TelegramGroupConfig;
    topicConfig?: TelegramTopicConfig;
  }) => {
    const {
      isGroup,
      chatId,
      chatTitle,
      resolvedThreadId,
      senderId,
      senderUsername,
      effectiveGroupAllow,
      hasGroupAllowOverride,
      groupConfig,
      topicConfig,
    } = params;
    const baseAccess = evaluateTelegramGroupBaseAccess({
      isGroup,
      groupConfig,
      topicConfig,
      hasGroupAllowOverride,
      effectiveGroupAllow,
      senderId,
      senderUsername,
      enforceAllowOverride: true,
      requireSenderForAllowOverride: true,
    });
    if (!baseAccess.allowed) {
      if (baseAccess.reason === "group-disabled") {
        logVerbose(`Blocked telegram group ${chatId} (group disabled)`);
        return true;
      }
      if (baseAccess.reason === "topic-disabled") {
        logVerbose(
          `Blocked telegram topic ${chatId} (${resolvedThreadId ?? "unknown"}) (topic disabled)`,
        );
        return true;
      }
      logVerbose(
        `Blocked telegram group sender ${senderId || "unknown"} (group allowFrom override)`,
      );
      return true;
    }
    if (!isGroup) {
      return false;
    }
    const policyAccess = evaluateTelegramGroupPolicyAccess({
      isGroup,
      chatId,
      cfg,
      telegramCfg,
      topicConfig,
      groupConfig,
      effectiveGroupAllow,
      senderId,
      senderUsername,
      resolveGroupPolicy,
      enforcePolicy: true,
      useTopicAndGroupOverrides: true,
      enforceAllowlistAuthorization: true,
      allowEmptyAllowlistEntries: false,
      requireSenderForAllowlistAuthorization: true,
      checkChatAllowlist: true,
    });
    if (!policyAccess.allowed) {
      if (policyAccess.reason === "group-policy-disabled") {
        logVerbose("Blocked telegram group message (groupPolicy: disabled)");
        return true;
      }
      if (policyAccess.reason === "group-policy-allowlist-no-sender") {
        logVerbose("Blocked telegram group message (no sender ID, groupPolicy: allowlist)");
        return true;
      }
      if (policyAccess.reason === "group-policy-allowlist-empty") {
        logVerbose(
          "Blocked telegram group message (groupPolicy: allowlist, no group allowlist entries)",
        );
        return true;
      }
      if (policyAccess.reason === "group-policy-allowlist-unauthorized") {
        logVerbose(`Blocked telegram group message from ${senderId} (groupPolicy: allowlist)`);
        return true;
      }
      logger.info({ chatId, title: chatTitle, reason: "not-allowed" }, "skipping group message");
      return true;
    }
    return false;
  };

  type TelegramGroupAllowContext = Awaited<ReturnType<typeof resolveTelegramGroupAllowFromContext>>;
  type TelegramEventAuthorizationMode = "reaction" | "callback-scope" | "callback-allowlist";
  type TelegramEventAuthorizationResult = { allowed: true } | { allowed: false; reason: string };
  type TelegramEventAuthorizationContext = TelegramGroupAllowContext & { dmPolicy: DmPolicy };

  const TELEGRAM_EVENT_AUTH_RULES: Record<
    TelegramEventAuthorizationMode,
    {
      enforceDirectAuthorization: boolean;
      enforceGroupAllowlistAuthorization: boolean;
      deniedDmReason: string;
      deniedGroupReason: string;
    }
  > = {
    reaction: {
      enforceDirectAuthorization: true,
      enforceGroupAllowlistAuthorization: false,
      deniedDmReason: "reaction unauthorized by dm policy/allowlist",
      deniedGroupReason: "reaction unauthorized by group allowlist",
    },
    "callback-scope": {
      enforceDirectAuthorization: false,
      enforceGroupAllowlistAuthorization: false,
      deniedDmReason: "callback unauthorized by inlineButtonsScope",
      deniedGroupReason: "callback unauthorized by inlineButtonsScope",
    },
    "callback-allowlist": {
      enforceDirectAuthorization: true,
      // Group auth is already enforced by shouldSkipGroupMessage (group policy + allowlist).
      // An extra allowlist gate here would block users whose original command was authorized.
      enforceGroupAllowlistAuthorization: false,
      deniedDmReason: "callback unauthorized by inlineButtonsScope allowlist",
      deniedGroupReason: "callback unauthorized by inlineButtonsScope allowlist",
    },
  };

  const resolveTelegramEventAuthorizationContext = async (params: {
    chatId: number;
    isGroup: boolean;
    isForum: boolean;
    messageThreadId?: number;
    groupAllowContext?: TelegramGroupAllowContext;
  }): Promise<TelegramEventAuthorizationContext> => {
    const groupAllowContext =
      params.groupAllowContext ??
      (await resolveTelegramGroupAllowFromContext({
        chatId: params.chatId,
        accountId,
        isGroup: params.isGroup,
        isForum: params.isForum,
        messageThreadId: params.messageThreadId,
        groupAllowFrom,
        resolveTelegramGroupConfig,
      }));
    // Use direct config dmPolicy override if available for DMs
    const effectiveDmPolicy =
      !params.isGroup &&
      groupAllowContext.groupConfig &&
      "dmPolicy" in groupAllowContext.groupConfig
        ? (groupAllowContext.groupConfig.dmPolicy ?? telegramCfg.dmPolicy ?? "pairing")
        : (telegramCfg.dmPolicy ?? "pairing");
    return { dmPolicy: effectiveDmPolicy, ...groupAllowContext };
  };

  const authorizeTelegramEventSender = (params: {
    chatId: number;
    chatTitle?: string;
    isGroup: boolean;
    senderId: string;
    senderUsername: string;
    mode: TelegramEventAuthorizationMode;
    context: TelegramEventAuthorizationContext;
  }): TelegramEventAuthorizationResult => {
    const { chatId, chatTitle, isGroup, senderId, senderUsername, mode, context } = params;
    const {
      dmPolicy,
      resolvedThreadId,
      storeAllowFrom,
      groupConfig,
      topicConfig,
      groupAllowOverride,
      effectiveGroupAllow,
      hasGroupAllowOverride,
    } = context;
    const authRules = TELEGRAM_EVENT_AUTH_RULES[mode];
    const {
      enforceDirectAuthorization,
      enforceGroupAllowlistAuthorization,
      deniedDmReason,
      deniedGroupReason,
    } = authRules;
    if (
      shouldSkipGroupMessage({
        isGroup,
        chatId,
        chatTitle,
        resolvedThreadId,
        senderId,
        senderUsername,
        effectiveGroupAllow,
        hasGroupAllowOverride,
        groupConfig,
        topicConfig,
      })
    ) {
      return { allowed: false, reason: "group-policy" };
    }

    if (!isGroup && enforceDirectAuthorization) {
      if (dmPolicy === "disabled") {
        logVerbose(
          `Blocked telegram direct event from ${senderId || "unknown"} (${deniedDmReason})`,
        );
        return { allowed: false, reason: "direct-disabled" };
      }
      if (dmPolicy !== "open") {
        // For DMs, prefer per-DM/topic allowFrom (groupAllowOverride) over account-level allowFrom
        const dmAllowFrom = groupAllowOverride ?? allowFrom;
        const effectiveDmAllow = normalizeDmAllowFromWithStore({
          allowFrom: dmAllowFrom,
          storeAllowFrom,
          dmPolicy,
        });
        if (!isAllowlistAuthorized(effectiveDmAllow, senderId, senderUsername)) {
          logVerbose(`Blocked telegram direct sender ${senderId || "unknown"} (${deniedDmReason})`);
          return { allowed: false, reason: "direct-unauthorized" };
        }
      }
    }
    if (isGroup && enforceGroupAllowlistAuthorization) {
      if (!isAllowlistAuthorized(effectiveGroupAllow, senderId, senderUsername)) {
        logVerbose(`Blocked telegram group sender ${senderId || "unknown"} (${deniedGroupReason})`);
        return { allowed: false, reason: "group-unauthorized" };
      }
    }
    return { allowed: true };
  };

  // Handle emoji reactions to messages.
  bot.on("message_reaction", async (ctx) => {
    try {
      const reaction = ctx.messageReaction;
      if (!reaction) {
        return;
      }
      if (shouldSkipUpdate(ctx)) {
        return;
      }

      const chatId = reaction.chat.id;
      const messageId = reaction.message_id;
      const user = reaction.user;
      const senderId = user?.id != null ? String(user.id) : "";
      const senderUsername = user?.username ?? "";
      const isGroup = reaction.chat.type === "group" || reaction.chat.type === "supergroup";
      const isForum = reaction.chat.is_forum === true;

      // Resolve reaction notification mode (default: "own").
      const reactionMode = telegramCfg.reactionNotifications ?? "own";
      if (reactionMode === "off") {
        return;
      }
      if (user?.is_bot) {
        return;
      }
      if (reactionMode === "own" && !wasSentByBot(chatId, messageId)) {
        return;
      }
      const eventAuthContext = await resolveTelegramEventAuthorizationContext({
        chatId,
        isGroup,
        isForum,
      });
      const senderAuthorization = authorizeTelegramEventSender({
        chatId,
        chatTitle: reaction.chat.title,
        isGroup,
        senderId,
        senderUsername,
        mode: "reaction",
        context: eventAuthContext,
      });
      if (!senderAuthorization.allowed) {
        return;
      }

      // Enforce requireTopic for DM reactions: since Telegram doesn't provide messageThreadId
      // for reactions, we cannot determine if the reaction came from a topic, so block all
      // reactions if requireTopic is enabled for this DM.
      if (!isGroup) {
        const requireTopic = (eventAuthContext.groupConfig as TelegramDirectConfig | undefined)
          ?.requireTopic;
        if (requireTopic === true) {
          logVerbose(
            `Blocked telegram reaction in DM ${chatId}: requireTopic=true but topic unknown for reactions`,
          );
          return;
        }
      }

      // Detect added reactions.
      const oldEmojis = new Set(
        reaction.old_reaction
          .filter((r): r is ReactionTypeEmoji => r.type === "emoji")
          .map((r) => r.emoji),
      );
      const addedReactions = reaction.new_reaction
        .filter((r): r is ReactionTypeEmoji => r.type === "emoji")
        .filter((r) => !oldEmojis.has(r.emoji));

      if (addedReactions.length === 0) {
        return;
      }

      // Build sender label.
      const senderName = user
        ? [user.first_name, user.last_name].filter(Boolean).join(" ").trim() || user.username
        : undefined;
      const senderUsernameLabel = user?.username ? `@${user.username}` : undefined;
      let senderLabel = senderName;
      if (senderName && senderUsernameLabel) {
        senderLabel = `${senderName} (${senderUsernameLabel})`;
      } else if (!senderName && senderUsernameLabel) {
        senderLabel = senderUsernameLabel;
      }
      if (!senderLabel && user?.id) {
        senderLabel = `id:${user.id}`;
      }
      senderLabel = senderLabel || "unknown";

      // Reactions target a specific message_id; the Telegram Bot API does not include
      // message_thread_id on MessageReactionUpdated, so we route to the chat-level
      // session (forum topic routing is not available for reactions).
      const resolvedThreadId = isForum
        ? resolveTelegramForumThreadId({ isForum, messageThreadId: undefined })
        : undefined;
      const peerId = isGroup ? buildTelegramGroupPeerId(chatId, resolvedThreadId) : String(chatId);
      const parentPeer = buildTelegramParentPeer({ isGroup, resolvedThreadId, chatId });
      // Fresh config for bindings lookup; other routing inputs are payload-derived.
      const route = resolveAgentRoute({
        cfg: loadConfig(),
        channel: "telegram",
        accountId,
        peer: { kind: isGroup ? "group" : "direct", id: peerId },
        parentPeer,
      });
      const sessionKey = route.sessionKey;

      // Enqueue system event for each added reaction.
      for (const r of addedReactions) {
        const emoji = r.emoji;
        const text = `Telegram reaction added: ${emoji} by ${senderLabel} on msg ${messageId}`;
        enqueueSystemEvent(text, {
          sessionKey,
          contextKey: `telegram:reaction:add:${chatId}:${messageId}:${user?.id ?? "anon"}:${emoji}`,
        });
        logVerbose(`telegram: reaction event enqueued: ${text}`);
      }
    } catch (err) {
      runtime.error?.(danger(`telegram reaction handler failed: ${String(err)}`));
    }
  });
  const processInboundMessage = async (params: {
    ctx: TelegramContext;
    msg: Message;
    chatId: number;
    resolvedThreadId?: number;
    dmThreadId?: number;
    storeAllowFrom: string[];
    sendOversizeWarning: boolean;
    oversizeLogMessage: string;
  }) => {
    const {
      ctx,
      msg,
      chatId,
      resolvedThreadId,
      dmThreadId,
      storeAllowFrom,
      sendOversizeWarning,
      oversizeLogMessage,
    } = params;

    // Text fragment handling - Telegram splits long pastes into multiple inbound messages (~4096 chars).
    // We buffer “near-limit” messages and append immediately-following parts.
    const text = typeof msg.text === "string" ? msg.text : undefined;
    const isCommandLike = (text ?? "").trim().startsWith("/");
    if (text && !isCommandLike) {
      const nowMs = Date.now();
      const senderId = msg.from?.id != null ? String(msg.from.id) : "unknown";
      // Use resolvedThreadId for forum groups, dmThreadId for DM topics
      const threadId = resolvedThreadId ?? dmThreadId;
      const key = `text:${chatId}:${threadId ?? "main"}:${senderId}`;
      const existing = textFragmentBuffer.get(key);

      if (existing) {
        const last = existing.messages.at(-1);
        const lastMsgId = last?.msg.message_id;
        const lastReceivedAtMs = last?.receivedAtMs ?? nowMs;
        const idGap = typeof lastMsgId === "number" ? msg.message_id - lastMsgId : Infinity;
        const timeGapMs = nowMs - lastReceivedAtMs;
        const canAppend =
          idGap > 0 &&
          idGap <= TELEGRAM_TEXT_FRAGMENT_MAX_ID_GAP &&
          timeGapMs >= 0 &&
          timeGapMs <= TELEGRAM_TEXT_FRAGMENT_MAX_GAP_MS;

        if (canAppend) {
          const currentTotalChars = existing.messages.reduce(
            (sum, m) => sum + (m.msg.text?.length ?? 0),
            0,
          );
          const nextTotalChars = currentTotalChars + text.length;
          if (
            existing.messages.length + 1 <= TELEGRAM_TEXT_FRAGMENT_MAX_PARTS &&
            nextTotalChars <= TELEGRAM_TEXT_FRAGMENT_MAX_TOTAL_CHARS
          ) {
            existing.messages.push({ msg, ctx, receivedAtMs: nowMs });
            scheduleTextFragmentFlush(existing);
            return;
          }
        }

        // Not appendable (or limits exceeded): flush buffered entry first, then continue normally.
        clearTimeout(existing.timer);
        textFragmentBuffer.delete(key);
        textFragmentProcessing = textFragmentProcessing
          .then(async () => {
            await flushTextFragments(existing);
          })
          .catch(() => undefined);
        await textFragmentProcessing;
      }

      const shouldStart = text.length >= TELEGRAM_TEXT_FRAGMENT_START_THRESHOLD_CHARS;
      if (shouldStart) {
        const entry: TextFragmentEntry = {
          key,
          messages: [{ msg, ctx, receivedAtMs: nowMs }],
          timer: setTimeout(() => {}, TELEGRAM_TEXT_FRAGMENT_MAX_GAP_MS),
        };
        textFragmentBuffer.set(key, entry);
        scheduleTextFragmentFlush(entry);
        return;
      }
    }

    // Media group handling - buffer multi-image messages
    const mediaGroupId = msg.media_group_id;
    if (mediaGroupId) {
      const existing = mediaGroupBuffer.get(mediaGroupId);
      if (existing) {
        clearTimeout(existing.timer);
        existing.messages.push({ msg, ctx });
        existing.timer = setTimeout(async () => {
          mediaGroupBuffer.delete(mediaGroupId);
          mediaGroupProcessing = mediaGroupProcessing
            .then(async () => {
              await processMediaGroup(existing);
            })
            .catch(() => undefined);
          await mediaGroupProcessing;
        }, mediaGroupTimeoutMs);
      } else {
        const entry: MediaGroupEntry = {
          messages: [{ msg, ctx }],
          timer: setTimeout(async () => {
            mediaGroupBuffer.delete(mediaGroupId);
            mediaGroupProcessing = mediaGroupProcessing
              .then(async () => {
                await processMediaGroup(entry);
              })
              .catch(() => undefined);
            await mediaGroupProcessing;
          }, mediaGroupTimeoutMs),
        };
        mediaGroupBuffer.set(mediaGroupId, entry);
      }
      return;
    }

    let media: Awaited<ReturnType<typeof resolveMedia>> = null;
    try {
      media = await resolveMedia(ctx, mediaMaxBytes, opts.token, opts.proxyFetch);
    } catch (mediaErr) {
      if (isMediaSizeLimitError(mediaErr)) {
        if (sendOversizeWarning) {
          const limitMb = Math.round(mediaMaxBytes / (1024 * 1024));
          await withTelegramApiErrorLogging({
            operation: "sendMessage",
            runtime,
            fn: () =>
              bot.api.sendMessage(chatId, `⚠️ File too large. Maximum size is ${limitMb}MB.`, {
                reply_to_message_id: msg.message_id,
              }),
          }).catch(() => {});
        }
        logger.warn({ chatId, error: String(mediaErr) }, oversizeLogMessage);
        return;
      }
      logger.warn({ chatId, error: String(mediaErr) }, "media fetch failed");
      await withTelegramApiErrorLogging({
        operation: "sendMessage",
        runtime,
        fn: () =>
          bot.api.sendMessage(chatId, "⚠️ Failed to download media. Please try again.", {
            reply_to_message_id: msg.message_id,
          }),
      }).catch(() => {});
      return;
    }

    // Skip sticker-only messages where the sticker was skipped (animated/video)
    // These have no media and no text content to process.
    const hasText = Boolean((msg.text ?? msg.caption ?? "").trim());
    if (msg.sticker && !media && !hasText) {
      logVerbose("telegram: skipping sticker-only message (unsupported sticker type)");
      return;
    }

    const allMedia = media
      ? [
          {
            path: media.path,
            contentType: media.contentType,
            stickerMetadata: media.stickerMetadata,
          },
        ]
      : [];
    const senderId = msg.from?.id ? String(msg.from.id) : "";
    const conversationThreadId = resolvedThreadId ?? dmThreadId;
    const conversationKey =
      conversationThreadId != null ? `${chatId}:topic:${conversationThreadId}` : String(chatId);
    const debounceLane = resolveTelegramDebounceLane(msg);
    const debounceKey = senderId
      ? `telegram:${accountId ?? "default"}:${conversationKey}:${senderId}:${debounceLane}`
      : null;
    await inboundDebouncer.enqueue({
      ctx,
      msg,
      allMedia,
      storeAllowFrom,
      debounceKey,
      debounceLane,
      botUsername: ctx.me?.username,
    });
  };
  bot.on("callback_query", async (ctx) => {
    const callback = ctx.callbackQuery;
    if (!callback) {
      return;
    }
    if (shouldSkipUpdate(ctx)) {
      return;
    }
    const answerCallbackQuery =
      typeof (ctx as { answerCallbackQuery?: unknown }).answerCallbackQuery === "function"
        ? () => ctx.answerCallbackQuery()
        : () => bot.api.answerCallbackQuery(callback.id);
    // Answer immediately to prevent Telegram from retrying while we process
    await withTelegramApiErrorLogging({
      operation: "answerCallbackQuery",
      runtime,
      fn: answerCallbackQuery,
    }).catch(() => {});
    try {
      const data = (callback.data ?? "").trim();
      const callbackMessage = callback.message;
      if (!data || !callbackMessage) {
        return;
      }
      const editCallbackMessage = async (
        text: string,
        params?: Parameters<typeof bot.api.editMessageText>[3],
      ) => {
        const editTextFn = (ctx as { editMessageText?: unknown }).editMessageText;
        if (typeof editTextFn === "function") {
          return await ctx.editMessageText(text, params);
        }
        return await bot.api.editMessageText(
          callbackMessage.chat.id,
          callbackMessage.message_id,
          text,
          params,
        );
      };
      const deleteCallbackMessage = async () => {
        const deleteFn = (ctx as { deleteMessage?: unknown }).deleteMessage;
        if (typeof deleteFn === "function") {
          return await ctx.deleteMessage();
        }
        return await bot.api.deleteMessage(callbackMessage.chat.id, callbackMessage.message_id);
      };
      const replyToCallbackChat = async (
        text: string,
        params?: Parameters<typeof bot.api.sendMessage>[2],
      ) => {
        const replyFn = (ctx as { reply?: unknown }).reply;
        if (typeof replyFn === "function") {
          return await ctx.reply(text, params);
        }
        return await bot.api.sendMessage(callbackMessage.chat.id, text, params);
      };

      const inlineButtonsScope = resolveTelegramInlineButtonsScope({
        cfg,
        accountId,
      });
      if (inlineButtonsScope === "off") {
        return;
      }

      const chatId = callbackMessage.chat.id;
      const isGroup =
        callbackMessage.chat.type === "group" || callbackMessage.chat.type === "supergroup";
      if (inlineButtonsScope === "dm" && isGroup) {
        return;
      }
      if (inlineButtonsScope === "group" && !isGroup) {
        return;
      }

      const messageThreadId = callbackMessage.message_thread_id;
      const isForum = callbackMessage.chat.is_forum === true;
      const eventAuthContext = await resolveTelegramEventAuthorizationContext({
        chatId,
        isGroup,
        isForum,
        messageThreadId,
      });
      const { resolvedThreadId, dmThreadId, storeAllowFrom, groupConfig } = eventAuthContext;
      const requireTopic = (groupConfig as { requireTopic?: boolean } | undefined)?.requireTopic;
      if (!isGroup && requireTopic === true && dmThreadId == null) {
        logVerbose(
          `Blocked telegram callback in DM ${chatId}: requireTopic=true but no topic present`,
        );
        return;
      }
      const senderId = callback.from?.id ? String(callback.from.id) : "";
      const senderUsername = callback.from?.username ?? "";
      const authorizationMode: TelegramEventAuthorizationMode =
        inlineButtonsScope === "allowlist" ? "callback-allowlist" : "callback-scope";
      const senderAuthorization = authorizeTelegramEventSender({
        chatId,
        chatTitle: callbackMessage.chat.title,
        isGroup,
        senderId,
        senderUsername,
        mode: authorizationMode,
        context: eventAuthContext,
      });
      if (!senderAuthorization.allowed) {
        return;
      }

      const paginationMatch = data.match(/^commands_page_(\d+|noop)(?::(.+))?$/);
      if (paginationMatch) {
        const pageValue = paginationMatch[1];
        if (pageValue === "noop") {
          return;
        }

        const page = Number.parseInt(pageValue, 10);
        if (Number.isNaN(page) || page < 1) {
          return;
        }

        const agentId = paginationMatch[2]?.trim() || resolveDefaultAgentId(cfg);
        const skillCommands = listSkillCommandsForAgents({
          cfg,
          agentIds: [agentId],
        });
        const result = buildCommandsMessagePaginated(cfg, skillCommands, {
          page,
          surface: "telegram",
        });

        const keyboard =
          result.totalPages > 1
            ? buildInlineKeyboard(
                buildCommandsPaginationKeyboard(result.currentPage, result.totalPages, agentId),
              )
            : undefined;

        try {
          await editCallbackMessage(result.text, keyboard ? { reply_markup: keyboard } : undefined);
        } catch (editErr) {
          const errStr = String(editErr);
          if (!errStr.includes("message is not modified")) {
            throw editErr;
          }
        }
        return;
      }

      // Model selection callback handler (mdl_prov, mdl_list_*, mdl_sel_*, mdl_back)
      const modelCallback = parseModelCallbackData(data);
      if (modelCallback) {
        const modelData = await buildModelsProviderData(cfg);
        const { byProvider, providers } = modelData;

        const editMessageWithButtons = async (
          text: string,
          buttons: ReturnType<typeof buildProviderKeyboard>,
        ) => {
          const keyboard = buildInlineKeyboard(buttons);
          try {
            await editCallbackMessage(text, keyboard ? { reply_markup: keyboard } : undefined);
          } catch (editErr) {
            const errStr = String(editErr);
            if (errStr.includes("no text in the message")) {
              try {
                await deleteCallbackMessage();
              } catch {}
              await replyToCallbackChat(text, keyboard ? { reply_markup: keyboard } : undefined);
            } else if (!errStr.includes("message is not modified")) {
              throw editErr;
            }
          }
        };

        if (modelCallback.type === "providers" || modelCallback.type === "back") {
          if (providers.length === 0) {
            await editMessageWithButtons("No providers available.", []);
            return;
          }
          const providerInfos: ProviderInfo[] = providers.map((p) => ({
            id: p,
            count: byProvider.get(p)?.size ?? 0,
          }));
          const buttons = buildProviderKeyboard(providerInfos);
          await editMessageWithButtons("Select a provider:", buttons);
          return;
        }

        if (modelCallback.type === "list") {
          const { provider, page } = modelCallback;
          const modelSet = byProvider.get(provider);
          if (!modelSet || modelSet.size === 0) {
            // Provider not found or no models - show providers list
            const providerInfos: ProviderInfo[] = providers.map((p) => ({
              id: p,
              count: byProvider.get(p)?.size ?? 0,
            }));
            const buttons = buildProviderKeyboard(providerInfos);
            await editMessageWithButtons(
              `Unknown provider: ${provider}\n\nSelect a provider:`,
              buttons,
            );
            return;
          }
          const models = [...modelSet].toSorted();
          const pageSize = getModelsPageSize();
          const totalPages = calculateTotalPages(models.length, pageSize);
          const safePage = Math.max(1, Math.min(page, totalPages));

          // Resolve current model from session (prefer overrides)
          const sessionState = resolveTelegramSessionState({
            chatId,
            isGroup,
            isForum,
            messageThreadId,
            resolvedThreadId,
          });
          const currentModel = sessionState.model;

          const buttons = buildModelsKeyboard({
            provider,
            models,
            currentModel,
            currentPage: safePage,
            totalPages,
            pageSize,
          });
          const text = formatModelsAvailableHeader({
            provider,
            total: models.length,
            cfg,
            agentDir: resolveAgentDir(cfg, sessionState.agentId),
            sessionEntry: sessionState.sessionEntry,
          });
          await editMessageWithButtons(text, buttons);
          return;
        }

        if (modelCallback.type === "select") {
          const selection = resolveModelSelection({
            callback: modelCallback,
            providers,
            byProvider,
          });
          if (selection.kind !== "resolved") {
            const providerInfos: ProviderInfo[] = providers.map((p) => ({
              id: p,
              count: byProvider.get(p)?.size ?? 0,
            }));
            const buttons = buildProviderKeyboard(providerInfos);
            await editMessageWithButtons(
              `Could not resolve model "${selection.model}".\n\nSelect a provider:`,
              buttons,
            );
            return;
          }
          // Process model selection as a synthetic message with /model command
          const syntheticMessage = buildSyntheticTextMessage({
            base: callbackMessage,
            from: callback.from,
            text: `/model ${selection.provider}/${selection.model}`,
          });
          await handleIngressWithVeriCore({
            msg: syntheticMessage,
            onFallback: async () => {
              await processMessage(buildSyntheticContext(ctx, syntheticMessage), [], storeAllowFrom, {
                forceWasMentioned: true,
                messageIdOverride: callback.id,
              });
            },

          });
          return;
        }

        return;
      }

      const syntheticMessage = buildSyntheticTextMessage({
        base: callbackMessage,
        from: callback.from,
        text: data,

      });
      await handleIngressWithVeriCore({
        msg: syntheticMessage,
        onFallback: async () => {
          await processMessage(buildSyntheticContext(ctx, syntheticMessage), [], storeAllowFrom, {
            forceWasMentioned: true,
            messageIdOverride: callback.id,
          });
        },

      });
    } catch (err) {
      runtime.error?.(danger(`callback handler failed: ${String(err)}`));
    }
  });

  // Handle group migration to supergroup (chat ID changes)
  bot.on("message:migrate_to_chat_id", async (ctx) => {
    try {
      const msg = ctx.message;
      if (!msg?.migrate_to_chat_id) {
        return;
      }
      if (shouldSkipUpdate(ctx)) {
        return;
      }

      const oldChatId = String(msg.chat.id);
      const newChatId = String(msg.migrate_to_chat_id);
      const chatTitle = msg.chat.title ?? "Unknown";

      runtime.log?.(warn(`[telegram] Group migrated: "${chatTitle}" ${oldChatId} → ${newChatId}`));

      if (!resolveChannelConfigWrites({ cfg, channelId: "telegram", accountId })) {
        runtime.log?.(warn("[telegram] Config writes disabled; skipping group config migration."));
        return;
      }

      // Check if old chat ID has config and migrate it
      const currentConfig = loadConfig();
      const migration = migrateTelegramGroupConfig({
        cfg: currentConfig,
        accountId,
        oldChatId,
        newChatId,
      });

      if (migration.migrated) {
        runtime.log?.(warn(`[telegram] Migrating group config from ${oldChatId} to ${newChatId}`));
        migrateTelegramGroupConfig({ cfg, accountId, oldChatId, newChatId });
        await writeConfigFile(currentConfig);
        runtime.log?.(warn(`[telegram] Group config migrated and saved successfully`));
      } else if (migration.skippedExisting) {
        runtime.log?.(
          warn(
            `[telegram] Group config already exists for ${newChatId}; leaving ${oldChatId} unchanged`,
          ),
        );
      } else {
        runtime.log?.(
          warn(`[telegram] No config found for old group ID ${oldChatId}, migration logged only`),
        );
      }
    } catch (err) {
      runtime.error?.(danger(`[telegram] Group migration handler failed: ${String(err)}`));
    }
  });

  type InboundTelegramEvent = {
    ctxForDedupe: TelegramUpdateKeyContext;
    ctx: TelegramContext;
    msg: Message;
    chatId: number;
    isGroup: boolean;
    isForum: boolean;
    messageThreadId?: number;
    senderId: string;
    senderUsername: string;
    requireConfiguredGroup: boolean;
    sendOversizeWarning: boolean;
    oversizeLogMessage: string;
    errorMessage: string;
  };

  const handleInboundMessageLike = async (event: InboundTelegramEvent) => {
    try {
      if (shouldSkipUpdate(event.ctxForDedupe)) {
        return;
      }
      const eventAuthContext = await resolveTelegramEventAuthorizationContext({
        chatId: event.chatId,
        isGroup: event.isGroup,
        isForum: event.isForum,
        messageThreadId: event.messageThreadId,
      });
      const {
        dmPolicy,
        resolvedThreadId,
        dmThreadId,
        storeAllowFrom,
        groupConfig,
        topicConfig,
        groupAllowOverride,
        effectiveGroupAllow,
        hasGroupAllowOverride,
      } = eventAuthContext;
      // For DMs, prefer per-DM/topic allowFrom (groupAllowOverride) over account-level allowFrom
      const dmAllowFrom = groupAllowOverride ?? allowFrom;
      const effectiveDmAllow = normalizeDmAllowFromWithStore({
        allowFrom: dmAllowFrom,
        storeAllowFrom,
        dmPolicy,
      });

      if (event.requireConfiguredGroup && (!groupConfig || groupConfig.enabled === false)) {
        logVerbose(`Blocked telegram channel ${event.chatId} (channel disabled)`);
        return;
      }

      if (
        shouldSkipGroupMessage({
          isGroup: event.isGroup,
          chatId: event.chatId,
          chatTitle: event.msg.chat.title,
          resolvedThreadId,
          senderId: event.senderId,
          senderUsername: event.senderUsername,
          effectiveGroupAllow,
          hasGroupAllowOverride,
          groupConfig,
          topicConfig,
        })
      ) {
        return;
      }

      if (
        shouldSkipGroupByCooldown({
          isGroup: event.isGroup,
          chatId: event.chatId,
          resolvedThreadId,
          groupConfig,
          text: event.msg.text ?? event.msg.caption ?? "",
          botUsername: event.ctx.me?.username,
        })
      ) {
        logVerbose(
          `Blocked telegram group ${event.chatId} (reply cooldown: ${resolveMinReplyIntervalSeconds(groupConfig)}s)`,
        );
        return;
      }

      if (!event.isGroup && (hasInboundMedia(event.msg) || hasReplyTargetMedia(event.msg))) {
        const dmAuthorized = await enforceTelegramDmAccess({
          isGroup: event.isGroup,
          dmPolicy,
          msg: event.msg,
          chatId: event.chatId,
          effectiveDmAllow,
          accountId,
          bot,
          logger,
        });
        if (!dmAuthorized) {
          return;
        }
      }

      await processInboundMessage({
        ctx: event.ctx,
        msg: event.msg,
        chatId: event.chatId,
        resolvedThreadId,
        dmThreadId,
        storeAllowFrom,
        sendOversizeWarning: event.sendOversizeWarning,
        oversizeLogMessage: event.oversizeLogMessage,
      });
    } catch (err) {
      runtime.error?.(danger(`${event.errorMessage}: ${String(err)}`));
    }
  };

  bot.on("message", async (ctx) => {
    const msg = ctx.message;
    if (!msg) {
      return;
    }
    await handleInboundMessageLike({
      ctxForDedupe: ctx,
      ctx: buildSyntheticContext(ctx, msg),
      msg,
      chatId: msg.chat.id,
      isGroup: msg.chat.type === "group" || msg.chat.type === "supergroup",
      isForum: msg.chat.is_forum === true,
      messageThreadId: msg.message_thread_id,
      senderId: msg.from?.id != null ? String(msg.from.id) : "",
      senderUsername: msg.from?.username ?? "",
      requireConfiguredGroup: false,
      sendOversizeWarning: true,
      oversizeLogMessage: "media exceeds size limit",
      errorMessage: "handler failed",
    });
  });

  // Handle channel posts — enables bot-to-bot communication via Telegram channels.
  // Telegram bots cannot see other bot messages in groups, but CAN in channels.
  // This handler normalizes channel_post updates into the standard message pipeline.
  bot.on("channel_post", async (ctx) => {
    const post = ctx.channelPost;
    if (!post) {
      return;
    }

    const chatId = post.chat.id;
    const syntheticFrom = post.sender_chat
      ? {
          id: post.sender_chat.id,
          is_bot: true as const,
          first_name: post.sender_chat.title || "Channel",
          username: post.sender_chat.username,
        }
      : {
          id: chatId,
          is_bot: true as const,
          first_name: post.chat.title || "Channel",
          username: post.chat.username,
        };
    const syntheticMsg: Message = {
      ...post,
      from: post.from ?? syntheticFrom,
      chat: {
        ...post.chat,
        type: "supergroup" as const,
      },
    } as Message;

    await handleInboundMessageLike({
      ctxForDedupe: ctx,
      ctx: buildSyntheticContext(ctx, syntheticMsg),
      msg: syntheticMsg,
      chatId,
      isGroup: true,
      isForum: false,
      senderId:
        post.sender_chat?.id != null
          ? String(post.sender_chat.id)
          : post.from?.id != null
            ? String(post.from.id)
            : "",
      senderUsername: post.sender_chat?.username ?? post.from?.username ?? "",
      requireConfiguredGroup: true,
      sendOversizeWarning: false,
      oversizeLogMessage: "channel post media exceeds size limit",
      errorMessage: "channel_post handler failed",
    });
  });
};
