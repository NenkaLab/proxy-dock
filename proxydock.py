"""ProxyDock entry point (python proxydock.py / PyInstaller target)."""
import sys

if len(sys.argv) > 1 and sys.argv[1] == "--tun-helper":     # elevated TUN helper (see proxydock/singbox/helper.py)
    from proxydock.singbox.helper import main as _helper_main
    sys.exit(_helper_main(sys.argv[2:]))

from proxydock import i18n  # noqa: E402

# Pick the language before importing the UI: some labels are built at import time.
i18n.init_from_config()

from proxydock.ui.window import main  # noqa: E402

if __name__ == "__main__":
    main()
