#!/usr/bin/env python3
"""Fetch Antigravity's system prompt and tool declarations from the public
x1xhlol/system-prompts-and-models-of-ai-tools collection and write

    <out>/system.txt   system prompt, without the embedded tool block
    <out>/tools.json   Gemini `tools` array

The text is not stored in this repository.
"""
import json
import re
import sys
import urllib.request

URL = ("https://raw.githubusercontent.com/x1xhlol/system-prompts-and-models-of-ai-tools/"
       "main/Google/Antigravity/Fast%20Prompt.txt")
SCALARS = {"string": "STRING", "number": "NUMBER", "boolean": "BOOLEAN", "any": "STRING"}


def schema(ts: str, desc: str) -> dict:
    ts = ts.strip().rstrip(",}").strip()
    out: dict = {}
    if ts.endswith("[]"):
        inner = ts[:-2]
        out = {"type": "ARRAY", "items": {"type": SCALARS.get(inner, "STRING")}}
    elif "|" in ts and ts.startswith('"'):
        out = {"type": "STRING", "enum": re.findall(r'"([^"]+)"', ts)}
    else:
        out = {"type": SCALARS.get(ts, "STRING")}
    if desc:
        out["description"] = desc
    return out


def parse_tools(block: str) -> list:
    decls, comment = [], []
    cur = None
    obj = None  # nested object being filled
    for raw in block.split("\n"):
        line = raw.rstrip()
        if line.startswith("//"):
            comment.append(line[2:].strip())
            continue
        m = re.match(r"type (\w+) = \(_: \{", line)
        if m:
            cur = {"name": m.group(1), "description": " ".join(comment),
                   "parameters": {"type": "OBJECT", "properties": {}, "required": []}}
            comment = []
            continue
        if cur is None:
            comment = []
            continue
        if line.startswith("}) => any"):
            decls.append(cur)
            cur, obj, comment = None, None, []
            continue
        desc = " ".join(comment)
        comment = []
        m = re.match(r"(\w+)(\?)?: (.*)$", line)
        if not m:
            continue
        name, optional, ts = m.groups()
        target = cur["parameters"]
        if ts == "{":
            obj = {"type": "OBJECT", "properties": {}, "required": []}
            if desc:
                obj["description"] = desc
            target["properties"][name] = obj
            if not optional:
                target["required"].append(name)
            continue
        if obj is not None:
            closes = ts.rstrip(",").endswith("}")
            obj["properties"][name] = schema(ts.rstrip("}"), desc)
            if not optional:
                obj["required"].append(name)
            if closes:
                obj = None
            continue
        target["properties"][name] = schema(ts, desc)
        if not optional:
            target["required"].append(name)
    return decls


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else "agy"
    text = urllib.request.urlopen(URL, timeout=60).read().decode()
    head, _, tools = text.partition("\n# Tools\n")
    if not tools:
        sys.exit("could not find the '# Tools' section; the source file layout changed")
    block = tools.split("namespace functions {", 1)[1].rsplit("} // namespace functions", 1)[0]
    decls = parse_tools(block)
    if not decls:
        sys.exit("no tool declarations parsed")
    import os
    os.makedirs(out, exist_ok=True)
    open(f"{out}/system.txt", "w").write(head.rstrip() + "\n")
    json.dump([{"functionDeclarations": decls}], open(f"{out}/tools.json", "w"), indent=1)
    print(f"{len(decls)} tools: {', '.join(d['name'] for d in decls)}")


if __name__ == "__main__":
    main()
