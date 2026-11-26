"""Random config.json files with wrong types, edge values and DPAPI secrets, for comparing the Python and Rust
readers (py_dump_config.py / dump_config example / compare_config.py).

    python native/tools/fuzz_config.py <out dir> [count]
"""
import json
import os
import random
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pyapp  # noqa: E402,F401  (the Python app: see pyapp.py)
from proxydock import config  # noqa: E402

ODD = [None, True, False, 0, 1, -5, 70000, 3.5, "", " x ", "127.0.0.1", "한국어", [], ["a", " b ", ""], ["a", 1], {}, "tls://dns.google",
       "https://dns.example/q", "ftp://x", "a.com", "localhost", "chacha20-ietf-poly1305", "bogus", "tun", "gvisor"]


def value_for(default):
    r = random.random()
    if r < 0.35:
        return random.choice(ODD)
    if isinstance(default, bool):
        return random.choice([True, False])
    if isinstance(default, int):
        return random.choice([0, 1, 5, 10, 99, 120, 30307, 65535, 65536, -1, 100000, 86401])
    if isinstance(default, list):
        return random.choice([[], ["*"], [" a.com ", ""], ["https://Example.com/x", "example.com", "a..b", "x"]])
    return random.choice(["", "x", " pad ", "1.1.1.1", "tls://1.1.1.1:8853", "h3://dns.x/q", "dark", "ko", "xx",
                          "socks5", "vless", "group", "2022-blake3-aes-128-gcm"])


def section(defaults):
    d = {}
    for k, v in defaults.items():
        if random.random() < 0.7:
            d[k] = value_for(v)
    if random.random() < 0.3:
        d["unknown_" + str(random.randint(0, 9))] = random.choice(ODD)
    return d


def main():
    out, count = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 200
    os.makedirs(out, exist_ok=True)
    random.seed(1234)
    for i in range(count):
        inbounds, outbounds = [], []
        for _ in range(random.randint(0, 4)):
            ib = section(config.INBOUND_DEFAULTS)
            for k in config.SECRET_FIELDS:
                if random.random() < 0.3:
                    ib[k] = random.choice(["", "pw-" + str(random.randint(0, 999)), "비밀"])
            sealed = config._seal({k: v for k, v in ib.items() if not (k in config.SECRET_FIELDS and not isinstance(v, str))})
            if random.random() < 0.1:          # an unencrypted secret (legacy) or a foreign blob
                sealed["password"] = "plain" if random.random() < 0.5 else sealed.get("password", "")
                if random.random() < 0.5:
                    sealed["ss_password_dpapi"] = "AAAA"
            inbounds.append({"id": f"i{len(inbounds)}", "name": "in", "inbound": sealed})
        for _ in range(random.randint(0, 4)):
            ob = section(config.OUTBOUND_DEFAULTS)
            for k in config.SECRET_FIELDS:
                if random.random() < 0.3:
                    ob[k] = random.choice(["", "pw", "{\"x\": 1}"])
            sealed = config._seal({k: v for k, v in ob.items() if not (k in config.SECRET_FIELDS and not isinstance(v, str))})
            outbounds.append({"id": f"o{len(outbounds)}", "name": "out", "outbound": sealed})
        data = {"version": 3, "global": section(config.GLOBAL_DEFAULTS) if random.random() < 0.9 else random.choice(ODD),
                "graph": {"inbounds": inbounds, "outbounds": outbounds}}
        with open(os.path.join(out, f"c{i:03}.json"), "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False)
    print(f"{count} files in {out}")


if __name__ == "__main__":
    main()
