Understand the meaning, and write sentences that convey essentially the same message, but are written in the native way in the target language:
- Use native grammar and sentence structure to match target language conventions.
— use local idioms and expressions instead of literal translations.

## Supported Languages
- **German (DE)** Berlin/Germany variant only
- **English (EN)** NYC/US variant only
- **French (FR)** - Paris/France variant only
- **Italian (IT)** - Rome/Italy variant only
- **Spanish (ES)** - Madrid/Spain variant only
- **Portuguese (PT)** - Lisbon/Portugal variant only
- **Dutch (NL)** - Amsterdam/Netherlands variant only
- **Polish (PL)** - Warsaw/Poland variant only
- **Russian (RU)** - Moscow/Russian variant only

**Note**: NEVER use non-European variants of Spanish, French, and Portugese.

## Translation Files
- Location: `config/locales/`
- Pattern: `[component].[language_code].yml`
- Language codes: DE, EN, FR, IT, ES, PT, NL, PL, RU
- Default fallback: English (EN)

## Conventions — DO NOT BREAK

### Never pass `default:` to `t(...)` or `I18n.t(...)`
Every language must carry a real translation. A `default: 'English fallback'`
argument hides the fact that other locales were skipped, so a user browsing in
German or Polish sees English silently. If you add a new translation key:

1. Add the key to **every** `base.<locale>.yml` (or the appropriate
   per-component `[name].<locale>.yml`) with a native-language value.
2. Call it as `t('namespace.key')` without `default:`.

Exception for "soft lookup" where the key may legitimately be absent (e.g. the
exchange has no instructions yet): use `I18n.exists?('...')` to probe, then
call `t('...')` only when it exists. Do NOT use `default: nil` for this.

For dynamic keys (`tax_report.summary.tax_#{pct}`, `progress_steps.#{step}`,
etc.), either enumerate every possible value in every locale, or refactor to a
single parameterized key (`tax_report.summary.tax_percent` with interpolation
`%{pct}`). Don't hide the gap with `default: "Tax (#{pct}%)"`.

### YAML style for locale files
- For single-line values, use inline quoted strings: `key: 'Value'` or
  `key: "Value"`. Match the sibling keys' quoting style.
- For genuinely multi-line content (multi-paragraph HTML copy, long
  instructions, ads), prefer the `|` block scalar — it's the lightest, most
  readable option and preserves line breaks:

  ```yaml
  dca_profit_html: |
      <p>If you were DCA-ing into Bitcoin for the last %{years} years…</p>
      <p>That's <b>%{profit}%</b>…</p>
  ```

  Do NOT fake multi-line with `\n` escapes inside a double-quoted string, and
  do NOT use `>` (folded) unless you explicitly want newlines collapsed to
  spaces.

- Never use Ruby `<<~YAML` heredocs when generating YAML from scripts — Ruby's
  dedent strips leading whitespace and collides with YAML's 4-space
  indentation rules, silently breaking every downstream key. Build YAML as an
  array of explicitly-indented lines instead, and use
  `YAML.dump(str, line_width: -1)` for each scalar so long values don't get
  hard-wrapped into invalid block scalars.

- Indentation is 4 spaces per level. Check existing files before adding
  blocks — mismatched indent silently breaks every downstream key.

## Various
- don't capitalize headers in other languages than English where it's not the common convention
- "tokenomics" refers to economics, and should be localized without losing this analogy.
- *Capitalize "Bitcoin" when referring to the network, protocol, or asset as a whole, but use lowercase when referring to units, e.g., "two bitcoins."
- In Polish, decline Bitcoin according to case (na Bitcoina, w Bitcoinie, 5 bitcoinów, etc.).

### Prediction market

English: prediction market
German: prognosemarkt
Spanish: mercado de predicción
French: marché de prédiction
Italian: mercato di previsione
Dutch: voorspellingsmarkt
Portuguese: mercado de previsão
Polish: rynek predykcyjny
Russian: рынок предсказаний