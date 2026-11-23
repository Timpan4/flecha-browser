#!/usr/bin/env bash
set -euo pipefail

# Run before the sandbox starts any of its own services. Check the
# mounted runtime, build and toolkit trees as well as the private mounts.
roots=(/usr /etc /dev /run /tmp /var /mnt)
for directory in /bin /sbin /lib /lib64; do
  if [[ -d $directory && ! -L $directory ]]; then roots+=("$directory"); fi
done
sockets=$(find "${roots[@]}" -type s -print)
if [[ -n $sockets || -e /run/systemd/resolve || -e /var/run/systemd/resolve ]]; then
  printf 'Host runtime sockets are visible inside the launcher\n' >&2
  exit 1
fi
getent hosts localhost >/dev/null
if getent hosts localtest.me >/dev/null; then
  printf 'Public name localtest.me resolved inside the launcher\n' >&2
  exit 1
else
  status=$?
  # getent exit status 2 means the supplied key was absent.
  if [[ $status != 2 ]]; then
    printf 'Resolver check was inconclusive (getent exited %s)\n' "$status" >&2
    exit 1
  fi
fi
printf 'Isolation check passed: runtime sockets absent, localhost resolved, localtest.me unresolved\n'
