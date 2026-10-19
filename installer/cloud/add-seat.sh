#!/bin/bash
# BipAgents: one more person on a shared server of the all-included offer, in an account of their own.
# Run as root at first boot, after the first seat (installer/cloud/user-data.yaml):
#     add-seat.sh <n> <setup file>
# The account bip<n> (without sudo), its own range of user ids for its agents' sandboxes, its door's port (from the
# setup file: 444 for the second seat, 445, 446) let in by the firewall, then the installer's --cloud mode as that
# account. The voices already running on the server are shared. Its log: /var/log/bipagents-install<n>.log.
# ASCII only, like user-data.yaml (installer/tests/test_cloud_init.py checks both).
set -u
n="${1:-}"
setup="${2:-}"
case "$n" in
    '' | *[!0-9]*) echo "usage: add-seat.sh <seat number, 2 and up> <setup file>" >&2; exit 2 ;;
esac
if [ "$n" -lt 2 ] || [ ! -f "$setup" ]; then
    echo "usage: add-seat.sh <seat number, 2 and up> <setup file>" >&2
    exit 2
fi
user="bip$n"
port=$(python3 -c 'import json, sys; print(int(json.load(open(sys.argv[1])).get("port") or 443))' "$setup") || exit 1

id -u "$user" >/dev/null 2>&1 || useradd --create-home --shell /bin/bash "$user"
passwd --lock "$user" >/dev/null
# Its agents' sandboxes (rootless podman): a range of user ids no other seat has (the first seat has 200000 and up).
first=$((200000 + 100000 * n))
grep -q "^$user:" /etc/subuid || usermod --add-subuids "$first-$((first + 65535))" --add-subgids "$first-$((first + 65535))" "$user"
chown "$user:$user" "$setup"
chmod 600 "$setup"
ufw allow "$port/tcp" >/dev/null
# Its services run when nobody is logged in.
loginctl enable-linger "$user"
systemctl start "user@$(id -u "$user").service"
sudo -iu "$user" bash -c "curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | bash -s -- --cloud $setup" \
    > "/var/log/bipagents-install$n.log" 2>&1
# The installer kept the secret for the bridge (deleted once the app has paired); this copy goes.
shred -u "$setup" 2>/dev/null || rm -f "$setup"
