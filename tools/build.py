"""ProxyDock 빌드 도구.

    python tools/build.py win|linux|mac|all

  win    Windows 실행 파일   -> dist/win/ProxyDock.exe   (Windows 에서만)
  linux  Linux 실행 파일     -> dist/linux/ProxyDock     (Linux 에서는 직접, Windows/macOS 에서는 Docker 로)
  mac    macOS 앱 번들       -> dist/mac/ProxyDock.app   (macOS 에서만)
  all    이 컴퓨터에서 가능한 것만 모두 빌드하고 결과를 요약

각 OS 용 가상환경(.venv / .venv-linux / .venv-mac)을 만들고, requirements.txt 와 설치된 패키지를
비교해 빠졌거나 버전이 다른 것만 설치한 뒤 ProxyDock.spec 으로 빌드한다.
배치/셸 스크립트(build*.bat, build*.sh)는 이 파일을 부르기만 한다 (한글 출력이 콘솔 코드 페이지에 따라
깨지지 않도록 로직은 모두 여기에 둔다).
"""
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC = "ProxyDock.spec"
DOCKER_IMAGE = "proxydock-linux-build"
PLATFORM = "win" if sys.platform == "win32" else "mac" if sys.platform == "darwin" else "linux"
NAMES = {"win": "Windows", "linux": "Linux", "mac": "macOS"}
OUTPUTS = {"win": "dist/win/ProxyDock.exe", "linux": "dist/linux/ProxyDock", "mac": "dist/mac/ProxyDock.app"}


class Skip(Exception):
    """이 컴퓨터에서는 빌드할 수 없음 (실패가 아님)."""


def say(target, step, msg):
    print(f"[{target} {step}] {msg}", flush=True)


def run(cmd, **kw):
    print("    $ " + " ".join(cmd), flush=True) if os.environ.get("BUILD_VERBOSE") else None
    return subprocess.run(cmd, cwd=ROOT, **kw)


def venv_python(target):
    default = {"win": ".venv", "linux": ".venv-linux", "mac": ".venv-mac"}[target]
    venv = os.environ.get("VENV_DIR") or os.path.join(ROOT, default)
    py = os.path.join(venv, "Scripts", "python.exe") if target == "win" else os.path.join(venv, "bin", "python")
    if os.path.exists(py):
        say(target, "1/4", f"가상환경 확인: {os.path.relpath(venv, ROOT) if venv.startswith(ROOT) else venv}")
    else:
        if sys.version_info[:2] < (3, 14):
            print(f"    주의: Python {sys.version.split()[0]} 로 가상환경을 만듭니다 (권장 3.14)")
        say(target, "1/4", f"Python {sys.version.split()[0]} 가상환경 만드는 중: {venv}")
        if run([sys.executable, "-m", "venv", venv]).returncode != 0:
            raise RuntimeError("가상환경을 만들지 못했습니다")
    return py


