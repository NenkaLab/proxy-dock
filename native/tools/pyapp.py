"""Where the Python app is, for the parity tools (they compare the Rust core with it).

The Python app is no longer on main. The reference is its last state on main, commit 1e882fc (PY_REF below): it has
the fixes made while porting, which the `old` branch (the last Python release) does not. Check it out next to this
repository and point PROXYDOCK_PY at it:

    git worktree add --detach ../proxy-py 1e882fc
    python -m pip install -r ../proxy-py/requirements.txt
    $env:PROXYDOCK_PY = "..\\proxy-py"          (cmd: set PROXYDOCK_PY=..\\proxy-py)

Importing this module puts that folder first on sys.path, so `import proxydock` finds the Python app. Without
PROXYDOCK_PY the repository root is tried (a checkout from before the Python app was removed).
"""
import os
import sys

# the last commit on main with the Python app (the parity reference)
PY_REF = "1e882fc"

REPO = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
PY_ROOT = os.path.abspath(os.environ.get("PROXYDOCK_PY") or REPO)

if not os.path.isfile(os.path.join(PY_ROOT, "proxydock", "__init__.py")):
    raise SystemExit(f"the Python app is not in {PY_ROOT}: check out commit {PY_REF} and set PROXYDOCK_PY "
                     "(see native/tools/pyapp.py)")
if PY_ROOT not in sys.path:
    sys.path.insert(0, PY_ROOT)
