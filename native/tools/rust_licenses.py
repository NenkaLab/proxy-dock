"""List the Rust crates the app is built from (name, version, licence) into licenses/rust-crates.txt.

    python native/tools/rust_licenses.py

Only crates the app actually builds on Windows (normal dependencies of proxydock and proxydock-core), not build
or dev dependencies. Run it again when the dependencies change.
"""
import json
import os
import subprocess

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
NATIVE = os.path.join(ROOT, "native")
OUT = os.path.join(ROOT, "licenses", "rust-crates.txt")


def main():
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc"],
        cwd=NATIVE, capture_output=True, check=True).stdout)
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    roots = [p["id"] for p in meta["packages"] if p["name"] in ("proxydock", "proxydock-core") and p["source"] is None]
    seen, todo = set(), list(roots)
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k["kind"] in (None, "normal") for k in d["dep_kinds"]):
                todo.append(d["pkg"])
    rows = sorted((pkgs[i]["name"], pkgs[i]["version"], pkgs[i].get("license") or "see the crate", pkgs[i].get("repository") or "")
                  for i in seen if i not in meta["workspace_members"])
    lines = [
        "ProxyDock (Rust app) is built from these Rust crates, each under the licence shown (SPDX).",
        "Their full texts are in each crate's source (crates.io) and repository.",
        "",
    ]
    w = max(len(f"{n} {v}") for n, v, _, _ in rows)
    for n, v, lic, repo in rows:
        lines.append(f"{(n + ' ' + v).ljust(w)}  {lic}" + (f"  {repo}" if repo else ""))
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print(f"{len(rows)} crates -> {OUT}")


if __name__ == "__main__":
    main()
