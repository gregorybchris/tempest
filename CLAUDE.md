# CLAUDE.md

## Branches

- By default, commit directly to `main` and push to the remote as you go.
- Use dev branches or worktrees only when needed to avoid conflicts (e.g. parallel work).

## Commit messages

- Subject line: short and imperative. It says precisely and concretely what was done.
- Body (optional): more detail, in paragraphs grouped by topic. Describe what actually changed and any important context. Be precise. No fluff.

## Documentation

- `README.md` stays lean: a conceptual overview, status, and installation and usage instructions.
- Anything longer goes in `specs/`, or in `protocol/` for the protocol. Link to it from the README only when it helps.
- Technical guidance, such as framework rules and architecture, belongs in `specs/`, not in this file.

## Prose

Applies to docs, specs, comments and messages.

- Be succinct and get to the point.
- No filler sentences.
- No mannered prose.
