#!/bin/bash
# BipAgents: one more person on a shared server of the all-included offer, in an account of their own.
# Run as root at first boot, after the first place (installer/cloud/user-data.yaml), and by the seat agent
# (installer/cloud/seat-agent.py) when the guichet orders a place installed again for its next person:
#     add-seat.sh <n> <setup file>
# The setup file (root's, 0600) comes from the guichet over HTTPS, never from the first-boot script the provider keeps:
# fetched at first boot with a one-time token (seat-agent.py --bootstrap), later in a signed order.
# The account bip<n> (bip for place 1; without sudo), its own range of user ids for its agents' sandboxes, its door's
# port (from the setup file: 443 for place 1, 444, 445 ... up to 482 for place 40) let in by the firewall, then the installer's --cloud mode
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
# The BipAgents version the guichet wants installed (a commit's 40 characters; none: the latest).
version=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1])).get("version") or "")' "$setup") || exit 1
pin=""
if [ -n "$version" ]; then
    case "$version" in
        *[!0-9a-f]*) echo "add-seat.sh: a version is a commit's 40 characters" >&2; exit 2 ;;
    esac
    [ "${#version}" -eq 40 ] || { echo "add-seat.sh: a version is a commit's 40 characters" >&2; exit 2; }
    pin="BIPAGENTS_TO=$version "
fi

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
# Its soft memory ceiling (MemoryHigh on its slice, a share of the server's memory) before its install's heavy steps;
# the seat agent keeps it set every two minutes after.
here=$(dirname "$0")
[ -f "$here/seat-agent.py" ] && python3 "$here/seat-agent.py" --ceiling >/dev/null 2>&1
# Hermes and the agents' images of the version it installs, ready-made and shared by the server's places (usually
# there already: nothing to fetch). Its install then points at them instead of fetching its own. Each phase's time
# goes to /var/log/bipagents-timeline.log.
echo "$(date -u +%FT%TZ) place $n shared start" >> /var/log/bipagents-timeline.log
[ -f "$here/seat-agent.py" ] && python3 "$here/seat-agent.py" --prepare "${version:-main}" >/dev/null 2>&1
echo "$(date -u +%FT%TZ) place $n install start" >> /var/log/bipagents-timeline.log
sudo -iu "$user" bash -c "curl -fsSL https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh | ${pin}bash -s -- --cloud $setup" \
    > "/var/log/bipagents-install$n.log" 2>&1
echo "$(date -u +%FT%TZ) place $n install done" >> /var/log/bipagents-timeline.log
# The installer kept the secret for the bridge (deleted once the app has paired); this copy goes.
shred -u "$setup" 2>/dev/null || rm -f "$setup"
