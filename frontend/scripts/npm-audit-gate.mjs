// Frontend supply-chain gate (SECURITY-INPUT §9 / TM-R03).
//
// Fails on any High/Critical npm advisory EXCEPT the documented allowlist
// below. The allowlist exists only for advisories with NO upstream fix
// available — never for convenience. Each entry names the advisory, why it
// cannot be fixed by a dependency update, and when the entry must be
// removed. Any High/Critical finding outside the allowlist fails the gate,
// as does an unparseable/missing audit report (fail closed).
//
// Usage: node scripts/npm-audit-gate.mjs
//        node scripts/npm-audit-gate.mjs --audit-json <file>   (offline test:
//        check a saved `npm audit --json` report instead of running npm)
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const ALLOWED_GHSA = new Set([
  // GHSA-fc5q-39xc-vjpf (CVE-2026-112366): picomatch ReDoS via
  // single-branch repeated extglobs, reachable through
  // vitest -> vite -> tinyglobby -> fdir -> picomatch (dev-only).
  // No fixed picomatch release exists yet: latest 4.0.7 is vulnerable
  // and the fix (micromatch/picomatch PR #194) is unreleased as of
  // 2026-11-18, so no dependency update can clear this advisory, and
  // `npm audit fix --force` only proposes a bogus vitest 4 -> 3
  // downgrade that stays inside the vulnerable range. Risk in this
  // tree is nil: picomatch runs dev-time only (vite/vitest file
  // globbing) over trusted developer-controlled patterns, and the
  // frontend is dev-only/unshipped. REMOVE this entry when picomatch
  // ships a fixed release.
  "GHSA-FC5Q-39XC-VJPF",
]);

// The allowlist matches ADVISORY ids, but npm audit's JSON reports one
// entry per package in the affected tree, and only the root vulnerable
// package carries the advisory object in `via`. Transitive entries
// (fdir, vite, vitest, ...) list only dependency NAME strings in `via`
// (e.g. fdir's `via` is ["picomatch"]). So an entry is explained by
// resolving its `via` name-chain back to the root package(s) that carry
// the advisory objects. An entry is allowed only if that resolution
// yields at least one advisory id AND every id is allowlisted; an
// unresolvable entry (no advisory id anywhere up its chain) fails
// closed, as does any entry whose chain reaches a second, non-allowlisted
// advisory.
function resolveGhsaIds(name, vulns, visited = new Set()) {
  if (visited.has(name)) return new Set();
  visited.add(name);
  const vuln = vulns[name];
  if (!vuln) return new Set();
  const ids = new Set();
  const via = Array.isArray(vuln.via) ? vuln.via : [];
  for (const entry of via) {
    if (entry && typeof entry === "object" && typeof entry.url === "string") {
      const m = entry.url.match(/GHSA-[a-z0-9-]+/i);
      if (m) ids.add(m[0].toUpperCase());
    } else if (typeof entry === "string") {
      for (const id of resolveGhsaIds(entry, vulns, visited)) ids.add(id);
    }
  }
  return ids;
}

function loadAudit(argv) {
  const flag = argv.indexOf("--audit-json");
  if (flag !== -1 && argv[flag + 1]) {
    try {
      return JSON.parse(readFileSync(argv[flag + 1], "utf8"));
    } catch (err) {
      console.error(`cannot read audit JSON file ${argv[flag + 1]}: ${err.message}; failing closed.`);
      process.exit(1);
    }
  }
  try {
    const out = execFileSync("npm", ["audit", "--json"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return JSON.parse(out);
  } catch (err) {
    // `npm audit` exits 1 when findings exist but still prints the
    // JSON report on stdout — use it. Anything else is unparseable:
    // fail closed rather than passing on a missing report.
    const out = typeof err.stdout === "string" ? err.stdout : "";
    if (out) {
      try {
        return JSON.parse(out);
      } catch {
        // fall through to the fail-closed error below
      }
    }
    console.error("npm audit did not return a parseable JSON report; failing closed.");
    if (err.stderr) console.error(String(err.stderr).trim());
    process.exit(1);
  }
}

const report = loadAudit(process.argv.slice(2));
const vulns = report.vulnerabilities ?? {};
const counts = report.metadata?.vulnerabilities ?? {};
console.log(
  `npm audit: total=${counts.total ?? "?"} info=${counts.info ?? 0} low=${counts.low ?? 0} ` +
    `moderate=${counts.moderate ?? 0} high=${counts.high ?? 0} critical=${counts.critical ?? 0}`,
);

const offenders = [];
for (const [name, vuln] of Object.entries(vulns)) {
  if (vuln.severity !== "high" && vuln.severity !== "critical") continue;
  const ids = resolveGhsaIds(name, vulns);
  const unallowed = [...ids].filter((id) => !ALLOWED_GHSA.has(id));
  if (ids.size === 0 || unallowed.length > 0) {
    offenders.push({ name, severity: vuln.severity, ids: [...ids] });
  } else {
    console.log(`allowed (no upstream fix, see ALLOWED_GHSA): ${name} (${vuln.severity}) via ${[...ids].join(", ")}`);
  }
}

if (offenders.length > 0) {
  console.error(`FAIL: ${offenders.length} High/Critical finding(s) outside the allowlist:`);
  for (const o of offenders) {
    console.error(`  - ${o.name} (${o.severity}): ${o.ids.join(", ") || "(no advisory id)"}`);
  }
  process.exit(1);
}
console.log("npm audit gate: PASS (no High/Critical findings outside the allowlist)");
