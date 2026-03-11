#!/usr/bin/env node

import fs from "node:fs";
import module from "node:module";
import path from "node:path";

const MIN_NODE_MAJOR = 22;
const MIN_NODE_MINOR = 12;
const MIN_NODE_VERSION = `${MIN_NODE_MAJOR}.${MIN_NODE_MINOR}`;

const parseNodeVersion = (rawVersion) => {
  const [majorRaw = "0", minorRaw = "0"] = rawVersion.split(".");
  return {
    major: Number(majorRaw),
    minor: Number(minorRaw),
  };
};

const isSupportedNodeVersion = (version) =>
  version.major > MIN_NODE_MAJOR ||
  (version.major === MIN_NODE_MAJOR && version.minor >= MIN_NODE_MINOR);

const ensureSupportedNodeVersion = () => {
  if (isSupportedNodeVersion(parseNodeVersion(process.versions.node))) {
    return;
  }

  process.stderr.write(
    `openclaw: Node.js v${MIN_NODE_VERSION}+ is required (current: v${process.versions.node}).\n` +
      "If you use nvm, run:\n" +
      "  nvm install 22\n" +
      "  nvm use 22\n" +
      "  nvm alias default 22\n",
  );
  process.exit(1);
};

ensureSupportedNodeVersion();

// https://nodejs.org/api/module.html#module-compile-cache
if (module.enableCompileCache && !process.env.NODE_DISABLE_COMPILE_CACHE) {
  try {
    module.enableCompileCache();
  } catch {
    // Ignore errors
  }
}

const isModuleNotFoundError = (err) =>
  err && typeof err === "object" && "code" in err && err.code === "ERR_MODULE_NOT_FOUND";

const installProcessWarningFilter = async () => {
  // Keep bootstrap warnings consistent with the TypeScript runtime.
  for (const specifier of ["./dist/warning-filter.js", "./dist/warning-filter.mjs"]) {
    try {
      const mod = await import(specifier);
      if (typeof mod.installProcessWarningFilter === "function") {
        mod.installProcessWarningFilter();
        return;
      }
    } catch (err) {
      if (isModuleNotFoundError(err)) {
        continue;
      }
      throw err;
    }
  }
};

await installProcessWarningFilter();

const hasExplicitStateOrConfigOverride = (env) =>
  Boolean(
    env.OPENCLAW_STATE_DIR?.trim() ||
      env.CLAWDBOT_STATE_DIR?.trim() ||
      env.OPENCLAW_CONFIG_PATH?.trim() ||
      env.CLAWDBOT_CONFIG_PATH?.trim() ||
      env.OPENCLAW_HOME?.trim(),
  );

const detectRepoStateDirFromCwd = (cwd = process.cwd()) => {
  let current = path.resolve(cwd);
  while (true) {
    const candidate = path.join(current, ".BGIseed-state", "openclaw.json");
    try {
      if (fs.existsSync(candidate)) {
        return path.dirname(candidate);
      }
    } catch {
      // Ignore inaccessible ancestors and keep walking upward.
    }
    const parent = path.dirname(current);
    if (parent === current) {
      return null;
    }
    current = parent;
  }
};

if (!hasExplicitStateOrConfigOverride(process.env)) {
  const detectedStateDir = detectRepoStateDirFromCwd();
  if (detectedStateDir) {
    process.env.OPENCLAW_STATE_DIR = detectedStateDir;
  }
}

const tryImport = async (specifier) => {
  try {
    await import(specifier);
    return true;
  } catch (err) {
    // Only swallow missing-module errors; rethrow real runtime errors.
    if (isModuleNotFoundError(err)) {
      return false;
    }
    throw err;
  }
};

if (await tryImport("./dist/entry.js")) {
  // OK
} else if (await tryImport("./dist/entry.mjs")) {
  // OK
} else {
  throw new Error("openclaw: missing dist/entry.(m)js (build output).");
}
