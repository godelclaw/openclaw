import { logVerbose } from "../../globals.js";
import { isInternalMessageChannel } from "../../utils/message-channel.js";
import {
  buildVeriCoreStimulusInput,
  runVeriCoreMindlockApprove,
  runVeriCoreMindlockList,
  runVeriCoreMindlockPending,
  runVeriCoreMindlockReject,
  runVeriCoreMindlockStatus,
  runVeriCoreMindlockView,
  type VeriCoreMindlockDecisionResult,
  type VeriCoreMindlockListResult,
  type VeriCoreMindlockPendingResult,
  type VeriCoreMindlockStatusResult,
  type VeriCoreMindlockViewResult,
} from "../../vericore/impetus.js";
import type { CommandHandler } from "./commands-types.js";

function extractArgs(commandBodyNormalized: string, command: string): string | null {
  if (commandBodyNormalized === command) {
    return "";
  }
  if (commandBodyNormalized.startsWith(command + " ")) {
    return commandBodyNormalized.slice(command.length + 1).trim();
  }
  return null;
}

function parseMindlockArtifactArgs(commandBodyNormalized: string, command: string): {
  artifactId?: string;
  reason?: string;
  error?: string;
} {
  const body = extractArgs(commandBodyNormalized, command);
  const trimmed = (body ?? "").trim();
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

function parseMindlockMenuArgs(commandBodyNormalized: string): {
  box?: "in" | "out" | "pending" | "rejected";
  limit?: number;
  error?: string;
} {
  const body = (extractArgs(commandBodyNormalized, "/mindlock") ?? "").trim();
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

function formatMindlockPendingMessage(result: VeriCoreMindlockPendingResult): string {
  if (!result.items.length) {
    return "Mindlock pending review queue is empty.";
  }

  const lines = result.items.slice(0, 10).flatMap((item, index) => {
    const target = item.target_path ? ` -> ${item.target_path}` : "";
    const main = `${index + 1}. id=${item.id}${target} (${item.size_bytes} bytes)`;
    if (item.reviewer_assessment?.status === "pending") {
      return [main, "   🔍 Reviewer: pending"];
    }
    if (item.reviewer_assessment?.verdict && item.reviewer_assessment?.reason) {
      return [
        main,
        `   🔍 Reviewer: ${item.reviewer_assessment.verdict} — ${item.reviewer_assessment.reason}`,
      ];
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
    "View: /view <id|index>",
    "Approve: /a <id|index> [reason]",
    "Reject: /reject <id|index> [reason] (alias: /r)",
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
    "View: /view <id|index>",
    "Approve: /a <id|index> [reason]",
    "Reject: /reject <id|index> [reason] (alias: /r)",
  ]
    .filter((line): line is string => typeof line === "string" && line.trim().length > 0)
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
      return [main, "   🔍 Reviewer: pending"];
    }
    if (item.reviewer_assessment?.verdict && item.reviewer_assessment?.reason) {
      return [
        main,
        `   🔍 Reviewer: ${item.reviewer_assessment.verdict} — ${item.reviewer_assessment.reason}`,
      ];
    }
    return [main];
  });

  const asOf =
    typeof result.as_of_ts === "number"
      ? `As of: ${new Date(result.as_of_ts * 1000).toISOString()}`
      : undefined;

  return [`Mindlock ${result.box} (${result.count}):`, asOf, ...lines]
    .filter((line): line is string => typeof line === "string" && line.trim().length > 0)
    .join("\n");
}

function formatMindlockViewMessage(result: VeriCoreMindlockViewResult): string {
  const asOf =
    typeof result.modified_ts === "number"
      ? `Modified: ${new Date(result.modified_ts * 1000).toISOString()}`
      : undefined;
  const target = result.target_path ? `- target: ${result.target_path}` : undefined;
  const reviewer = result.reviewer_assessment?.verdict
    ? `- reviewer: ${result.reviewer_assessment.verdict}${
        result.reviewer_assessment.reason ? ` — ${result.reviewer_assessment.reason}` : ""
      }`
    : result.reviewer_assessment?.status === "pending"
      ? "- reviewer: pending"
      : undefined;
  const previewHeader = result.preview_binary
    ? "Preview: binary artifact"
    : result.preview_truncated
      ? "Preview (truncated):"
      : "Preview:";

  return [
    `Mindlock view: ${result.id}`,
    asOf,
    `- path: ${result.path}`,
    target,
    `- bytes: ${result.size_bytes}`,
    reviewer,
    previewHeader,
    result.preview,
  ]
    .filter((line): line is string => typeof line === "string" && line.trim().length > 0)
    .join("\n");
}

function formatMindlockDecisionMessage(result: VeriCoreMindlockDecisionResult): string {
  const lines = [`Mindlock ${result.status}: ${result.id}`, `- source: ${result.source}`];
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

function isMindlockOperator(params: Parameters<CommandHandler>[0]): boolean {
  if (!params.command.isAuthorizedSender) {
    return false;
  }
  if (params.command.surface === "webchat" || params.command.surface === "web") {
    return true;
  }
  if (params.command.senderId) {
    return params.command.senderIsOwner;
  }
  if (isInternalMessageChannel(params.command.channel)) {
    const scopes = params.ctx.GatewayClientScopes ?? [];
    return scopes.includes("operator.approvals") || scopes.includes("operator.admin");
  }
  return true;
}

function buildStimulus(params: Parameters<CommandHandler>[0]) {
  return {
    ...buildVeriCoreStimulusInput(params.ctx as never),
    session_key: params.ctx.SessionKey ?? params.sessionKey,
    content: params.ctx.Body ?? params.command.commandBodyNormalized,
  };
}

export const handleMindlockCommands: CommandHandler = async (params, allowTextCommands) => {
  if (!allowTextCommands) {
    return null;
  }

  const normalized = params.command.commandBodyNormalized;
  const isMindlockMenu = normalized === "/mindlock" || normalized.startsWith("/mindlock ");
  const isReview = normalized === "/review";
  const isView = normalized === "/view" || normalized.startsWith("/view ");
  const isApprove = normalized === "/a" || normalized.startsWith("/a ");
  const isReject = normalized === "/reject" || normalized.startsWith("/reject ");

  if (!isMindlockMenu && !isReview && !isView && !isApprove && !isReject) {
    return null;
  }

  if (!isMindlockOperator(params)) {
    logVerbose(
      `Ignoring ${normalized.split(/\s+/, 1)[0] || "/mindlock"} from unauthorized sender: ${params.command.senderId || "<unknown>"}`,
    );
    return {
      shouldContinue: false,
      reply: { text: "Mindlock review commands are owner-only." },
    };
  }

  const stimulus = buildStimulus(params);

  try {
    if (isMindlockMenu) {
      const parsed = parseMindlockMenuArgs(normalized);
      if (parsed.error) {
        return { shouldContinue: false, reply: { text: parsed.error } };
      }
      if (!parsed.box) {
        const status = await runVeriCoreMindlockStatus(stimulus);
        return { shouldContinue: false, reply: { text: formatMindlockStatusMessage(status) } };
      }
      const listing = await runVeriCoreMindlockList(parsed.box, stimulus, parsed.limit);
      return { shouldContinue: false, reply: { text: formatMindlockListMessage(listing) } };
    }

    if (isReview) {
      const result = await runVeriCoreMindlockPending(stimulus);
      return { shouldContinue: false, reply: { text: formatMindlockPendingMessage(result) } };
    }

    if (isView) {
      const parsed = parseMindlockArtifactArgs(normalized, "/view");
      if (!parsed.artifactId) {
        return { shouldContinue: false, reply: { text: "Usage: /view <artifact_id|index>" } };
      }
      const result = await runVeriCoreMindlockView(parsed.artifactId, stimulus);
      return { shouldContinue: false, reply: { text: formatMindlockViewMessage(result) } };
    }

    if (isApprove) {
      const parsed = parseMindlockArtifactArgs(normalized, "/a");
      if (!parsed.artifactId) {
        return { shouldContinue: false, reply: { text: "Usage: /a <artifact_id|index> [reason]" } };
      }
      const result = await runVeriCoreMindlockApprove(parsed.artifactId, stimulus, parsed.reason);
      return { shouldContinue: false, reply: { text: formatMindlockDecisionMessage(result) } };
    }

    const parsed = parseMindlockArtifactArgs(normalized, "/reject");
    if (!parsed.artifactId) {
      return {
        shouldContinue: false,
        reply: { text: "Usage: /reject <artifact_id|index> [reason] (alias: /r)" },
      };
    }
    const result = await runVeriCoreMindlockReject(parsed.artifactId, stimulus, parsed.reason);
    return { shouldContinue: false, reply: { text: formatMindlockDecisionMessage(result) } };
  } catch (err) {
    const label = isReview
      ? "mindlock review failed"
      : isView
        ? "mindlock view failed"
        : isApprove
          ? "mindlock approve failed"
          : isReject
            ? "mindlock reject failed"
            : "mindlock command failed";
    return { shouldContinue: false, reply: { text: `${label}: ${String(err)}` } };
  }
};
