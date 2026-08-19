from typing import Annotated, Final, NoReturn

from rich.console import Console
from typer import Exit, Option, Typer

main = Typer(pretty_exceptions_enable=False)

OUT_CONSOLE: Final[Console] = Console()
ERR_CONSOLE: Final[Console] = Console(stderr=True)


def _fail(message: str) -> NoReturn:
    ERR_CONSOLE.print(f"error: {message}", markup=False, highlight=False, soft_wrap=True)
    raise Exit(1)


@main.callback()
def root() -> None:
    pass


@main.command(name="client")
def client_command(
    *,
    name: Annotated[str, Option("--name", "-n", help="Who to greet.")] = "tempest",
) -> None:
    """Run the Tempest client."""
    if not name.strip():
        _fail("--name cannot be empty")
    print(f"Client, {name}!")


@main.command(name="tracker")
def tracker_command(
    *,
    name: Annotated[str, Option("--name", "-n", help="Who to greet.")] = "tempest",
) -> None:
    """Run the Tempest tracker."""
    if not name.strip():
        _fail("--name cannot be empty")
    print(f"Tracker, {name}!")


if __name__ == "__main__":
    main()
