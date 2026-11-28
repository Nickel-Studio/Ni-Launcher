#!/usr/bin/env python3
"""Prune releases and tags older than the current and previous stable release.

The release workflow runs this only after a tagged build has uploaded every
asset. Keeping two stable releases keeps the Releases page small and avoids
pagination/billing pressure on update checks.
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.github.com"
VERSION_RE = re.compile(r"^v(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$")


def parse_version(tag: str) -> tuple[tuple[int, int, int], str | None] | None:
    match = VERSION_RE.fullmatch(tag)
    if not match:
        return None
    base = (int(match.group(1)), int(match.group(2)), int(match.group(3)))
    return base, match.group(4)


def select_retention(tags: list[str], current: str) -> tuple[list[str], list[str]]:
    """Return (kept, obsolete) tags for the current stable release.

    The current stable release and the highest older stable release are kept.
    Older stable releases, the same release's prereleases, and older
    prereleases are obsolete. Tags newer than the current release are ignored
    so an accidental old workflow run cannot remove a newer release.
    """

    current_parsed = parse_version(current)
    if current_parsed is None or current_parsed[1] is not None:
        raise ValueError("current tag must be a stable vX.Y.Z tag")
    if current not in tags:
        raise ValueError("current tag is missing from the release/tag list")
    current_base = current_parsed[0]

    stable_older = sorted(
        {
            (parsed[0], tag)
            for tag in tags
            if (parsed := parse_version(tag)) is not None
            and parsed[1] is None
            and parsed[0] < current_base
        },
        reverse=True,
    )
    previous = stable_older[0][1] if stable_older else None
    kept = [tag for tag in (current, previous) if tag]
    kept_set = set(kept)

    obsolete = []
    for tag in set(tags):
        parsed = parse_version(tag)
        if parsed is None or tag in kept_set:
            continue
        base, prerelease = parsed
        if base < current_base or (base == current_base and prerelease is not None):
            obsolete.append(tag)

    obsolete.sort(key=lambda tag: (parse_version(tag)[0], parse_version(tag)[1] or ""))
    return kept, obsolete


def request(method: str, path: str, token: str) -> tuple[int, object | None]:
    request = urllib.request.Request(
        API + path,
        method=method,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "nickel-release-pruner",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read()
            return response.status, json.loads(body) if body else None
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return 404, None
        raise


def list_releases(repository: str, token: str) -> list[dict]:
    releases = []
    page = 1
    while True:
        _, data = request(
            "GET", f"/repos/{repository}/releases?per_page=100&page={page}", token
        )
        if not isinstance(data, list) or not data:
            break
        releases.extend(data)
        if len(data) < 100:
            break
        page += 1
    return releases


def list_tags(repository: str, token: str) -> list[str]:
    tags = []
    page = 1
    while True:
        _, data = request(
            "GET",
            f"/repos/{repository}/git/matching-refs/tags/v?per_page=100&page={page}",
            token,
        )
        if not isinstance(data, list) or not data:
            break
        for ref in data:
            name = ref.get("ref", "")
            if name.startswith("refs/tags/"):
                tags.append(name.removeprefix("refs/tags/"))
        if len(data) < 100:
            break
        page += 1
    return tags


def main() -> int:
    repository = os.environ.get("GITHUB_REPOSITORY", "").strip()
    current = (os.environ.get("RELEASE_TAG") or os.environ.get("GITHUB_REF_NAME", "")).strip()
    token = (os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN") or "").strip()
    dry_run = os.environ.get("PRUNE_DRY_RUN") == "1"

    if not repository or not current or not token:
        print(
            "GITHUB_REPOSITORY, RELEASE_TAG/GITHUB_REF_NAME and GITHUB_TOKEN are required",
            file=sys.stderr,
        )
        return 2

    releases = list_releases(repository, token)
    tags = list_tags(repository, token)
    tag_set = set(tags)
    tag_set.update(release.get("tag_name", "") for release in releases)
    tag_set.discard("")

    kept, obsolete = select_retention(sorted(tag_set), current)
    by_tag = {release.get("tag_name"): release for release in releases}

    deleted_releases = []
    deleted_tags = []
    for tag in obsolete:
        release = by_tag.get(tag)
        if release is not None:
            if not dry_run:
                request("DELETE", f"/repos/{repository}/releases/{release['id']}", token)
            deleted_releases.append(tag)
        if not dry_run:
            status, _ = request(
                "DELETE",
                f"/repos/{repository}/git/refs/tags/{urllib.parse.quote(tag, safe='')}",
                token,
            )
            if status not in (204, 404, 422):
                raise RuntimeError(f"unexpected tag deletion status {status}")
        deleted_tags.append(tag)

    print(
        json.dumps(
            {
                "current": current,
                "kept": kept,
                "obsolete": obsolete,
                "deletedReleases": deleted_releases,
                "deletedTags": deleted_tags,
                "dryRun": dry_run,
            },
            ensure_ascii=True,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, RuntimeError, urllib.error.URLError) as error:
        print(f"release pruning failed: {error}", file=sys.stderr)
        raise SystemExit(1)
