import re
import sys

text = open(sys.argv[1], encoding="utf-8").read()
start = text.index("## QUALITY GATES")
block = text[start:]
open_fence = block.index("```") + 3
close_fence = block.index("```", open_fence)
body = block[open_fence:close_fence]

gates = []
name = None
run = []
run_sep = " "


def strip_comment(value):
    """Drop a YAML inline comment: `name: x   # note` -> `x`."""
    marker = value.find("  #")
    if marker != -1:
        value = value[:marker]
    return value.rstrip()


def flush():
    if name:
        gates.append((name, run_sep.join(run).strip()))


for line in body.splitlines():
    m = re.match(r"^- name:\s*(.+?)\s*$", line)
    if m:
        flush()
        name, run, run_sep = strip_comment(m.group(1)).strip(), [], " "
        continue
    m = re.match(r"^\s+run:\s*(.*)$", line)
    if m:
        value = m.group(1)
        if value[:1] in ("|", ">"):
            # Literal/folded block: one shell command per line.
            run, run_sep = [], "\n"
        else:
            # Plain scalar wrapped over several lines: YAML folds with spaces.
            run, run_sep = [strip_comment(value)], " "
        continue
    if name and line.startswith("    "):
        continuation = line.strip()
        if continuation:
            run.append(continuation)
flush()

for gate_name, command in gates:
    print(gate_name.replace("\t", " ") + "\t" + command.replace("\n", "\\n"))
