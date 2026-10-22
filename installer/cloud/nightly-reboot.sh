#!/bin/bash
# BipAgents: the night's reboot on a server of the all-included offer installed before 2026-10-22 (the newer ones have
# it from their first boot: user-data.yaml, whose settings these are, word for word). Run once as root:
#     curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/installer/cloud/nightly-reboot.sh | bash
# unattended-upgrades then reboots the server by itself when an update asks for it, at 00:00-00:44 server time (a
# minute of its own, from its machine id), after the updates at 23:00-23:30. On a server of the guichet, the seat agent
# is taken again and tells the guichet of each reboot as the server shuts down (a planned reboot is not an outage).
# Running it again changes nothing. ASCII only, like user-data.yaml (installer/tests/test_cloud_init.py checks it).
set -eu
lib=/usr/local/lib/bipagents
repo=https://raw.githubusercontent.com/Alexbob0/BipAgents/main/installer/cloud

cat > /etc/apt/apt.conf.d/52bipagents-reboot <<'EOF'
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-WithUsers "true";
Unattended-Upgrade::Automatic-Reboot-Time "00:20";
EOF
mkdir -p /etc/systemd/system/apt-daily-upgrade.timer.d /etc/needrestart/conf.d
cat > /etc/systemd/system/apt-daily-upgrade.timer.d/60-bipagents.conf <<'EOF'
[Timer]
OnCalendar=
OnCalendar=*-*-* 23:00
RandomizedDelaySec=30min
EOF
cat > /etc/needrestart/conf.d/60-bipagents.conf <<'EOF'
$nrconf{restart} = 'a';
EOF
m=$(( 0x$(cut -c1-4 /etc/machine-id) % 45 )) && printf 'Unattended-Upgrade::Automatic-Reboot-Time "00:%02d";\n' "$m" > /etc/apt/apt.conf.d/53bipagents-reboot-time

if [ -f /etc/bipagents/guichet.json ]; then
    curl -fsSL "$repo/seat-agent.py" -o "$lib/seat-agent.py.new" && mv "$lib/seat-agent.py.new" "$lib/seat-agent.py"
    cat > /etc/systemd/system/bipagents-reboot-notice.service <<EOF
[Unit]
Description=BipAgents: tell the guichet when this server reboots (a planned reboot is no outage)
Wants=network-online.target
After=network-online.target nss-lookup.target systemd-resolved.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/true
ExecStop=/usr/bin/python3 $lib/seat-agent.py --rebooting
TimeoutStopSec=50s

[Install]
WantedBy=multi-user.target
EOF
fi
systemctl daemon-reload
if [ -f /etc/systemd/system/bipagents-reboot-notice.service ]; then
    systemctl enable --now bipagents-reboot-notice.service
fi
echo "nightly reboot set: $(cut -d'"' -f2 /etc/apt/apt.conf.d/53bipagents-reboot-time) server time, when an update asks"
