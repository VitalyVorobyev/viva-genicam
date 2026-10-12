"""The book's Python snippets run against the fake camera.

A snippet the book marks with an HTML comment just above its code block is
executed here:

- ``<!-- pytest: camera -->`` with ``cam``, a camera opened on the fake, and
  ``info``, the fake's ``GigeDeviceInfo``;
- ``<!-- pytest: device -->`` with ``info`` only, and nothing holding control;
- ``<!-- pytest: none -->`` with neither.

``vg`` and ``np`` are imported for all three. The Rust snippets are included
from compiled examples; the Python ones are hand-written, so this is what
keeps them true.
"""

from __future__ import annotations

import re
from pathlib import Path

import numpy as np
import pytest

import viva_genicam as vg

BOOK = Path(__file__).resolve().parents[3] / "book" / "src"
MARKED = re.compile(r"<!-- pytest: (camera|device|none) -->\n```python\n(.*?)```", re.S)


def marked_snippets() -> list:
    pages = sorted(BOOK.glob("python*.md")) + sorted((BOOK / "python").glob("*.md"))
    found = []
    for page in pages:
        text = page.read_text(encoding="utf-8")
        for match in MARKED.finditer(text):
            line = text[: match.start()].count("\n") + 2
            found.append(
                pytest.param(match.group(1), match.group(2), id=f"{page.name}:{line}")
            )
    return found


SNIPPETS = marked_snippets() if BOOK.is_dir() else []
pytestmark = pytest.mark.skipif(not BOOK.is_dir(), reason="the book is not in this checkout")


def test_the_book_marks_snippets_to_run():
    """A marker the pattern no longer matches must not silently run nothing."""
    assert len(SNIPPETS) >= 10


@pytest.mark.parametrize("context, code", SNIPPETS)
def test_book_snippet_runs(context, code, fake_gige):
    namespace: dict = {"vg": vg, "np": np}
    if context in ("camera", "device"):
        namespace["info"] = fake_gige.device_info()
    if context == "camera":
        namespace["cam"] = vg.connect_gige(namespace["info"])
    try:
        exec(compile(code, "<book snippet>", "exec"), namespace)
    finally:
        if context == "camera":
            namespace["cam"].close()
