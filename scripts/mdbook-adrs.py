#!/usr/bin/env python3
"""mdBook 0.5.x preprocessor: inject ADR files nested under Architecture.

mdBook 0.5.x sidebar nesting is driven by Chapter.number depth, not
sub_items alone. This preprocessor:

1. Finds the Architecture chapter and reads its number (e.g. [15]).
2. Creates an "Architecture Decision Records" hub with number [15, 1].
3. Creates one child per ADR with number [15, 1, N].
4. Appends the hub to Architecture's sub_items.

Result in sidebar:
  15. Architecture
      15.1. Architecture Decision Records
          15.1.1. ADR 0001
          ...

Registration (docs/book.toml):
    [preprocessor.adrs]
    command = "python3 ../scripts/mdbook-adrs.py"
"""

import json
import os
import re
import sys


def main():
    if len(sys.argv) > 1:
        if sys.argv[1] == "supports":
            sys.exit(0)

    data = json.load(sys.stdin)
    context, book = data[0], data[1]

    root = context.get("root", ".")
    adr_dir = os.path.join(root, "adr")

    if not os.path.isdir(adr_dir):
        print(json.dumps(book))
        return

    adr_files = sorted(
        f for f in os.listdir(adr_dir)
        if re.match(r"^\d{4}-.*\.md$", f)
    )

    if not adr_files:
        print(json.dumps(book))
        return

    # Find Architecture chapter and its number
    # mdbook 0.5.x renamed Book.sections to Book.items (the legacy
    # "sections" key still deserializes as an alias but the renderer reads
    # the canonical key, so writing it silently drops the injected
    # chapters). Operate on whichever key the input carries.
    list_keys = [k for k in ("items", "sections") if k in book] or ["sections"]
    sections = book[list_keys[0]]
    arch_number = None
    arch_index = None

    for i, item in enumerate(sections):
        if "Chapter" not in item:
            continue
        ch = item["Chapter"]
        if ch.get("path", "").endswith("architecture/index.md"):
            arch_number = ch.get("number", [])
            arch_index = i
            break

    if arch_index is None:
        # No Architecture chapter — fall back to appending at end
        arch_number = [len(sections)]

    # Build ADR child chapters with proper numbering
    adr_children = []
    for ordinal, filename in enumerate(adr_files, start=1):
        filepath = os.path.join(adr_dir, filename)
        with open(filepath, "r", encoding="utf-8") as fh:
            content = fh.read()

        title = filename
        for line in content.split("\n"):
            if line.startswith("# "):
                title = line[2:].strip()
                break
        title = re.sub(r"^ADR-\d{4}:\s*", "", title)

        child_number = list(arch_number) + [1, ordinal]

        adr_children.append({
            "Chapter": {
                "name": f"{filename[:4]}: {title}",
                "content": content,
                "number": child_number,
                "sub_items": [],
                "path": f"adr/{filename}",
                "source_path": f"adr/{filename}",
                "parent_names": ["Architecture", "Architecture Decision Records"],
            }
        })

    # Build the ADR hub chapter
    hub_number = list(arch_number) + [1]

    adr_hub = {
        "Chapter": {
            "name": "Architecture Decision Records",
            "content": "# Architecture Decision Records\n\n"
                       "Every architectural choice has a recorded decision. "
                       "Each ADR states the context, the decision, and the consequences.\n",
            "number": hub_number,
            "sub_items": adr_children,
            "path": "adr/index.md",
            "source_path": "adr/index.md",
            "parent_names": ["Architecture"],
        }
    }

    # Append hub to Architecture's sub_items (NOT to top-level items)
    if arch_index is not None:
        existing_subs = sections[arch_index]["Chapter"].get("sub_items", [])
        sections[arch_index]["Chapter"]["sub_items"] = existing_subs + [adr_hub]
    else:
        # No Architecture found — add as top-level
        sections.append(adr_hub)

    book[list_keys[0]] = sections
    print(json.dumps(book))


if __name__ == "__main__":
    main()
