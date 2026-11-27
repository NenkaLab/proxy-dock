#!/bin/sh
# Runs in alpine (see tools/build_bypass_image.py). Inputs: /guest (native/bypass-guest), /agent/pd-agent,
# /singbox/sing-box (+ LICENSE). Output and /out/manifest.env:
#   x86_64 (Hyper-V): /out/disk.img (raw, GPT: 1 = EFI system partition with GRUB, kernel and initramfs,
#                     2 = squashfs with sing-box)
#   aarch64 (macOS Virtualization.framework, which boots Linux itself): /out/vmlinux (the uncompressed Image),
#                     /out/initrd and /out/opt.sqfs
set -eu
ARCH=$(uname -m)
if [ "$ARCH" = x86_64 ]; then
    apk add --no-cache linux-virt kmod grub-efi mtools sgdisk squashfs-tools cpio > /dev/null
    MODULES="hv_utils af_packet vsock hv_sock hv_storvsc sd_mod squashfs"
else
    apk add --no-cache linux-virt kmod squashfs-tools cpio > /dev/null
    MODULES="af_packet vsock vmw_vsock_virtio_transport virtio_net virtio_blk squashfs"
fi
KV=$(ls /lib/modules)
R=/rootfs
mkdir -p $R/etc/apk
cp -a /etc/apk/keys $R/etc/apk/
cp /etc/apk/repositories $R/etc/apk/
apk add --no-cache --root $R --initdb busybox busybox-binsh > /dev/null
rm -rf $R/etc/apk $R/lib/apk $R/var/cache
# the modules /init loads and what they depend on, decompressed (busybox modprobe reads plain .ko)
for m in $MODULES; do
    modprobe -S "$KV" --show-depends "$m" | awk '/^insmod/ {print $2}'
done | sort -u | while read -r f; do
    d=$R$(dirname "$f")
    mkdir -p "$d"
    gunzip -c "$f" > "$d/$(basename "$f" .gz)"
done
cp /lib/modules/$KV/modules.builtin /lib/modules/$KV/modules.builtin.modinfo /lib/modules/$KV/modules.order $R/lib/modules/$KV/
depmod -b $R "$KV"
install -m 755 /agent/pd-agent $R/sbin/pd-agent
install -m 755 /guest/image/init $R/init
echo "proxydock-bypass" > $R/etc/hostname
(cd $R && find . | cpio -o -H newc 2> /dev/null | gzip -9) > /tmp/initrd

mkdir -p /tmp/sq
cp /singbox/sing-box /tmp/sq/sing-box
chmod 755 /tmp/sq/sing-box
[ -f /singbox/LICENSE ] && cp /singbox/LICENSE /tmp/sq/LICENSE.sing-box
mksquashfs /tmp/sq /tmp/opt.sqfs -comp zstd -noappend -quiet -all-root

if [ "$ARCH" = x86_64 ]; then
    grub-mkimage -O x86_64-efi -p /EFI/BOOT -c /guest/image/grub.cfg -o /tmp/BOOTX64.EFI \
        part_gpt fat normal linux search search_fs_file configfile gzio echo efi_gop all_video

    # partition sizes in MiB: the ESP fits kernel + initramfs + GRUB with room, the squashfs partition is rounded up
    ESP=32
    SQ=$(( ($(stat -c %s /tmp/opt.sqfs) + 1048575) / 1048576 + 1 ))
    TOTAL=$((1 + ESP + SQ + 1))
    truncate -s ${TOTAL}M /out/disk.img
    sgdisk -n1:1M:+${ESP}M -t1:ef00 -c1:EFI -n2:0:+${SQ}M -t2:8300 -c2:opt /out/disk.img > /dev/null
    mformat -i /out/disk.img@@1M -F -v PDBYPASS ::
    mmd -i /out/disk.img@@1M ::/EFI ::/EFI/BOOT
    mcopy -i /out/disk.img@@1M /tmp/BOOTX64.EFI ::/EFI/BOOT/BOOTX64.EFI
    mcopy -i /out/disk.img@@1M /boot/vmlinuz-virt ::/vmlinuz
    mcopy -i /out/disk.img@@1M /tmp/initrd ::/initrd
    dd if=/tmp/opt.sqfs of=/out/disk.img bs=1M seek=$((1 + ESP)) conv=notrunc status=none
else
    # Alpine's arm64 kernel is an EFI "zimg" wrapper around a gzip Image; Virtualization.framework wants the Image
    OFF=$(od -A n -t u4 -j 8 -N 4 /boot/vmlinuz-virt | tr -d ' ')
    SIZE=$(od -A n -t u4 -j 12 -N 4 /boot/vmlinuz-virt | tr -d ' ')
    tail -c +$((OFF + 1)) /boot/vmlinuz-virt | head -c "$SIZE" | gunzip > /out/vmlinux
    [ "$(dd if=/out/vmlinux bs=1 skip=56 count=4 2> /dev/null | od -A n -t x1 | tr -d ' ')" = 41524d64 ] || { echo "not an arm64 Image"; exit 1; }
    cp /tmp/initrd /out/initrd
    cp /tmp/opt.sqfs /out/opt.sqfs
fi
{
    echo "KERNEL=$KV"
    echo "ALPINE=$(cat /etc/alpine-release)"
    echo "INITRD_BYTES=$(stat -c %s /tmp/initrd)"
    echo "SQUASHFS_BYTES=$(stat -c %s /tmp/opt.sqfs)"
} > /out/manifest.env
cat /out/manifest.env
