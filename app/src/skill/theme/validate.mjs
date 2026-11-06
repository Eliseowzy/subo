#!/usr/bin/env node
// Validate a Subo color theme the same way Subo's importer does.
//   node scripts/validate.mjs <theme.subo-theme.json>
// Allowed tokens come from ../reference/default.subo-theme.json. Exit code 1 on errors.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ref = (name) => JSON.parse(readFileSync(join(here, "..", "reference", name), "utf8"));

const FORBIDDEN = /[;{}<>\\]|url\s*\(|@import|expression\s*\(/i;
const ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

const errors = [];
const warnings = [];
const info = [];

const file = process.argv[2];
if (!file) {
  console.error("usage: node scripts/validate.mjs <theme.subo-theme.json>");
  process.exit(2);
}
let pack;
try {
  pack = JSON.parse(readFileSync(file, "utf8"));
} catch (e) {
  console.error(`✗ not valid JSON: ${e.message}`);
  process.exit(1);
}

const text = (v, max) => typeof v === "string" && v.trim() && v.length <= max;

if (pack.format === "subo-theme") theme(pack);
else if (pack.format === "subo-locale") errors.push(`this is a language pack — use the subo-locale skill's validator`);
else errors.push(`"format" must be "subo-theme"`);

function common(p, idField, re) {
  if (p.version !== 1) errors.push(`"version" must be 1`);
  if (!text(p[idField], 64) || !re.test(p[idField])) errors.push(`"${idField}" is invalid: ${JSON.stringify(p[idField])}`);
  if (!text(p.name, 80)) errors.push(`"name" is required (≤ 80 chars)`);
  if (p.author != null && !text(p.author, 120)) warnings.push(`"author" ignored (string ≤ 120 chars)`);
}

function theme(p) {
  common(p, "id", ID);
  if (p.id === "default") errors.push(`"id" must not be "default" (the built-in theme)`);
  const d = ref("default.subo-theme.json");
  const names = new Set([...Object.keys(d.base ?? {}), ...Object.keys(d.light ?? {}), ...Object.keys(d.dark ?? {})]);
  let total = 0;
  for (const sec of ["base", "light", "dark"]) {
    const s = p[sec];
    if (s == null) continue;
    if (typeof s !== "object" || Array.isArray(s)) {
      errors.push(`"${sec}" must be an object`);
      continue;
    }
    let n = 0;
    for (const [k0, v] of Object.entries(s)) {
      const k = k0.replace(/^--/, "");
      if (!names.has(k)) warnings.push(`${sec}.${k}: unknown token, will be dropped`);
      else if (typeof v !== "string" || !v.trim() || v.length > 300) warnings.push(`${sec}.${k}: value must be a non-empty string ≤ 300 chars`);
      else if (FORBIDDEN.test(v)) warnings.push(`${sec}.${k}: contains url(), @import, ; { } < > or \\ — will be dropped`);
      else n++;
    }
    total += n;
    info.push(`${sec}: ${n} token(s)`);
  }
  if (!total) errors.push("no valid tokens in base / light / dark");
  if (p.light && !p.dark) warnings.push("no dark section: dark mode will use Subo's built-in dark colors");
  if (p.dark && !p.light) warnings.push("no light section: light mode will use Subo's built-in light colors");
}

for (const m of info) console.log(`  ${m}`);
for (const m of warnings) console.log(`⚠ ${m}`);
for (const m of errors) console.log(`✗ ${m}`);
console.log(errors.length ? `✗ ${errors.length} error(s)` : warnings.length ? `✓ valid, ${warnings.length} warning(s)` : "✓ valid");
process.exit(errors.length ? 1 : 0);
