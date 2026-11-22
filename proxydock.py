"""ProxyDock entry point (python proxydock.py / PyInstaller target)."""
from proxydock import i18n

# Pick the language before importing the UI: some labels are built at import time.
i18n.init_from_config()

from proxydock.ui.window import main  # noqa: E402

if __name__ == "__main__":
    main()
