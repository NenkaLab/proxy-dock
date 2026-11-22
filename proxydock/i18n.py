"""Translations.

Source strings in the code are English and double as lookup keys: ``_t("Start test")``.
``locales/<code>.json`` maps each English string to its translation; anything missing falls
back to English. Strings with placeholders are translated first and formatted afterwards,
e.g. ``_t("Port {port} available").format(port=port)``.

The language is chosen once at startup (``init_from_config``) because some labels are
built at import time; changing it in Settings takes effect after a restart.
"""
import json
import os
import sys

DEFAULT_LANGUAGE = "en"
LANGUAGES = [
    ("en", "English"),
    ("ko", "한국어"),
    ("ja", "日本語"),
    ("zh_CN", "简体中文"),
    ("zh_TW", "繁體中文"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("es", "Español"),
]
LANGUAGE_CODES = [code for code, _ in LANGUAGES]

_current = DEFAULT_LANGUAGE
_table = {}


def locale_dir():
    base = getattr(sys, "_MEIPASS", None)
    if base:
        return os.path.join(base, "proxydock", "locales")
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), "locales")


def load_table(code):
    """English -> translation table for ``code`` (empty for English or a missing file)."""
    if code == DEFAULT_LANGUAGE:
        return {}
    try:
        with open(os.path.join(locale_dir(), f"{code}.json"), encoding="utf-8") as f:
            data = json.load(f)
        return {k: v for k, v in data.items() if isinstance(k, str) and isinstance(v, str) and v}
    except (OSError, ValueError):
        return {}


def set_language(code):
    global _current, _table
    if code not in LANGUAGE_CODES:
        code = DEFAULT_LANGUAGE
    _current = code
    _table = load_table(code)


def current():
    return _current


def _t(text):
    return _table.get(text, text)


def _config_path():
    """Same location as config.CONFIG_PATH (duplicated here so this module has no imports)."""
    home = os.path.expanduser("~")
    if sys.platform == "win32":
        base = os.environ.get("APPDATA") or home
    elif sys.platform == "darwin":
        base = os.path.join(home, "Library", "Application Support")
    else:
        base = os.environ.get("XDG_CONFIG_HOME") or os.path.join(home, ".config")
    return os.path.join(base, "ProxyDock", "config.json")


def init_from_config():
    """Read the language from config.json before the rest of the app is imported."""
    code = DEFAULT_LANGUAGE
    try:
        with open(_config_path(), encoding="utf-8") as f:
            g = json.load(f).get("global") or {}
        if isinstance(g.get("language"), str):
            code = g["language"]
    except (OSError, ValueError, AttributeError):
        pass
    set_language(code)
    return _current
