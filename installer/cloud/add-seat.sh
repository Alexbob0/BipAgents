#!/bin/bash
# BipAgents: one more person on a shared server of the all-included offer, in an account of their own.
# Run as root at first boot, after the first place (installer/cloud/user-data.yaml), and by the seat agent
# (installer/cloud/seat-agent.py) when the guichet orders a place installed again for its next person:
#     add-seat.sh <n> <setup file>
# The account bip<n> (bip for place 1; without sudo), its own range of user ids for its agents' sandboxes, its door's
# port (from the setup file: 443 for place 1, 444, 445, 446) let in by the firewall, then the installer's --cloud mode
# as that account. The voices already running on the server are shared. Its log: /var/log/bipagents-install<n>.log.
# ASCII only, like user-data.yaml (installer/tests/test_cloud_init.py checks both).
set -u
n="${1:-}"
setup="${2:-}"
case "$n" in
    '' | *[!0-9]*) echo "usage: add-seat.sh <place number, 1 and up> <setup file>" >&2; exit 2 ;;
esac
if [ "$n" -lt 1 ] || [ ! -f "$setup" ]; then
    echo "usage: add-seat.sh <place number, 1 and up> <setup file>" >&2
    exit 2
fi
user="bip$n"
[ "$n" -eq 1 ] && user="bip"
port=$(python3 -c 'import json, sys; print(int(json.load(open(sys.argv[1])).get("port") or 443))' "$setup") || exit 1

id -u "$user" >/dev/null 2>&1 || useradd --create-home --shell /bin/bash "$user"
passwd --lock "$user" >/dev/null
# Its agents' sandboxes (rootless podman): a range of user ids no other place has (user-data gives the first install of
# place 1 200000 and up; place n gets 200000 + 100000 * n, place 1 installed again 300000).
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
