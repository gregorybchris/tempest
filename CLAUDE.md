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

## Avoiding staleness

Agents can't read every file every time, so anything that must be kept in sync by hand will eventually drift. Avoid creating it:

- Don't document temporary things (prototypes, experiments, scaffolding) in lasting docs like the README.
- Don't repeat implementation details (exact values, file paths, function names) in prose when the code or spec already defines them. Describe concepts and link to the source of truth.
- Prefer one source of truth per fact. If a fact must appear in two places, make one point to the other.
- Avoid long-range dependencies between files that no test checks.

## Prose

Applies to docs, specs, comments and messages.

- Be succinct and get to the point.
- No filler sentences.
- No mannered prose.