def check_requirements(target, py):
    say(target, "2/4", "requirements.txt 확인...")
    if not os.path.exists(os.path.join(ROOT, "requirements.txt")):
        raise RuntimeError("requirements.txt 가 없습니다")
    r = run([py, "-m", "pip", "install", "--dry-run", "--disable-pip-version-check", "-r", "requirements.txt"],
            capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.returncode != 0:
        print(r.stdout + r.stderr)
        raise RuntimeError("requirements.txt 확인에 실패했습니다")
    need = [l for l in r.stdout.splitlines() if l.startswith("Would install")]
    if not need:
        print("          모든 패키지가 requirements.txt 와 일치합니다.")
    else:
        for line in need:
            print(f"          설치 필요: {line[len('Would install '):]}")
        print("          설치 중...", flush=True)
        if run([py, "-m", "pip", "install", "--disable-pip-version-check", "-q", "-r", "requirements.txt"]).returncode:
            raise RuntimeError("패키지 설치에 실패했습니다")
    c = run([py, "-m", "pip", "check", "--disable-pip-version-check"], capture_output=True, text=True,
            encoding="utf-8", errors="replace")
    if c.returncode != 0:
        print("경고: 설치된 패키지끼리 버전이 맞지 않습니다.\n" + c.stdout)


def pyinstaller(target, py):
    say(target, "3/4", "빌드...")
    r = run([py, "-m", "PyInstaller", "--noconfirm", "--clean", "--log-level", "WARN",
             "--distpath", f"dist/{target}", "--workpath", f"build/{target}", SPEC])
    if r.returncode != 0:
        raise RuntimeError("PyInstaller 빌드에 실패했습니다")
    if target == "mac":     # .app 번들을 만들고 남는 중간 폴더
        shutil.rmtree(os.path.join(ROOT, "dist", "mac", "ProxyDock"), ignore_errors=True)


def copy_licenses(target):
    say(target, "4/4", "라이선스 복사...")
    dst = os.path.join(ROOT, "dist", target, "licenses")
    os.makedirs(dst, exist_ok=True)
    src = os.path.join(ROOT, "licenses")
    for name in os.listdir(src):
        if name.endswith(".txt"):
            shutil.copy2(os.path.join(src, name), dst)


def build_native(target):
    py = venv_python(target)
    check_requirements(target, py)
    pyinstaller(target, py)
    copy_licenses(target)
    print(f"완료: {OUTPUTS[target]}")
    if target == "mac":
        print("참고: 서명/공증되지 않은 앱이라 다른 Mac 에서는 처음 실행 시 우클릭 -> 열기 가 필요할 수 있습니다.")


def build_linux_docker():
    """Windows/macOS 에서 Docker 컨테이너 안에서 Linux 빌드. 이미지와 패키지는 캐시된다."""
    if shutil.which("docker") is None:
        raise Skip("Docker 가 설치되어 있지 않습니다 (Docker Desktop 필요)")
    if run(["docker", "info"], capture_output=True).returncode != 0:
        raise Skip("Docker 가 실행 중이 아닙니다 (Docker Desktop 을 실행하세요)")
    say("linux", "0/4", "빌드 이미지 준비 (처음 한 번만 오래 걸림)...")
    if run(["docker", "build", "-q", "-t", DOCKER_IMAGE, "docker/linux"], capture_output=True).returncode:
        raise RuntimeError("Docker 이미지를 만들지 못했습니다 (docker build docker/linux)")
    r = run(["docker", "run", "--rm",
             "-v", f"{ROOT}:/src",
             "-v", "proxydock-linux-venv:/opt/venv",        # 가상환경/패키지 캐시 (다음 빌드부터 빠름)
             "-v", "proxydock-pip-cache:/root/.cache/pip",
             "-e", "VENV_DIR=/opt/venv", "-e", "PYTHONIOENCODING=utf-8",
             "-w", "/src", DOCKER_IMAGE, "python", "tools/build.py", "linux"])
    if r.returncode != 0:
        raise RuntimeError("컨테이너 안에서 빌드에 실패했습니다")


def build(target):
    if target == PLATFORM:
        build_native(target)
    elif target == "linux":
        build_linux_docker()
    elif target == "mac":
        raise Skip("macOS 앱은 Mac 에서만 빌드할 수 있습니다 (Mac 에서 build_mac.sh)")
    else:
        raise Skip("Windows 실행 파일은 Windows 에서만 빌드할 수 있습니다 (Windows 에서 build_win.bat)")


def main(argv):
    if len(argv) != 2 or argv[1] not in ("win", "linux", "mac", "all"):
        print(__doc__)
        return 64
    target = argv[1]
    if target != "all":
        try:
            build(target)
            return 0
        except Skip as e:
            print(f"건너뜀: {e}")
            return 2
        except RuntimeError as e:
            print(f"실패: {e}")
            return 1

    results = {}
    for t in ("win", "linux", "mac"):
        try:
            build(t)
            results[t] = f"성공   - {OUTPUTS[t]}"
        except Skip as e:
            results[t] = f"건너뜀 - {e}"
        except RuntimeError as e:
            results[t] = f"실패   - {e}"
        print()
    print("===== 빌드 결과 =====")
    for t, r in results.items():
        print(f"  {NAMES[t]:8}: {r}")
    return 1 if any(r.startswith("실패") for r in results.values()) else 0


if __name__ == "__main__":
    # Windows 콘솔/파이프 어디서든 한글이 깨지지 않게
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8" if not stream.isatty() else stream.encoding, errors="replace")
        except (AttributeError, ValueError):
            pass
    sys.exit(main(sys.argv))
