#!/bin/bash
# The Direct line on Linux, end to end, in a privileged container (network namespaces need it), from Windows or
# macOS:
#     docker run --rm --privileged -v "<repo>:/src" -v "<repo>/bin:/target/bin:ro" \
#         -v proxydock-target:/target -v proxydock-cargo:/usr/local/cargo/registry rust:1-bookworm \
#         bash /src/tools/docker_linux_bypass_check.sh
# A firewall rule in the container's own namespace drops everything leaving eth0 (as a VPN's kill switch would);
# the Direct line (a macvlan copy of eth0 in a namespace of its own, a fixed address in Docker's network) must
# still get out, TCP and UDP. Needs bin/linux-amd64/sing-box (python tools/fetch_singbox.py linux-amd64).
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null && apt-get install -y -qq iproute2 curl iptables >/dev/null 2>&1
cd /src/native
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/target}
cargo build -q -p proxydock-core --example bypass_e2e --bin bypass_helper
cargo test -q -p proxydock-core --lib bypass
ADDR=$(ip -4 -o addr show dev eth0 | awk '{print $4}')
GW=$(ip -4 route show default | awk '{print $3}')
echo "container eth0 $ADDR gw $GW"
# a kill switch in the main namespace: nothing leaves eth0 from here
iptables -I OUTPUT -o eth0 -j DROP
echo "main namespace (must fail): $(curl -s -m 5 -o /dev/null -w '%{http_code}' http://example.com/ || echo fail)"
export PROXYDOCK_BYPASS_ADAPTER=eth0
export PROXYDOCK_BYPASS_STATIC="172.17.0.250/${ADDR#*/},$GW,8.8.8.8"
"$CARGO_TARGET_DIR/debug/examples/bypass_e2e"
echo "namespaces left (must be 0): $(ip netns list | wc -l)"
