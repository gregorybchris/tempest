# tempest

A scaffolded Python project: `uv`, `ruff`, `ty`, `pytest`, and a Typer + Rich CLI. The
tooling is wired up and green; the code is a placeholder waiting to be replaced.

## Install

Requires Python 3.14+.

```bash
uv sync --dev
```

## Run

```bash
$ uv run tempest hello
Hello, tempest!

$ uv run tempest hello --name world
Hello, world!

$ uv run tempest --version
tempest 0.1.0
```

## Layout

| Path             | What it holds                                            |
| ---------------- | -------------------------------------------------------- |
| `src/tempest/`   | The package. `cli.py` is the entry point.                 |
| `tests/`         | Pytest suite, run against `src/` via `pythonpath`.        |
| `specs/`         | Design notes and project specs.                           |
| `.githooks/`     | Pre-commit hook: ruff, ty, pytest.                        |
| `.github/`       | CI, running the same three checks.                        |

## Development

```bash
make lint       # ruff check + ruff format --check
make format     # ruff format + ruff check --fix
make typecheck  # ty
make test       # pytest
```

Enable the pre-commit hook, which runs all four:

```bash
git config core.hooksPath .githooks
```
