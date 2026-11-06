#!/usr/bin/env node
// Validate a Subo language pack the same way Subo's importer does.
//   node scripts/validate.mjs <pack.subo-locale.json>
// The key structure comes from ../reference/zh-CN.subo-locale.json. Exit code 1 on errors.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ref = (name) => JSON.parse(readFileSync(join(here, "..", "reference", name), "utf8"));

const CODE = /^[A-Za-z]{2,3}(-[A-Za-z0-9]{1,8})*$/;
const PLURAL = new Set(["zero", "one", "two", "few", "many", "other"]);

const errors = [];
const warnings = [];
const info = [];

const file = process.argv[2];
if (!file) {
  console.error("usage: node scripts/validate.mjs <pack.subo-locale.json>");
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

if (pack.format === "subo-locale") locale(pack);
else if (pack.format === "subo-theme") errors.push(`this is a color theme — use the subo-theme skill's validator`);
else errors.push(`"format" must be "subo-locale"`);

function common(p, idField, re) {
  if (p.version !== 1) errors.push(`"version" must be 1`);
  if (!text(p[idField], 64) || !re.test(p[idField])) errors.push(`"${idField}" is invalid: ${JSON.stringify(p[idField])}`);
  if (!text(p.name, 80)) errors.push(`"name" is required (≤ 80 chars)`);
  if (p.author != null && !text(p.author, 120)) warnings.push(`"author" ignored (string ≤ 120 chars)`);
}

function isLeaf(v) {
  return typeof v === "string" || (v && typeof v === "object" && "other" in v);
}
function forms(v) {
  return typeof v === "string" ? [v] : Object.values(v);
}
function holes(s) {
  return [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]);
}
function leaves(v) {
  return isLeaf(v) ? 1 : v && typeof v === "object" ? Object.values(v).reduce((n, x) => n + leaves(x), 0) : 0;
}

function locale(p) {
  common(p, "code", CODE);
  const builtin = readdirSync(join(here, "..", "reference")).flatMap((f) => f.match(/^(.+)\.subo-locale\.json$/)?.[1] ?? []);
  const fallback = builtin.includes(p.fallback) ? p.fallback : "en";
  if (p.fallback != null && !builtin.includes(p.fallback)) warnings.push(`"fallback" should be a built-in language (${builtin.join(", ")}); will default to en`);
  if (!p.messages || typeof p.messages !== "object") {
    errors.push(`"messages" must be an object`);
    return;
  }
  const base = ref("zh-CN.subo-locale.json").messages;
  const count = [0, 0];
  walk(base, p.messages, "", count);
  if (!count[0]) errors.push("no valid messages");
  const pct = count[1] ? Math.floor((count[0] / count[1]) * 1000) / 10 : 0;
  info.push(`coverage: ${count[0]} / ${count[1]} (${pct}%) — the rest falls back to ${fallback}`);
  if (p.packages != null) packages(p.packages);
}

// Optional package descriptions: name → plain string. Not counted in coverage.
function packages(v) {
  if (typeof v !== "object" || Array.isArray(v)) {
    warnings.push(`"packages" must be an object (package name → description) — will be dropped`);
    return;
  }
  const NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
  let ok = 0;
  for (const [name, desc] of Object.entries(v)) {
    if (!NAME.test(name)) warnings.push(`packages.${name}: not a TeX Live package name — will be dropped`);
    else if (!text(desc, 300)) warnings.push(`packages.${name}: must be a non-empty string ≤ 300 chars — will be dropped`);
    else ok++;
  }
  let en = null;
  try {
    en = ref("packages.en.json");
  } catch {
    /* the reference is optional */
  }
  const of = en ? Object.keys(en).filter((n) => n in v).length : 0;
  info.push(en
    ? `package descriptions: ${ok} (${of} / ${Object.keys(en).length} installed packages) — the rest show English`
    : `package descriptions: ${ok} — the rest show English`);
}

function walk(ref, src, path, count) {
  for (const [k, rv] of Object.entries(ref)) {
    const at = path ? `${path}.${k}` : k;
    if (!(k in src)) {
      count[1] += leaves(rv);
      continue;
    }
    const v = src[k];
    if (isLeaf(rv)) {
      count[1]++;
      const ok =
        (typeof v === "string" && v.length <= 4000) ||
        (v && typeof v === "object" && !Array.isArray(v) && "other" in v &&
          Object.entries(v).every(([f, s]) => PLURAL.has(f) && typeof s === "string" && s.length <= 4000));
      if (!ok) {
        warnings.push(`${at}: must be a string or a plural object with "other" — will be dropped`);
        continue;
      }
      const allowed = new Set([...forms(rv).flatMap(holes), "count"]);
      const extra = [...new Set(forms(v).flatMap(holes))].filter((h) => !allowed.has(h));
      if (extra.length) warnings.push(`${at}: unknown placeholder(s) {${extra.join("}, {")}} — allowed: ${[...allowed].map((h) => `{${h}}`).join(" ")}`);
      count[0]++;
    } else if (v && typeof v === "object" && !Array.isArray(v)) {
      walk(rv, v, at, count);
    } else {
      warnings.push(`${at}: must be an object (group) — will be dropped`);
      count[1] += leaves(rv);
    }
  }
  for (const k of Object.keys(src)) if (!(k in ref)) warnings.push(`${path ? `${path}.${k}` : k}: unknown key, will be dropped`);
}

for (const m of info) console.log(`  ${m}`);
for (const m of warnings) console.log(`⚠ ${m}`);
for (const m of errors) console.log(`✗ ${m}`);
console.log(errors.length ? `✗ ${errors.length} error(s)` : warnings.length ? `✓ valid, ${warnings.length} warning(s)` : "✓ valid");
process.exit(errors.length ? 1 : 0);
