"""A build guide cannot be generated from an unverified project."""

from helpers import create_project


async def test_unverified_project_cannot_generate_a_build_guide(http, alice):
    project_id = await create_project(http, alice)
    response = await http.post(f"/v1/projects/{project_id}/breadboard/generate", json={"rev": 0}, headers=alice)
    assert response.status_code == 409
    assert response.json()["code"] == "not_verified"

    response = await http.post(f"/v1/projects/{project_id}/breadboard/generate", json={"rev": 1}, headers=alice)
    assert response.status_code == 409
    assert response.json()["code"] == "stale_rev"
