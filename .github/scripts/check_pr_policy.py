#!/usr/bin/env python3
"""Validate Settlers PR branches and visible dependency declarations, offline."""

import json
import os
from pathlib import Path
import re
import sys
from urllib.parse import urlsplit


WORK_BRANCH = re.compile(
    r"(?:feature|fix|improvement|chore|docs|refactor|test|perf|build|ci)/"
    r"[a-z0-9]+(?:-[a-z0-9]+)*\Z"
)
FIELD = re.compile(
    r"^[ \t]*(?:[-*+][ \t]+)?(?:\*\*)?(Upstream|Downstream)(?:\*\*)?"
    r"[ \t]*:(?:\*\*)?[ \t]*(.*?)[ \t]*$",
    re.IGNORECASE | re.MULTILINE,
)
URL = re.compile(r"https?://[^\s<>\[\]()`;,]+")
PR_PATH = re.compile(r"/[A-Za-z0-9-]+/[A-Za-z0-9_.-]+/pull/[1-9][0-9]*/?\Z")


def is_pr_url(value):
    """Check the URL shape; existence and merge status need authenticated review."""
    try:
        parsed = urlsplit(value.rstrip(".!"))
        return (
            parsed.scheme == "https"
            and parsed.netloc == "github.com"
            and PR_PATH.fullmatch(parsed.path) is not None
        )
    except ValueError:
        return False


def validate(event):
    errors = []
    pull = event.get("pull_request")
    if not isinstance(pull, dict):
        return ["Expected a pull_request event."]
    base = pull.get("base") or {}
    head = pull.get("head") or {}
    if base.get("ref") == "dev":
        if not WORK_BRANCH.fullmatch(head.get("ref") or ""):
            errors.append(
                "PRs to dev need a typed branch: feature|fix|improvement|chore|docs|"
                "refactor|test|perf|build|ci/<lowercase-dashed-slug>."
            )
    elif base.get("ref") == "main":
        head_repo = (head.get("repo") or {}).get("full_name")
        base_repo = (base.get("repo") or {}).get("full_name")
        if head.get("ref") != "dev" or not base_repo or head_repo != base_repo:
            errors.append("Release PRs to main must come from dev in the same repository.")
    else:
        errors.append("PRs must target dev, or main for a dev-to-main release.")

    # Template instructions and examples in HTML comments are not declarations.
    body = re.sub(r"<!--[\s\S]*?(?:-->|\Z)", "", pull.get("body") or "")
    body = body.replace("\r\n", "\n").replace("\r", "\n")
    fields = {"upstream": [], "downstream": []}
    for match in FIELD.finditer(body):
        fields[match.group(1).lower()].append(match.group(2))
    for label, values in fields.items():
        if len(values) != 1:
            errors.append(f"Include exactly one visible {label.title()}: field on its own line.")
            continue
        value = values[0]
        links = URL.findall(value)
        no_dependencies = re.fullmatch(
            r"None(?:\.|[ \t]+(?:\([^\r\n]*\)|[-—–:][ \t]+[^\r\n]+))?",
            value,
            re.IGNORECASE,
        )
        if no_dependencies and not links:
            continue
        if re.match(r"None\b", value, re.IGNORECASE) or not links or not all(is_pr_url(link) for link in links):
            errors.append(
                f"{label.title()}: must contain GitHub PR URL(s), or None with an optional reason."
            )
    return errors


def main():
    event_path = os.environ.get("GITHUB_EVENT_PATH")
    if not event_path:
        print("GITHUB_EVENT_PATH is required.", file=sys.stderr)
        return 1
    try:
        event = json.loads(Path(event_path).read_text(encoding="utf-8"))
        if not isinstance(event, dict):
            raise ValueError("Expected an event object")
    except (OSError, ValueError):
        print("Could not read the GitHub event JSON.", file=sys.stderr)
        return 1
    errors = validate(event)
    if errors:
        print("Pull request policy failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("Pull request branch and dependency declarations passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
