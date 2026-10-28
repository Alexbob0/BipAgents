#!/bin/bash
# BipAgents: what a server must also do for its places' encrypted spaces (installer/bipinstall/space.py,
# internal/space-encryption.md, private) to mean something: the open spaces' data passes through memory, and memory reaches the
# disk through the swap file, crash dumps and logs. Run as root before any place whose setup carries "keys" is
# installed: by the first-boot commands (user-data.yaml, which ships this script as
# /usr/local/lib/bipagents/space-hardening.sh) and by add-seat.sh. Done once: /var/lib/bipagents/space-hardened says
# so (the seat agent tells the guichet), and running it again changes nothing:
#     space-hardening.sh
# - the swap file emptied, then encrypted with a new random key at each boot (systemd-cryptsetup, plain mode:
#   nothing to keep);
# - no core dumps kept (systemd-coredump, apport), none written by the kernel;
# - the journal in memory only (lost at a reboot; 200 MB at most), what it had kept on the disk removed;
# - gocryptfs and FUSE installed, and user_allow_other (the agents' browser containers read the open folder).
# Any step that fails stops it with an error: the place is then not installed.
# ASCII only, like user-data.yaml (installer/tests/test_cloud_init.py checks it).
set -eu

DONE=/var/lib/bipagents/space-hardened
if [ -f "$DONE" ]; then
    echo "space hardening already done ($(cat "$DONE"))"
    exit 0
fi

if command -v apt-get >/dev/null 2>&1; then
    DEBIAN_FRONTEND=noninteractive apt-get install -y gocryptfs fuse3 cryptsetup-bin >/dev/null
elif command -v dnf >/dev/null 2>&1; then
    dnf install -y gocryptfs fuse3 cryptsetup >/dev/null
fi
grep -q '^user_allow_other' /etc/fuse.conf 2>/dev/null || echo 'user_allow_other' >> /etc/fuse.conf

# Swap: the file is taken out of use and overwritten with zeros (what memory put there in clear goes), then becomes
# the backing of an encrypted device, keyed from /dev/urandom at each boot.
if grep -q '^/swapfile ' /etc/fstab; then
    if grep -q '^/swapfile ' /proc/swaps; then
        swapoff /swapfile
    fi
    size_mb=$(( $(stat -c %s /swapfile) / 1048576 ))
    dd if=/dev/zero of=/swapfile bs=1M count="$size_mb" conv=notrunc,fsync status=none
    sed -i 's|^/swapfile .*|/dev/mapper/bipswap none swap sw,nofail 0 0|' /etc/fstab
    grep -q '^bipswap ' /etc/crypttab 2>/dev/null \
        || echo 'bipswap /swapfile /dev/urandom swap,plain,cipher=aes-xts-plain64,size=512,nofail' >> /etc/crypttab
fi
if grep -q '^bipswap ' /etc/crypttab 2>/dev/null; then
    systemctl daemon-reload
    [ -e /dev/mapper/bipswap ] || systemctl start systemd-cryptsetup@bipswap.service
    if ! swapon --show=NAME --noheadings | grep -qx "$(readlink -f /dev/mapper/bipswap)"; then
        swapon /dev/mapper/bipswap
    fi
fi

# Core dumps: none kept.
mkdir -p /etc/systemd/coredump.conf.d
printf '[Coredump]\nStorage=none\nProcessSizeMax=0\n' > /etc/systemd/coredump.conf.d/60-bipagents.conf
printf 'kernel.core_pattern = |/bin/false\nfs.suid_dumpable = 0\n' > /etc/sysctl.d/61-bipagents-space.conf
sysctl --system >/dev/null
if systemctl list-unit-files apport.service >/dev/null 2>&1; then
    systemctl disable --now apport.service >/dev/null 2>&1 || true   # not installed everywhere
fi

# The journal in memory, and what it kept on the disk so far removed (before any place runs).
mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\nStorage=volatile\nRuntimeMaxUse=200M\n' > /etc/systemd/journald.conf.d/61-bipagents-space.conf
systemctl restart systemd-journald
rm -rf /var/log/journal

mkdir -p "$(dirname "$DONE")"
date -u +%FT%TZ > "$DONE"
echo "space hardening done: swap emptied and encrypted at each boot, no core dumps, journal in memory"
