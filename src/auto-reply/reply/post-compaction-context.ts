import fs from "node:fs";
import path from "node:path";
import { resolveCronStyleNow } from "../../agents/current-time.js";
import { resolveUserTimezone } from "../../agents/date-time.js";
import type { OpenClawConfig } from "../../config/config.js";
import { openBoundaryFile } from "../../infra/boundary-file-read.js";

const MAX_CONTEXT_CHARS = 3000;
const CRITICAL_WORKSPACE_FILES = ["AGENTS.md", "SOUL.md"] as const;

type CriticalWorkspaceFile = (typeof CRITICAL_WORKSPACE_FILES)[number];

function formatDateStamp(nowMs: number, timezone: string): string {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone: timezone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(new Date(nowMs));
  const year = parts.find((p) => p.type === "year")?.value;
  const month = parts.find((p) => p.type === "month")?.value;
  const day = parts.find((p) => p.type === "day")?.value;
  if (year && month && day) {
    return `${year}-${month}-${day}`;
  }
  return new Date(nowMs).toISOString().slice(0, 10);
}

async function readWorkspaceCriticalFile(
  workspaceDir: string,
  fileName: CriticalWorkspaceFile,
): Promise<string | null> {
  const filePath = path.join(workspaceDir, fileName);
  try {
    const opened = await openBoundaryFile({
      absolutePath: filePath,
      rootPath: workspaceDir,
      boundaryLabel: "workspace root",
    });
    if (!opened.ok) {
      return null;
    }
    return (() => {
      try {
        return fs.readFileSync(opened.fd, "utf-8");
      } finally {
        fs.closeSync(opened.fd);
      }
    })();
  } catch {
    return null;
  }
}

export async function readLeanWorkspaceIdentityContext(params: {
  workspaceDir: string;
  maxChars?: number;
  cfg?: OpenClawConfig;
  nowMs?: number;
}): Promise<string | null> {
  const resolvedNowMs = params.nowMs ?? Date.now();
  const timezone = resolveUserTimezone(params.cfg?.agents?.defaults?.userTimezone);
  const dateStamp = formatDateStamp(resolvedNowMs, timezone);
  const sections: string[] = [];

  for (const fileName of CRITICAL_WORKSPACE_FILES) {
    const content = await readWorkspaceCriticalFile(params.workspaceDir, fileName);
    const trimmed = content?.trim();
    if (!trimmed) {
      continue;
    }
    sections.push(`## ${fileName}\n\n${trimmed.replaceAll("YYYY-MM-DD", dateStamp)}`);
  }

  if (sections.length === 0) {
    return null;
  }

  const maxChars = params.maxChars ?? MAX_CONTEXT_CHARS;
  const combined = sections.join("\n\n");
  return combined.length > maxChars
    ? combined.slice(0, maxChars) + "\n...[truncated]..."
    : combined;
}

/**
 * Read lean workspace identity files for post-compaction injection.
 * Returns formatted system event text, or null if neither AGENTS.md nor SOUL.md is readable.
 * Substitutes YYYY-MM-DD placeholders with the real date so agents read the correct
 * daily memory files instead of guessing based on training cutoff.
 */
export async function readPostCompactionContext(
  workspaceDir: string,
  cfg?: OpenClawConfig,
  nowMs?: number,
): Promise<string | null> {
  const safeContent = await readLeanWorkspaceIdentityContext({
    workspaceDir,
    maxChars: MAX_CONTEXT_CHARS,
    cfg,
    nowMs,
  });
  if (!safeContent) {
    return null;
  }

  const resolvedNowMs = nowMs ?? Date.now();
  const { timeLine } = resolveCronStyleNow(cfg ?? {}, resolvedNowMs);

  return (
    "[Post-compaction context refresh]\n\n" +
    "Session was just compacted. The conversation summary above is a hint, NOT a substitute for your startup sequence. " +
    "Re-read the lean workspace identity files before responding to the user.\n\n" +
    `Critical workspace identity files:\n\n${safeContent}\n\n${timeLine}`
  );
}

/**
 * Extract named sections from markdown content.
 * Matches H2 (##) or H3 (###) headings case-insensitively.
 * Skips content inside fenced code blocks.
 * Captures until the next heading of same or higher level, or end of string.
 */
export function extractSections(content: string, sectionNames: string[]): string[] {
  const results: string[] = [];
  const lines = content.split("\n");

  for (const name of sectionNames) {
    let sectionLines: string[] = [];
    let inSection = false;
    let sectionLevel = 0;
    let inCodeBlock = false;

    for (const line of lines) {
      if (line.trimStart().startsWith("```")) {
        inCodeBlock = !inCodeBlock;
        if (inSection) {
          sectionLines.push(line);
        }
        continue;
      }

      if (inCodeBlock) {
        if (inSection) {
          sectionLines.push(line);
        }
        continue;
      }

      const headingMatch = line.match(/^(#{2,3})\s+(.+?)\s*$/);

      if (headingMatch) {
        const level = headingMatch[1].length;
        const headingText = headingMatch[2];

        if (!inSection) {
          if (headingText.toLowerCase() === name.toLowerCase()) {
            inSection = true;
            sectionLevel = level;
            sectionLines = [line];
            continue;
          }
        } else {
          if (level <= sectionLevel) {
            break;
          }
          sectionLines.push(line);
          continue;
        }
      }

      if (inSection) {
        sectionLines.push(line);
      }
    }

    if (sectionLines.length > 0) {
      results.push(sectionLines.join("\n").trim());
    }
  }

  return results;
}
