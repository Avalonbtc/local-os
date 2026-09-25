#!/bin/sh
set -eu
ssh-keygen -A >/dev/null
cat > /etc/ssh/sshd_config.d/rigdeck-lab.conf <<'CONFIG'
PermitRootLogin prohibit-password
PasswordAuthentication no
KbdInteractiveAuthentication no
AuthorizedKeysFile /run/lab-key.pub
StrictModes no
CONFIG
rig-miner recover
rig-miner watchdog >/var/log/rigdeck-watchdog.log 2>&1 &
exec /usr/sbin/sshd -D -e
