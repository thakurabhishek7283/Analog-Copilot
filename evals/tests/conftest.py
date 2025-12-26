"""The eval harness's tests. The metric and prompt tests are pure; the run tests start Postgres and
Redis (testcontainers) and the sim worker on the native ngspice, so they skip without Docker or
ngspice locally and fail instead in CI (REQUIRE_DOCKER, REQUIRE_NGSPICE), like apps/api/tests."""

from __future__ import annotations

import os
import sys
from collections.abc import Iterator
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import run_evals  # noqa: E402,F401  (puts tools/e2e on the path)
import stack  # noqa: E402


def docker_available() -> bool:
    try:
        import docker

        docker.from_env().ping()
        return True
    except Exception:  # noqa: BLE001
        return False


def ngspice_available() -> bool:
    from sim_runner import ngspice_batch

    return ngspice_batch.ngspice_path() is not None


def pytest_collection_modifyitems(config, items):
    users = [i for i in items if "databases" in i.fixturenames]
    if not users:
        return
    for available, env, msg in (
        (docker_available, "REQUIRE_DOCKER", "Docker is not running (testcontainers Postgres and Redis)"),
        (ngspice_available, "REQUIRE_NGSPICE", "ngspice not built: run third_party/ngspice/build-native.sh"),
    ):
        if available():
            continue
        if os.environ.get(env):
            raise pytest.UsageError(msg)
        for item in users:
            item.add_marker(pytest.mark.skip(reason=msg))


@pytest.fixture(scope="session")
def databases() -> Iterator[tuple[str, str]]:
    with stack.databases() as urls:
        yield urls

