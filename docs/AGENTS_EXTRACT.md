# Using `extract` (agent guide)

`extract` reads structured data from the current page with a small CSS-selector DSL
("Deep Lens"). It is read-only, and its output is prompt-injection sanitized and
PII-redacted before you see it. Use it when you need *data* (prices, links, table rows)
rather than something to click; use `get_state` + `act` for interaction.

Call it with either a pre-registered rule or an inline rule:

```json
{"rule_name": "page_title"}
{"inline": {"selector": "h1"}}
```

## 1. The three modes

| Mode | Rule shape | Returns |
| --- | --- | --- |
| **Single value** | `{"selector": "...", "attribute": "..."?}` | One string, or `null` (first match only) |
| **Fields** | `{"selector": "...", "fields": {...}}` | Array with one object per match |
| **Items** | `{"items": {"selector": "...", "fields": {...}}}` | Same as Fields (preferred spelling) |

### Single value

```json
{"inline": {"selector": "a.primary-link", "attribute": "href"}}
```

- Only the **first** element matching `selector` is read (`querySelector`).
- **Omitting `attribute` returns the element's text** (`innerText`, falling back to
  `textContent`, trimmed). `"attribute": "text"` means the same.
- `textContent`, `innerText` and `value` are read as live **DOM properties**.
- Any other name is read as an HTML **attribute** (`getAttribute`): `href`, `src`,
  `data-price`, `aria-label`, ... Attribute values are what the HTML says, so a relative
  `href="/a"` comes back as `"/a"`, not a resolved URL.

### Items (many records)

```json
{"inline": {"items": {
  "selector": "tr.product",
  "fields": {
    "name":  ".product-name",
    "price": ".product-price",
    "sku":   "@data-sku"
  }
}}}
```

returns, for example:

```json
[{"name": "Widget", "price": "$9", "sku": "W-1"}, {"name": "Gadget", "price": "$12", "sku": "G-2"}]
```

`items.selector` picks the repeating element (every match, via `querySelectorAll`).
Each entry of `fields` is `output_key: spec`, where `spec` is one of:

| `spec` | Meaning |
| --- | --- |
| `".product-name"` (a CSS selector) | Text of the **first descendant of the item** matching it, or `null` |
| `"&"` | The item's own text |
| `"@href"` (`@` + name) | The item's own attribute (`getAttribute`), or `null`. `@textContent`, `@innerText`, `@value`, `@tagName` are read as properties (`@tagName` gives e.g. `"H2"`) |

`&` and `@...` are never valid CSS selectors, so they cannot be confused with a child
selector.

## 2. Common mistakes

- **Treating `fields` values as DOM properties.** `"price": "textContent"` looks for a
  `<textContent>` element. Use `".price"` for a child's text, `"&"` for the item's own
  text, or `"@data-price"` for an attribute.
- **Repeating the item selector inside a field.** Field selectors are resolved *inside*
  each item. With `items.selector = "tr.product"`, use `".product-name"`, not
  `"tr.product .product-name"`.
- **Expecting a list from a single-value rule.** `{"selector": "li"}` returns only the
  first `<li>`. Use `items` for lists.
- **Putting an attribute on an `items` rule.** `attribute` only applies to single-value
  rules; for items use `"@attr"` in `fields`.
- **Using a `stable_key` or element `id` from `get_state` as a selector.** `stable_key`
  is not a CSS selector. Derive a selector from the element's role, label and attributes
  (see below).
- **Expecting resolved URLs.** `href`/`src` come back as written in the HTML.
- **Relying on generated class names.** `get_state` strips dynamic (hashed) class tokens,
  so a class you can see there is usually stable, but a class you cannot see may still
  exist on the page. Prefer `id`, `data-*`, `name`, `aria-*` and semantic tags.

## 3. Selector discovery workflow

1. `get_state` — find the elements that hold the data. Note each element's `role`,
   name/label and `attributes` (`id`, `name`, `data-*`, `href`, filtered `class`).
2. Write the narrowest selector those attributes support (`#price`, `table.results tr`,
   `a[href^="/product/"]`).
3. Try it with an inline `extract`. If `result` is `null`/empty/partly `null`, read the
   `errors` object in the response (next section) and adjust.
4. Pass `"debug": true` if you need to see exactly what was run (`script` in the
   response).
5. When the selector is right and you will reuse it, keep using it inline, or ask the
   operator to register it. `extract` never mutates the page.

## 4. Reading failures

A response is `{"rule", "result", "security_flags"}` plus, only when relevant:

- `errors` — why a result is `null`, empty or partly `null`. Keys are your field names,
  or `"$selector"` for the rule's own selector. Values start with a category:
  - `SelectorNoMatch: No elements matched selector '.name' within 1 of 3 items`
  - `AttributeNotFound: No attribute 'href' within all 3 items`
  - `SelectorNoMatch: No elements matched selector '.missing'` (key `$selector`)
- `script` — the generated JavaScript (only with `"debug": true`).

If the page script itself fails — typically an invalid selector — the call returns an
error beginning `ScriptEvalError:` with the browser's message, for example
`ScriptEvalError: Failed to execute 'querySelector' on 'Document': '<<' is not a valid
selector.` Fix the selector and retry. `errors` text is derived from the page's own
script engine, so it is sanitized like extracted content; if the sanitizer flags a
browser error message it is withheld.

## 5. Pre-registered rules

These are always available by `rule_name`; their names are reserved.

| `rule_name` | Returns |
| --- | --- |
| `page_title` | The page `<title>` text |
| `all_links` | Array of `{text, href}` for every `a[href]` |
| `meta_description` | The `content` of `meta[name=description]` |
| `headings` | Array of `{level, text}` for `h1`–`h6` (`level` is the tag name, e.g. `"H2"`) |

```json
{"rule_name": "all_links"}
```

## 6. Tested patterns

The rule shapes above are exercised by the repository's golden fixtures in
`core-runtime/tests/fixtures/golden/` (`page_title`, `link_href` — single value with an
attribute — and `product_table` — items with child-selector fields) and by real-browser
tests in `mcp-server/tests/extract_builtin_rules.rs` and
`mcp-server/tests/extract_diagnostics.rs`.
