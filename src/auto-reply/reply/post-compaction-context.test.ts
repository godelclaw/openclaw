import fs from "node:fs";
import path from "node:path";
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import type { OpenClawConfig } from "../../config/config.js";
import {
  readLeanWorkspaceIdentityContext,
  readPostCompactionContext,
} from "./post-compaction-context.js";

describe("readPostCompactionContext", () => {
  const tmpDir = path.join("/tmp", "test-post-compaction-" + Date.now());

  beforeEach(() => {
    fs.mkdirSync(tmpDir, { recursive: true });
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it("returns null when neither AGENTS.md nor SOUL.md exists", async () => {
    const result = await readPostCompactionContext(tmpDir);
    expect(result).toBeNull();
  });

  it("includes lean AGENTS.md content even without special sections", async () => {
    fs.writeFileSync(path.join(tmpDir, "AGENTS.md"), "# My Agent\n\nKeep this short.\n");
    const result = await readPostCompactionContext(tmpDir);
    expect(result).not.toBeNull();
    expect(result).toContain("## AGENTS.md");
    expect(result).toContain("Keep this short.");
  });

  it("includes SOUL.md alongside AGENTS.md when both exist", async () => {
    fs.writeFileSync(path.join(tmpDir, "AGENTS.md"), "# AGENTS\n\nRule A\n");
    fs.writeFileSync(path.join(tmpDir, "SOUL.md"), "# SOUL\n\nPersona B\n");
    const result = await readPostCompactionContext(tmpDir);
    expect(result).not.toBeNull();
    expect(result).toContain("## AGENTS.md");
    expect(result).toContain("Rule A");
    expect(result).toContain("## SOUL.md");
    expect(result).toContain("Persona B");
  });

  it("also works when only SOUL.md exists", async () => {
    fs.writeFileSync(path.join(tmpDir, "SOUL.md"), "# SOUL\n\nPersona only\n");
    const result = await readPostCompactionContext(tmpDir);
    expect(result).not.toBeNull();
    expect(result).toContain("## SOUL.md");
    expect(result).toContain("Persona only");
  });

  it("truncates when combined lean identity content exceeds limit", async () => {
    fs.writeFileSync(path.join(tmpDir, "AGENTS.md"), "A".repeat(2500));
    fs.writeFileSync(path.join(tmpDir, "SOUL.md"), "B".repeat(2500));
    const result = await readLeanWorkspaceIdentityContext({
      workspaceDir: tmpDir,
      maxChars: 2000,
    });
    expect(result).not.toBeNull();
    expect(result).toContain("[truncated]");
  });

  it("substitutes YYYY-MM-DD with the actual date in lean identity files", async () => {
    fs.writeFileSync(
      path.join(tmpDir, "AGENTS.md"),
      "Read memory/YYYY-MM-DD.md before replying.\n",
    );
    const cfg = {
      agents: { defaults: { userTimezone: "America/New_York", timeFormat: "12" } },
    } as OpenClawConfig;
    const nowMs = Date.UTC(2026, 2, 3, 14, 0, 0);
    const result = await readPostCompactionContext(tmpDir, cfg, nowMs);
    expect(result).not.toBeNull();
    expect(result).toContain("memory/2026-03-03.md");
    expect(result).not.toContain("memory/YYYY-MM-DD.md");
    expect(result).toContain(
      "Current time: Tuesday, March 3rd, 2026 — 9:00 AM (America/New_York) / 2026-03-03 14:00 UTC",
    );
  });

  it.runIf(process.platform !== "win32")(
    "returns null when AGENTS.md is a symlink escaping workspace and no other identity file exists",
    async () => {
      const outsideDir = fs.mkdtempSync(path.join("/tmp", "post-compaction-outside-"));
      try {
        const outside = path.join(outsideDir, "outside-secret.txt");
        fs.writeFileSync(outside, "secret");
        fs.symlinkSync(outside, path.join(tmpDir, "AGENTS.md"));

        const result = await readPostCompactionContext(tmpDir);
        expect(result).toBeNull();
      } finally {
        fs.rmSync(outsideDir, { recursive: true, force: true });
      }
    },
  );

  it.runIf(process.platform !== "win32")(
    "returns null when AGENTS.md is a hardlink alias and no other identity file exists",
    async () => {
      const outsideDir = fs.mkdtempSync(path.join("/tmp", "post-compaction-outside-"));
      try {
        const outside = path.join(outsideDir, "outside-secret.txt");
        fs.writeFileSync(outside, "secret");
        fs.linkSync(outside, path.join(tmpDir, "AGENTS.md"));

        const result = await readPostCompactionContext(tmpDir);
        expect(result).toBeNull();
      } finally {
        fs.rmSync(outsideDir, { recursive: true, force: true });
      }
    },
  );
});
