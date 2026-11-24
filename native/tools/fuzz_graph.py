"""Random settings files with messy graphs (links to missing blocks and lanes, duplicate ids, via / router / lane
loops, groups, wireless pairs with clashing numbers, IF and switch blocks, odd colours and positions) for
comparing the Python and Rust graph validation (parity_graph.py).

    python native/tools/fuzz_graph.py <out dir> [count]
"""
import json
import os
import random
import sys

ODD = [None, True, 0, 7, -1, 3.5, "", " ", "x", [], {}, [1, 2], ["a"], {"a": 1}]
COLORS = ["#AABBCC", "#aabbcc", "#0xffff", "#ff_f00", "#12345", "#gggggg", "red", "", None, 5, "#ff__00", "# fff00"]
NAMES = ["", "  ", "Name", " pad ", "#12", "#00042", "가" * 70, None, 3, "\x1cx\x1f"]
OB_TYPES = ["direct", "socks5", "http", "shadowsocks", "hysteria2", "group", "block", "subscription", "vless", "bogus"]


def maybe(v, p=0.8):
    return v if random.random() < p else random.choice(ODD)


def maybe_list(v, p=0.85):
    """a list, or something Python reads as "no list" (it iterates whatever is truthy there)"""
    return v if random.random() < p else random.choice([None, [], "", 0, False])


def pos():
    return random.choice([[1, 2], [1.5, -3], [True, 0], [1], "x", None, [1, "a"], [100, 200]])


def main():
    out, count = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 300
    os.makedirs(out, exist_ok=True)
    random.seed(4321)
    for i in range(count):
        pool = [f"b{k}" for k in range(12)] + [f"l{k}" for k in range(16)] + ["missing", "", None]
        def ref():
            # mostly ids that exist (outbounds b0-b4, routers b8-b10, lanes), so links survive and loops form
            r = random.random()
            if r < 0.4:
                return f"b{random.randint(0, 4)}"
            if r < 0.6:
                return f"b{random.randint(8, 10)}"
            if r < 0.85:
                return f"l{random.randint(0, 7)}"
            return random.choice(pool)
        def bid(k):
            # ids are mostly unique, sometimes repeated or missing (new ids are made then)
            r = random.random()
            return f"b{k}" if r < 0.85 else random.choice([f"b{max(k - 1, 0)}", None, "", 5])
        outbounds = []
        for k in range(random.randint(2, 5)):
            outbounds.append({"id": bid(k), "name": random.choice(NAMES),
                              "outbound": {"type": random.choice(OB_TYPES + ["socks5", "http", "shadowsocks", "group"] * 2), "host": random.choice(["", "a.example", "::1"]),
                                           "port": random.choice([0, 443, 70000, "8"]), "interface": random.choice(["", "wg0"])},
                              "via": maybe(ref()), "members": maybe_list([ref() for _ in range(random.randint(0, 4))]),
                              "pos": pos(), "color": random.choice(COLORS)})
        inbounds = []
        for k in range(5, 5 + random.randint(1, 3)):
            inbounds.append({"id": bid(k), "name": random.choice(NAMES), "autostart": maybe(True),
                             "inbound": {"type": random.choice(["socks5", "http", "tun", "bogus"]), "port": random.choice([1080, 0, 70000])},
                             "overrides": maybe({"timeout": random.choice([None, 5, 100000, "x"]), "dns_server": random.choice([None, "1.1.1.1"])}),
                             "target": maybe(ref()), "pos": pos(), "color": random.choice(COLORS)})
        routers = []
        for k in range(8, 8 + random.randint(1, 3)):
            rules = []
            for j in range(random.randint(0, 3)):
                rules.append(maybe({"id": random.choice([f"r{k}{j}", f"r{k}0", None]), "name": random.choice(NAMES),
                                    "match": maybe({"domain": [" A.com ", "", 5], "port": ["80"], "geoip": ["KR"],
                                                    "ip_private": random.choice([True, False, 1])}),
                                    "target": maybe(ref()), "color": random.choice(COLORS)}, 0.9))
            routers.append({"id": bid(k), "name": random.choice(NAMES), "style": random.choice(["router", "if", "x"]),
                            "rules": maybe_list(rules), "default": maybe(ref()), "switch": maybe(True), "switch_on": maybe(True),
                            "pos": pos(), "color": random.choice(COLORS), "default_color": random.choice(COLORS)})
        relays = []
        lane = iter(range(16))
        for k in range(10, 12):
            if random.random() < 0.2:
                continue
            ins = [maybe({"id": f"l{next(lane, 15)}", "gap": random.choice([0, 2, 9, True, "1"]), "out": maybe(ref())}, 0.9)
                   for _ in range(random.randint(0, 3))]
            outs = [maybe({"id": random.choice([f"l{next(lane, 15)}", None]), "gap": random.choice([0, 1]), "target": maybe(ref())}, 0.9)
                    for _ in range(random.randint(0, 3))]
            # routed blocks wire inputs to output lane ids
            for x in ins:
                o = random.choice(outs) if outs else None
                if isinstance(x, dict) and isinstance(o, dict) and random.random() < 0.6:
                    x["out"] = o.get("id")
            relays.append({"id": bid(k), "name": random.choice(NAMES), "routed": maybe(True), "wireless": maybe(random.random() < 0.4),
                           "inputs": maybe(ins), "outputs": maybe(outs), "number": random.choice([1, 2, 0, 10000, None, True]),
                           "name_out": random.choice(NAMES), "pos": pos(), "pos_out": pos(),
                           "broken": random.choice([None, "in", "out", "x"])})
        g = {"outbounds": outbounds, "inbounds": inbounds, "routers": routers, "relays": relays,
             "pc": maybe({"inbound": random.choice(["b5", "b6", ref(), None, 5]), "pos": pos(), "backup": maybe({"a": 1, "b": 1.5, "c": None, "d": True, "e": [1]}),
                          "applied": maybe(True), "server": maybe("127.0.0.1:1080")})}
        with open(os.path.join(out, f"g{i:03}.json"), "w", encoding="utf-8") as f:
            json.dump({"version": 3, "global": {}, "graph": g}, f, ensure_ascii=False)
    print(f"{count} files in {out}")


if __name__ == "__main__":
    main()
