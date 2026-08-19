"""Tempest.

Scaffolding only for now: the package exposes a version string and a CLI, and nothing
else. Real modules go beside this one and get re-exported here.
"""


def __getattr__(name: str) -> str:
    """Resolve ``__version__`` lazily.

    ``importlib.metadata`` costs about 30 ms to import -- a large share of the whole
    package's import time -- and almost nothing needs the version string. Deferring it
    keeps ``import tempest`` cheap while leaving ``tempest.__version__`` working for the
    CLI and anyone who asks.

    Importing the package must not depend on it having been installed, so a missing
    dist-info resolves to a placeholder rather than raising.
    """
    if name != "__version__":
        message = f"module {__name__!r} has no attribute {name!r}"
        raise AttributeError(message)

    from importlib.metadata import PackageNotFoundError, version  # noqa: PLC0415 - deferred on purpose

    try:
        return version("tempest")
    except PackageNotFoundError:  # pragma: no cover - only in an uninstalled tree
        return "0.0.0"


__all__ = ["__version__"]
