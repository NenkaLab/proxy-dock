"""Build the Direct line VM's guest (a tiny Linux). Needs Docker (arm64 runs emulated where the PC is not arm64).

    python tools/build_bypass_image.py [--arch x64|arm64] [--test]

  x64    Windows Hyper-V: a disk image (GRUB on an EFI partition, kernel, initramfs, squashfs with sing-box)
         -> dist/bypass/proxydock-bypass-x64.vhdx and proxydock-bypass-x64.zip (the release file)
  arm64  macOS Virtualization.framework (Apple silicon), which boots Linux itself: the uncompressed kernel Image,
         the initramfs and the squashfs -> dist/bypass/proxydock-bypass-arm64.zip (vmlinux, initrd, opt.sqfs)

Steps, each in a throwaway container:
  1. pd-agent (native/bypass-guest) built statically with musl in rust:alpine (--test runs its tests first)
  2. native/bypass-guest/image/build.sh in alpine: Alpine linux-virt kernel, a busybox initramfs with pd-agent and
     the few drivers that are modules (Hyper-V's or virtio's), and sing-box (the musl build in
     bin/linux-<arch>-musl, fetched if missing) on a read-only squashfs - paged in from the disk instead of taking
     RAM
  3. x64: raw disk -> VHDX; the zip for the release (the app downloads its own version's the first time a VM is
     prepared, checked against the release's SHA256SUMS.txt)

Each file gets a .sha256, and manifest-<arch>.json lists versions, sizes and hashes. Linux needs no image: there
the Direct line is a network namespace.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GUEST = os.path.join(ROOT, "native", "bypass-guest")
OUT = os.path.join(ROOT, "dist", "bypass")
ALPINE = "alpine:3.22"
RUST = "rust:1-alpine3.22"
# (the guest is musl/busybox: the glibc build of sing-box would not start there)
ARCHES = {
    "x64": {"platform": "linux/amd64", "singbox": "linux-amd64-musl"},
    "arm64": {"platform": "linux/arm64", "singbox": "linux-arm64-musl"},
}


def docker(image, platform, mounts, cmd, env=None):
    args = ["docker", "run", "--rm", "--platform", platform]
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
    ap.add_argument("--arch", choices=sorted(ARCHES), default="x64")
    ap.add_argument("--test", action="store_true", help="run pd-agent's tests before building")
    a = ap.parse_args()
    if shutil.which("docker") is None:
        sys.exit("Docker is needed (docker not found)")
    arch = ARCHES[a.arch]
    work = os.path.join(OUT, f"work-{a.arch}")

    sys.path.insert(0, os.path.join(ROOT, "tools"))
    import fetch_singbox
    from fetch_singbox import SINGBOX_VERSION

    # bin/linux-<arch>-musl: only sing-box (no Hysteria), re-fetched when the pinned version changes
    singbox_dir = os.path.join(ROOT, "bin", arch["singbox"])
    stamp = os.path.join(singbox_dir, ".versions")
    have = open(stamp, encoding="ascii").read().strip() if os.path.isfile(stamp) else ""
    if have != SINGBOX_VERSION or not os.path.isfile(os.path.join(singbox_dir, "sing-box")):
        fetch_singbox.fetch(arch["singbox"])
        with open(stamp, "w", encoding="ascii", newline="\n") as f:
            f.write(SINGBOX_VERSION + "\n")

    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(os.path.join(work, "agent"))

    # cargo's download cache and build folder survive between builds (one build folder per architecture)
    volumes = ["proxydock-bypass-cargo:/usr/local/cargo/registry", f"proxydock-bypass-target-{a.arch}:/target"]
    cargo = "cargo test --release --locked && " if a.test else ""
    docker(RUST, arch["platform"], [f"{GUEST}:/src", f"{os.path.join(work, 'agent')}:/out", *volumes],
           f"cd /src && {cargo}cargo build --release --locked && cp /target/release/pd-agent /out/",
           env={"CARGO_TARGET_DIR": "/target"})

    docker(ALPINE, arch["platform"], [f"{GUEST}:/guest:ro", f"{os.path.join(work, 'agent')}:/agent:ro",
                                      f"{singbox_dir}:/singbox:ro", f"{work}:/out"], "sh /guest/image/build.sh")

    os.makedirs(OUT, exist_ok=True)
    zip_name = f"proxydock-bypass-{a.arch}.zip"
    zip_path = os.path.join(OUT, zip_name)
    files = {}
    if a.arch == "x64":
        name = f"proxydock-bypass-{a.arch}.vhdx"
        docker(ALPINE, arch["platform"], [f"{work}:/work", f"{OUT}:/out"],
               "apk add --no-cache qemu-img > /dev/null && "
               f"rm -f /out/{name} && "
               f"qemu-img convert -f raw -O vhdx -o subformat=dynamic /work/disk.img /out/{name}")
        vhdx = os.path.join(OUT, name)
        files["vhdx"] = {"name": name, "bytes": os.path.getsize(vhdx), "sha256": write_sha(vhdx)}
        with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            z.write(vhdx, name)
    else:
        with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            for part in ("vmlinux", "initrd", "opt.sqfs"):
                z.write(os.path.join(work, part), part)
    files["zip"] = {"name": zip_name, "bytes": os.path.getsize(zip_path), "sha256": write_sha(zip_path)}

    env = dict(line.split("=", 1) for line in open(os.path.join(work, "manifest.env"), encoding="ascii").read().split())
    cargo_toml = open(os.path.join(GUEST, "Cargo.toml"), encoding="utf-8").read()
    agent_version = re.search(r'^version = "([^"]+)"', cargo_toml, re.M).group(1)
    manifest = {
        "arch": a.arch,
        "agent": agent_version,
        "sing_box": SINGBOX_VERSION,
        "kernel": env["KERNEL"],
        "alpine": env["ALPINE"],
        "initrd_bytes": int(env["INITRD_BYTES"]),
        "squashfs_bytes": int(env["SQUASHFS_BYTES"]),
        **files,
    }
    with open(os.path.join(OUT, f"manifest-{a.arch}.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    shutil.rmtree(work, ignore_errors=True)
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
