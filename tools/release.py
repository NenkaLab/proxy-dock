"""Cut a release: bump the version, commit, tag and push. GitHub Actions then builds and publishes it.

    python tools/release.py 2.1.0            # release 2.1.0
    python tools/release.py 2.1.0 --dry-run  # only show what would happen
    python tools/release.py --current        # print the current version

What it does:
  1. checks: on branch main, no uncommitted changes, in sync with origin/main, tag v<version> unused,
     the new version is higher than the current one, and the tests pass (tests/run_tests.py, skip with --no-tests)
  2. writes __version__ in proxydock/__init__.py and commits "Release v<version>"
  3. creates the annotated tag v<version> and pushes main and the tag
The tag starts .github/workflows/release.yml, which builds Windows / Linux / macOS, checks that the tag
matches __version__, and attaches the archives (+ SHA-256 checksums) to a GitHub Release.
"""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INIT = os.path.join(ROOT, "proxydock", "__init__.py")
SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$")


def git(*args, check=True):
    r = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace")
    if check and r.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed:\n{r.stderr.strip()}")
    return r.stdout.strip()


def current_version():
    with open(INIT, encoding="utf-8") as f:
        return re.search(r'__version__ = "([^"]+)"', f.read()).group(1)


def key(v):
    m = SEMVER.match(v)
    major, minor, patch, pre = int(m[1]), int(m[2]), int(m[3]), m[4]
    return (major, minor, patch, 0 if pre else 1, pre or "")      # 2.1.0-rc1 < 2.1.0


def main(argv):
    args = [a for a in argv[1:] if not a.startswith("--")]
    flags = {a for a in argv[1:] if a.startswith("--")}
    if "--current" in flags:
        print(current_version())
        return 0
    if len(args) != 1 or flags - {"--dry-run", "--no-tests"}:
        print(__doc__)
        return 64
    new = args[0].lstrip("v")
    dry = "--dry-run" in flags
    old = current_version()
    tag = f"v{new}"

    problems = []
    if not SEMVER.match(new):
        problems.append(f"'{new}' is not a version like 2.1.0 or 2.1.0-rc1")
    elif key(new) <= key(old):
        problems.append(f"{new} is not higher than the current version {old}")
    if git("rev-parse", "--abbrev-ref", "HEAD") != "main":
        problems.append("not on branch main")
    if git("status", "--porcelain"):
        problems.append("there are uncommitted changes")
    git("fetch", "--quiet", "--tags", "origin")
    if git("rev-parse", "HEAD") != git("rev-parse", "origin/main", check=False):
        problems.append("main is not in sync with origin/main (pull or push first)")
    if git("tag", "--list", tag):
        problems.append(f"tag {tag} already exists")
    if problems:
        print("cannot release:\n  - " + "\n  - ".join(problems))
        return 1

    if "--no-tests" not in flags:
        print("running tests...", flush=True)
        if subprocess.run([sys.executable, os.path.join(ROOT, "tests", "run_tests.py")], cwd=ROOT).returncode != 0:
            print("cannot release: tests failed")
            return 1

    print(f"release {old} -> {new} (tag {tag})")
    if dry:
        print("dry run: nothing changed")
        return 0
    with open(INIT, encoding="utf-8") as f:
        text = f.read()
    with open(INIT, "w", encoding="utf-8", newline="\n") as f:
        f.write(re.sub(r'__version__ = "[^"]+"', f'__version__ = "{new}"', text))
    git("add", INIT)
    git("commit", "-q", "-m", f"Release {tag}")
    git("tag", "-a", tag, "-m", f"ProxyDock {new}")
    git("push", "-q", "origin", "main")
    git("push", "-q", "origin", tag)
    print(f"pushed {tag}. The release workflow is building it:")
    print("  https://github.com/NenkaLab/proxy-dock/actions")
    return 0


if __name__ == "__main__":
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    sys.exit(main(sys.argv))
