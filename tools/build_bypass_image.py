"""Build the disk image of the VPN-bypass VM (a tiny Linux guest for Hyper-V). Needs Docker.

    python tools/build_bypass_image.py [--test]

Steps, each in a throwaway container:
  1. pd-agent (native/bypass-guest) built statically with musl in rust:alpine (--test runs its tests first)
  2. native/bypass-guest/image/build.sh in alpine: Alpine linux-virt kernel, a busybox initramfs with pd-agent and
     the few Hyper-V drivers that are modules, GRUB (EFI), and sing-box (the musl build in bin/linux-amd64-musl,
     fetched if missing) on a read-only squashfs partition - it is paged in from the disk instead
     of taking RAM
  3. raw disk -> VHDX (Hyper-V), and a zstd copy for downloading

Output in dist/bypass: proxydock-bypass-x64.vhdx, proxydock-bypass-x64.vhdx.zst, their .sha256 files and
manifest.json (versions, sizes, hashes). Only x64 for now.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GUEST = os.path.join(ROOT, "native", "bypass-guest")
OUT = os.path.join(ROOT, "dist", "bypass")
WORK = os.path.join(OUT, "work")
# the guest is musl/busybox: the glibc build of sing-box would not start there
ARCH, SINGBOX_TARGET = "x64", "linux-amd64-musl"
ALPINE = "alpine:3.22"
RUST = "rust:1-alpine3.22"
# cargo's download cache and build folder survive between builds
CARGO_VOLUMES = ["proxydock-bypass-cargo:/usr/local/cargo/registry", "proxydock-bypass-target:/target"]


def docker(image, mounts, cmd, env=None):
    args = ["docker", "run", "--rm"]
    for m in mounts:
        args += ["-v", m]
    for k, v in (env or {}).items():
        args += ["-e", f"{k}={v}"]
    args += [image, "sh", "-c", cmd]
    print("+", " ".join(args[-1:]))
    # Git Bash would rewrite the /container/paths in the arguments
    subprocess.run(args, check=True, env={**os.environ, "MSYS_NO_PATHCONV": "1"})


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def write_sha(path):
    digest = sha256(path)
    with open(path + ".sha256", "w", encoding="ascii", newline="\n") as f:
        f.write(f"{digest}  {os.path.basename(path)}\n")
    return digest


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--test", action="store_true", help="run pd-agent's tests before building")
    a = ap.parse_args()
    if shutil.which("docker") is None:
        sys.exit("Docker is needed (docker not found)")

    sys.path.insert(0, os.path.join(ROOT, "tools"))
    import fetch_singbox
    from fetch_singbox import SINGBOX_VERSION

    # bin/linux-amd64-musl: only sing-box (no Hysteria), re-fetched when the pinned version changes
    singbox_dir = os.path.join(ROOT, "bin", SINGBOX_TARGET)
    stamp = os.path.join(singbox_dir, ".versions")
    have = open(stamp, encoding="ascii").read().strip() if os.path.isfile(stamp) else ""
    if have != SINGBOX_VERSION or not os.path.isfile(os.path.join(singbox_dir, "sing-box")):
        fetch_singbox.fetch(SINGBOX_TARGET)
        with open(stamp, "w", encoding="ascii", newline="\n") as f:
            f.write(SINGBOX_VERSION + "\n")

    shutil.rmtree(WORK, ignore_errors=True)
    os.makedirs(os.path.join(WORK, "agent"))

    cargo = "cargo test --release --locked && " if a.test else ""
    docker(RUST, [f"{GUEST}:/src", f"{os.path.join(WORK, 'agent')}:/out", *CARGO_VOLUMES],
           f"cd /src && {cargo}cargo build --release --locked && cp /target/release/pd-agent /out/",
           env={"CARGO_TARGET_DIR": "/target"})

    docker(ALPINE, [f"{GUEST}:/guest:ro", f"{os.path.join(WORK, 'agent')}:/agent:ro", f"{singbox_dir}:/singbox:ro",
                    f"{WORK}:/out"], "sh /guest/image/build.sh")

    name = f"proxydock-bypass-{ARCH}.vhdx"
    docker(ALPINE, [f"{WORK}:/work", f"{OUT}:/out"],
           "apk add --no-cache qemu-img zstd > /dev/null && "
           f"rm -f /out/{name} /out/{name}.zst && "
           f"qemu-img convert -f raw -O vhdx -o subformat=dynamic /work/disk.img /out/{name} && "
           f"zstd -q -19 -T0 /out/{name} -o /out/{name}.zst")

    env = dict(line.split("=", 1) for line in open(os.path.join(WORK, "manifest.env"), encoding="ascii").read().split())
    cargo_toml = open(os.path.join(GUEST, "Cargo.toml"), encoding="utf-8").read()
    agent_version = re.search(r'^version = "([^"]+)"', cargo_toml, re.M).group(1)
    vhdx, zst = os.path.join(OUT, name), os.path.join(OUT, name + ".zst")
    manifest = {
        "arch": ARCH,
        "agent": agent_version,
        "sing_box": SINGBOX_VERSION,
        "kernel": env["KERNEL"],
        "alpine": env["ALPINE"],
        "initrd_bytes": int(env["INITRD_BYTES"]),
        "squashfs_bytes": int(env["SQUASHFS_BYTES"]),
        "vhdx": {"name": name, "bytes": os.path.getsize(vhdx), "sha256": write_sha(vhdx)},
        "vhdx_zst": {"name": name + ".zst", "bytes": os.path.getsize(zst), "sha256": write_sha(zst)},
    }
    with open(os.path.join(OUT, "manifest.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    shutil.rmtree(WORK, ignore_errors=True)
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
