#!/bin/bash
# BipAgents: what a server must also do for its places' encrypted spaces (installer/bipinstall/space.py,
# docs/space-encryption.md) to mean something: the open spaces' data passes through memory, and memory reaches the
# disk through the swap file, crash dumps and logs. Run once as root (cloud-init will, once encryption is on by
# default on the offer); running it again changes nothing:
#     space-hardening.sh
# - the swap file encrypted with a new random key at each boot (systemd-cryptsetup, plain mode: nothing to keep);
# - no core dumps kept (systemd-coredump, apport), none written by the kernel;
# - the journal in memory only (lost at a reboot; 200 MB at most);
# - gocryptfs and FUSE installed, and user_allow_other (the agents' browser containers read the open folder).
# ASCII only, like user-data.yaml (installer/tests/test_cloud_init.py checks it).
set -eu

if command -v apt-get >/dev/null 2>&1; then
    DEBIAN_FRONTEND=noninteractive apt-get install -y gocryptfs fuse3 cryptsetup-bin >/dev/null
elif command -v dnf >/dev/null 2>&1; then
    dnf install -y gocryptfs fuse3 cryptsetup >/dev/null
fi
grep -q '^user_allow_other' /etc/fuse.conf 2>/dev/null || echo 'user_allow_other' >> /etc/fuse.conf

# Swap: the file becomes the backing of an encrypted device, keyed from /dev/urandom at each boot.
if grep -q '^/swapfile ' /etc/fstab; then
    swapoff /swapfile 2>/dev/null || true
    sed -i 's|^/swapfile .*|/dev/mapper/bipswap none swap sw,nofail 0 0|' /etc/fstab
    grep -q '^bipswap ' /etc/crypttab 2>/dev/null \
        || echo 'bipswap /swapfile /dev/urandom swap,plain,cipher=aes-xts-plain64,size=512,nofail' >> /etc/crypttab
    systemctl daemon-reload
    systemctl start systemd-cryptsetup@bipswap.service && swapon /dev/mapper/bipswap || true
fi

# Core dumps: none kept.
mkdir -p /etc/systemd/coredump.conf.d
printf '[Coredump]\nStorage=none\nProcessSizeMax=0\n' > /etc/systemd/coredump.conf.d/60-bipagents.conf
printf 'kernel.core_pattern = |/bin/false\nfs.suid_dumpable = 0\n' > /etc/sysctl.d/61-bipagents-space.conf
sysctl --system >/dev/null
if systemctl list-unit-files apport.service >/dev/null 2>&1; then
    systemctl disable --now apport.service >/dev/null 2>&1 || true
fi

# The journal in memory.
mkdir -p /etc/systemd/journald.conf.d
printf '[Journal]\nStorage=volatile\nRuntimeMaxUse=200M\n' > /etc/systemd/journald.conf.d/61-bipagents-space.conf
systemctl restart systemd-journald
rm -rf /var/log/journal/* 2>/dev/null || true

echo "space hardening done: swap encrypted at each boot, no core dumps, journal in memory"
