---
name: subo-locale
description: Create or modify interface language packs (.subo-locale.json) for Subo, the offline LaTeX workbench. Use when the user wants Subo translated into a new language, or wants to tweak the wording of an existing Subo language.
---

# Subo language packs

Subo (素帛) is an offline desktop LaTeX editor. A language pack is a plain JSON file
(`<code>.subo-locale.json`) holding every piece of interface text. Users import it in
**Settings → General → Language packs → Import**.

Talk with the user in their own language. Deliver a single JSON file they can save and import.
Color themes are a separate skill (`subo-theme`); this one only covers languages.

## Files in this skill

```
reference/zh-CN.subo-locale.json    base language: the authoritative key structure
reference/en.subo-locale.json       English: the best source text to translate from
reference/<code>.subo-locale.json   any other language built into this Subo version
reference/packages.en.json          English one-line descriptions of the user's installed TeX packages
                                    (present when Subo could list them)
scripts/validate.mjs                checks a pack exactly like Subo's importer does
```

The reference files were exported from the user's own copy of Subo, so they match their version.
If the user gives you a pack they exported from Subo, start from that file instead.

Always finish by running the validator (Node 18+, no dependencies):

```bash
node scripts/validate.mjs path/to/pack.subo-locale.json
```

Fix every error. Warnings mean something will be dropped on import; fix those too unless the
user agrees. If you cannot run code, check the rules below by hand.

## Language pack format

```json
{
  "format": "subo-locale",
  "version": 1,
  "code": "ja",
  "name": "日本語",
  "author": "optional",
  "fallback": "en",
  "messages": { "common": { "cancel": "キャンセル" } },
  "packages": { "amsmath": "AMS の数式組版機能" }
}
```

- `code`: a BCP 47 tag (`ja`, `fr`, `zh-TW`, `pt-BR`). Subo uses it for `<html lang>`, plural
  rules and date formatting. Using a code Subo already ships (`zh-CN`, `en`) **overrides** that
  language. Use this to reword a few strings; anything you leave out keeps the built-in text.
- `name`: the language's name written in that language (`日本語`, not "Japanese").
- `fallback` (optional): built-in language used for keys you did not translate. Default `en`.
- `messages`: same nested structure as `reference/zh-CN.subo-locale.json`. Keys outside that
  structure are dropped. A partial pack is valid, and Subo reports coverage on import.
- `packages` (optional): TeX package name → one-line description, shown in
  **Settings → Installed packages**. Any package you leave out shows the English description from
  TeX Live. `fallback` does not apply here. A pack that overrides a built-in language (`zh-CN`,
  `zh-TW`) keeps that language's built-in descriptions for packages it does not list.

### Package descriptions

Only translate them if the user asks for it. When you do:

- Translate from `reference/packages.en.json`. The built-in `zh-CN` and `zh-TW` references show
  the style: short noun phrases, about one line.
- Keys are exact TeX Live package names (`tikz-cd`, `biblatex-gb7714-2015`). Never translate or
  change them. Unknown names are kept, so you can also describe packages the user installs later.
- Keep package, font, program and standard names as they are (`TikZ`, `BibTeX`, `GB/T 7714`), as
  well as TeX commands (`\shipout`). Plain strings only: no placeholders, no plurals, ≤ 300 chars.

### Translation rules

1. **Never rename or translate keys**, only values. Keep the nesting exactly.
2. Keep `{placeholders}` verbatim (`{name}`, `{count}`, `{detail}`...). You may reorder them and
   drop ones the sentence doesn't need. Never invent new ones.
3. Plurals: a value can be `{ "one": "...", "other": "..." }` using CLDR categories
   (`zero one two few many other`, `other` required). Use the categories the target language
   actually has. Languages without plural inflection (ja, zh, ko) use a plain string.
4. `editor.phrases`: keys are CodeMirror's English source strings. Translate the values only.
5. `latex.commands` / `latex.envs`: keys are LaTeX command and environment names. Translate the
   descriptions only.
6. Keep `⌘`, `↵`, `⇧` symbols as they are (Subo maps them to Ctrl on Windows/Linux). Keep
   product names (Subo, TeX Live, Overleaf, XeLaTeX, SyncTeX) untranslated.
7. `brand.primary`/`brand.secondary`/`brand.windowTitle`: the product name. Non-Chinese locales
   normally use `"Subo"`, `""`, `"Subo"`, as the English pack does.
8. Keep interface text short. Button labels should be about as long as the English ones.
   Match the tone of the English pack: plain, calm, and not overly formal.
9. Translate from `en` unless the user prefers Chinese as the source. Use `zh-CN` to resolve
   ambiguity because it is the original.
10. Large packs (~600 strings): work group by group, run the validator, and report coverage.
    Package descriptions add a few hundred more strings. Do them last, in batches.

## Hand-off

Give the user the file and say:

1. Save it as `<code>.subo-locale.json`.
2. In Subo: **Settings → General → Language packs → Import**. The language is switched to right
   away. Re-importing the same code updates it.
3. To share or back up all imported language packs, use **Export all** in the same section.
