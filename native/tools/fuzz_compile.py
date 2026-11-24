"""Random well-formed graphs (links mostly valid) for comparing graph.compile(): routers leading to routers and
IF switches, via chains, groups, pass-through lanes, inbounds relayed into other inbounds.

    python native/tools/fuzz_compile.py <out dir> [count]
"""
import json
import os
import random
import sys

MATCHES = [{"domain_suffix": ["kr"]}, {"geoip": ["jp"], "port": ["443"]}, {"ip_private": True}, {"process_name": ["game.exe"]},
           {}, {"domain": ["A.example"], "ip_cidr": ["10.0.0.0/8"]}]


def main():
    out, count = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 300
    os.makedirs(out, exist_ok=True)
    random.seed(99)
    for i in range(count):
        ob_ids = [f"o{k}" for k in range(random.randint(2, 6))]
        rt_ids = [f"r{k}" for k in range(random.randint(1, 4))]
        in_ids = [f"i{k}" for k in range(random.randint(1, 4))]
        lanes = [f"l{k}" for k in range(random.randint(0, 4))]
        targets = ob_ids + rt_ids
        outbounds = []
        for k, oid in enumerate(ob_ids):
            t = random.choice(["socks5", "http", "shadowsocks", "direct", "hysteria2", "group", "block", "vless"])
            n = {"id": oid, "name": random.choice(["", f"출구 {k}"]),
                 "outbound": {"type": t, "host": "s.example", "port": 1000 + k, "password": random.choice(["", "pw"]),
                              "ss_password": "ss-pw"},
                 "via": random.choice([None] + ob_ids)}
            if t == "group":
                n["members"] = random.sample(ob_ids, k=random.randint(1, len(ob_ids)))
            outbounds.append(n)
        routers = []
        for rid in rt_ids:
            style = random.choice(["router", "router", "if"])
            rules = [{"id": f"{rid}x{j}", "name": random.choice(["", f"규칙{j}"]), "match": random.choice(MATCHES),
                      "target": random.choice(targets + lanes + [None])} for j in range(random.randint(1, 4))]
            routers.append({"id": rid, "name": rid, "style": style, "rules": rules,
                            "default": random.choice(targets + lanes + [None]),
                            "switch": random.random() < 0.3, "switch_on": random.random() < 0.5})
        relays = []
        if lanes:
            routed = random.random() < 0.5
            ins = [{"id": lid, "out": None} for lid in lanes]
            outs = [{"id": f"{lid}o", "target": random.choice(targets + [None])} for lid in lanes]
            if routed:
                for x in ins:
                    x["out"] = random.choice(outs)["id"]
            relays.append({"id": "relay", "routed": routed, "inputs": ins, "outputs": outs})
        inbounds = []
        for k, iid in enumerate(in_ids):
            inbounds.append({"id": iid, "name": f"in{k}",
                             "inbound": {"type": random.choice(["socks5", "http", "shadowsocks", "hysteria2", "tun"]),
                                         "port": 2000 + k, "auth_enabled": random.random() < 0.5, "username": "u",
                                         "password": "p", "ss_password": "sp", "hy2_password": "hp",
                                         "host": random.choice(["127.0.0.1", "0.0.0.0", "::"])},
                             "target": random.choice(targets + lanes + in_ids + [None])})
        g = {"outbounds": outbounds, "inbounds": inbounds, "routers": routers, "relays": relays}
        with open(os.path.join(out, f"k{i:03}.json"), "w", encoding="utf-8") as f:
            json.dump({"version": 3, "global": {}, "graph": g}, f, ensure_ascii=False)
    print(f"{count} files in {out}")


if __name__ == "__main__":
    main()
