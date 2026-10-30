#!/usr/bin/env python3
"""Sync src/builtin.rs form/POST allowlists with the form and POST case manifests.

FORM_IDS and POST_IDS become exactly the IDs in tests/fixtures/form-layouts/cases.json,
tests/fixtures/post-layouts/cases.json and tests/fixtures/form-batches/*.json
(direct_post selects POST_IDS). IDs that left those lists, because a batch dropped
them, also leave READY_IDS; new ones join it. Rerunning is a no-op. Admission tests
verify the result.
"""
import glob
import json
import pathlib
import re

src = pathlib.Path("src/builtin.rs")
text = src.read_text()
files = ["tests/fixtures/form-layouts/cases.json", "tests/fixtures/post-layouts/cases.json"]
files += sorted(glob.glob("tests/fixtures/form-batches/*.json"))
cases = [c for f in files for c in json.loads(pathlib.Path(f).read_text())]
form = {c["id"] for c in cases if not c.get("direct_post")}
post = {c["id"] for c in cases if c.get("direct_post")}


def current(text, name):
    head = f"pub const {name}: &[&str] = &[\n"
    start = text.index(head) + len(head)
    return set(re.findall(r'"([^"]+)"', text[start : text.index("];", start)]))


def replace(text, name, ids):
    head = f"pub const {name}: &[&str] = &[\n"
    start = text.index(head) + len(head)
    end = text.index("];", start)
    return text[:start] + "".join(f'    "{i}",\n' for i in sorted(ids)) + text[end:]


removed = (current(text, "FORM_IDS") | current(text, "POST_IDS")) - (form | post)
ready = (current(text, "READY_IDS") - removed) | form | post
text = replace(text, "FORM_IDS", form)
text = replace(text, "POST_IDS", post)
text = replace(text, "READY_IDS", ready)
src.write_text(text)
print(f"{len(form)} form + {len(post)} POST cases; removed {sorted(removed)}")
