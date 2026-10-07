# CLAUDE.md

## Branches

- By default, commit directly to `main` and push to the remote as you go.
- Use dev branches or worktrees only when needed to avoid conflicts (e.g. parallel work).

## Commit messages

- Subject line: short and imperative. It says precisely and concretely what was done.
- Body (optional): more detail, in paragraphs grouped by topic. Describe what actually changed and any important context. Be precise. No fluff.

## Prose

Applies to docs, specs, comments and messages.

- Be succinct and get to the point.
- No filler sentences.
- No mannered prose.

## Leptos

- Use Leptos 0.8 (pinned). Don't mix in APIs from 0.6 or older.
- Never create one memo per item that subscribes to a shared signal: removing a subscriber is O(n) in `reactive_graph`, so this becomes quadratic. Use one effect that diffs into per-item signals, `Selector`, or keyed stores.
- Keep UI logic in plain Rust modules; keep components thin.
