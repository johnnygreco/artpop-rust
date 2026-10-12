import pytest


def pytest_addoption(parser):
    parser.addoption("--rust-bin", default=None, help="Path to a real artpop-rust binary; required for differential parity tests")
    parser.addoption("--require-rust", action="store_true", help="Fail collection instead of allowing absent Rust comparisons (release CI)")


def pytest_configure(config):
    if config.getoption("--require-rust") and not config.getoption("--rust-bin"):
        raise pytest.UsageError("--require-rust requires --rust-bin; skipped comparisons are not release parity evidence")
