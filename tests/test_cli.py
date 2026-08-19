"""Tests for the ``tempest`` command line interface.

The placeholder command is exercised on its happy path and on the one failure a user
can hit. Failures must exit non-zero with a single ``error:`` line and no traceback --
a leaked traceback is a bug, not a message.
"""

from typer.testing import CliRunner, Result

from tempest.cli import main

runner = CliRunner()


def assert_clean_failure(result: Result) -> None:
    """A reported failure exits non-zero, explains itself, and leaks no traceback."""
    assert result.exit_code != 0
    assert "Traceback" not in result.output
    assert "error: " in result.stderr


class TestTopLevel:
    """The app itself: help, version, and the no-arguments behaviour."""

    def test_no_arguments_shows_help(self) -> None:
        result = runner.invoke(main, [])
        assert "Usage" in result.output
