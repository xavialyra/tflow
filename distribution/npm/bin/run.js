#!/usr/bin/env node
const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");

function resolveBinary() {
  // 1. Explicit environment variable override
  if (process.env.TLAUNCH_BIN && fs.existsSync(process.env.TLAUNCH_BIN)) {
    return process.env.TLAUNCH_BIN;
  }

  // 2. Check local builds (during dev or package bundled binaries)
  const localCandidates = [
    path.join(__dirname, "../../../target/release/tlaunch"),
    path.join(__dirname, "../../../target/debug/tlaunch"),
    path.join(__dirname, `../binaries/tlaunch-${os.platform()}-${os.arch()}`),
    path.join(__dirname, "tlaunch"),
  ];

  for (const candidate of localCandidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  // 3. Search system PATH
  const pathDirs = (process.env.PATH || "").split(path.delimiter);
  for (const dir of pathDirs) {
    const full = path.join(dir, "tlaunch");
    if (fs.existsSync(full)) {
      return full;
    }
  }

  return null;
}

const binary = resolveBinary();
if (!binary) {
  console.error("\x1b[31m[!] tlaunch binary not found.\x1b[0m");
  console.error("Please ensure tlaunch is installed or set TLAUNCH_BIN to its path.");
  console.error("Visit https://github.com/tlaunch/tlaunch for prebuilt releases.");
  process.exit(1);
}

const env = {
  ...process.env,
  TLAUNCH_BIN: binary,
};

// Pass all arguments and inherit stdio (essential for piping stdin into -w - and tty interaction)
const result = spawnSync(binary, process.argv.slice(2), {
  stdio: "inherit",
  env,
});

if (result.error) {
  console.error(`Failed to start tlaunch: ${result.error.message}`);
  process.exit(1);
}

process.exit(result.status !== null ? result.status : 0);
