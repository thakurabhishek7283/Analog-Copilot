import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))

import circuit_core as cc  # noqa: E402
from ngspice_batch import REGISTRY_DIR, ngspice_path  # noqa: E402


def pytest_collection_modifyitems(config, items):
    """No ngspice: skip locally, fail in CI (REQUIRE_NGSPICE=1) so the gate can't pass silently."""
    if ngspice_path() is not None:
        return
    msg = "ngspice not built: run third_party/ngspice/build-native.sh"
    if os.environ.get("REQUIRE_NGSPICE"):
        raise pytest.UsageError(msg)
    for item in items:
        item.add_marker(pytest.mark.skip(reason=msg))


@pytest.fixture(scope="session")
def reg() -> cc.Registry:
    return cc.load_registry_dir(REGISTRY_DIR)
