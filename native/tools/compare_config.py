"""Compare two dumps (py_dump_config.py vs. dump_config example): values and key order.

    python native/tools/compare_config.py py.json rust.json
"""
import json
import sys


def walk(a, b, path, problems):
    if isinstance(a, dict) and isinstance(b, dict):
        if list(a) != list(b):
            problems.append(f"{path}: key order/set differs\n  py:   {list(a)}\n  rust: {list(b)}")
        for k in a.keys() | b.keys():
            if k not in a or k not in b:
                problems.append(f"{path}.{k}: only in {'py' if k in a else 'rust'}")
            else:
                walk(a[k], b[k], f"{path}.{k}", problems)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            problems.append(f"{path}: {len(a)} vs {len(b)} items")
        for i, (x, y) in enumerate(zip(a, b)):
            walk(x, y, f"{path}[{i}]", problems)
    elif a != b or type(a) is not type(b):
        problems.append(f"{path}: py {a!r} != rust {b!r}")


def main():
    with open(sys.argv[1], encoding="utf-8") as f:
        py = json.load(f)
    with open(sys.argv[2], encoding="utf-8") as f:
        rust = json.load(f)
    rust.pop("undecryptable", None)
    for k in ("theme_color", "flow_fins"):  # (Rust app only)
        rust.get("global", {}).pop(k, None)
    problems = []
    walk(py, rust, "$", problems)
    if problems:
        print("\n".join(problems))
        sys.exit(1)
    print(f"same: global {len(py['global'])} keys, {len(py['inbounds'])} inbound blocks, "
          f"{len(py['outbounds'])} outbound blocks, needs_save={py['needs_save']}")


if __name__ == "__main__":
    main()
