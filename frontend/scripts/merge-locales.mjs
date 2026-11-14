// Merge per-screen locale bundles into src/locales/{en,ur,hi}.json.
//
// Each screen keeps its strings in src/screens/<Name>/strings.{en,ur,hi}.json
// (namespaced keys, e.g. `tuning.*`). This script deep-merges them into the
// app bundles that src/lib/i18n.ts loads. Re-run after editing any screen's
// strings, and before committing.
//
// Fails loudly on: key collisions (same dotted path, different values),
// locale key-set mismatches (a screen missing a key in any locale), or a
// single-brace `{var}` interpolation (i18next needs `{{var}}`).
//
// Usage: node scripts/merge-locales.mjs [--check]   (--check: verify only)

import { readFileSync, writeFileSync, readdirSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const LOCALES = ["en", "ur", "hi"];

function loadJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

/** Collect dotted leaf paths of a nested object. */
function leafPaths(obj, prefix = "") {
  const out = [];
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value !== null && typeof value === "object") out.push(...leafPaths(value, path));
    else out.push(path);
  }
  return out;
}

function getPath(obj, path) {
  return path.split(".").reduce((acc, key) => acc?.[key], obj);
}

function setPath(obj, path, value) {
  const keys = path.split(".");
  let node = obj;
  for (const key of keys.slice(0, -1)) {
    node[key] ??= {};
    node = node[key];
  }
  node[keys[keys.length - 1]] = value;
}

/** Single-brace `{var}` (not `{{var}}`) is never interpolated by i18next. */
function checkInterpolation(obj, file, path = "") {
  for (const [key, value] of Object.entries(obj)) {
    const at = path ? `${path}.${key}` : key;
    if (typeof value === "string") {
      const singles = value.match(/(?<!\{)\{[a-zA-Z_][a-zA-Z0-9_]*\}(?!\})/g);
      if (singles) {
        throw new Error(`${file} ${at}: single-brace interpolation ${singles.join(", ")} — use {{var}}`);
      }
    } else if (value !== null && typeof value === "object") {
      checkInterpolation(value, file, at);
    }
  }
}

const checkOnly = process.argv.includes("--check");
const merged = {};
for (const locale of LOCALES) {
  merged[locale] = loadJson(join(root, "src", "locales", `${locale}.json`));
}

const screensDir = join(root, "src", "screens");
const screens = readdirSync(screensDir, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort();

for (const screen of screens) {
  const bundles = {};
  for (const locale of LOCALES) {
    const file = join(screensDir, screen, `strings.${locale}.json`);
    if (!existsSync(file)) continue;
    bundles[locale] = loadJson(file);
    checkInterpolation(bundles[locale], file);
  }
  const present = Object.keys(bundles);
  if (present.length === 0) continue;
  if (present.length !== LOCALES.length) {
    throw new Error(`${screen}: strings files missing for locales: ${LOCALES.filter((l) => !present.includes(l)).join(", ")}`);
  }
  // Key-set parity across the screen's three locale files.
  const keySets = LOCALES.map((locale) => leafPaths(bundles[locale]).sort().join("\n"));
  if (new Set(keySets).size !== 1) {
    const all = new Set(keySets.flatMap((s) => s.split("\n")));
    const missing = LOCALES.map((locale, i) => {
      const have = new Set(keySets[i].split("\n"));
      return `${locale}: missing ${[...all].filter((k) => !have.has(k)).join(", ") || "—"}`;
    });
    throw new Error(`${screen}: locale key sets differ:\n${missing.join("\n")}`);
  }
  for (const locale of LOCALES) {
    for (const path of leafPaths(bundles[locale])) {
      const value = getPath(bundles[locale], path);
      const existing = getPath(merged[locale], path);
      if (existing !== undefined && existing !== value) {
        throw new Error(`key collision: ${path} (${locale}) already in src/locales with a different value`);
      }
      setPath(merged[locale], path, value);
    }
  }
  console.log(`merged ${screen} (${leafPaths(bundles.en).length} keys)`);
}

// Final parity: the three app bundles must have identical key sets.
const finalSets = LOCALES.map((locale) => leafPaths(merged[locale]).sort().join("\n"));
if (new Set(finalSets).size !== 1) {
  throw new Error("src/locales key sets differ after merge — this is a bug in the merge script");
}

if (checkOnly) {
  console.log("locale merge check OK");
  process.exit(0);
}

for (const locale of LOCALES) {
  writeFileSync(
    join(root, "src", "locales", `${locale}.json`),
    JSON.stringify(merged[locale], null, 2) + "\n",
  );
}
console.log(`wrote src/locales/{${LOCALES.join(",")}} (${leafPaths(merged.en).length} keys each)`);
