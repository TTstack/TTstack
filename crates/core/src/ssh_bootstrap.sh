# Executed once by prepared guest init or cloud-init, never on ordinary restart.
marker=/var/lib/ttstack/ssh-initialized
[ ! -e "$marker" ] || exit 0
command -v sshd >/dev/null
if ! id "$account" >/dev/null 2>&1; then
    if command -v useradd >/dev/null; then
        useradd -m -s /bin/sh "$account"
    else
        adduser -D -s /bin/sh "$account"
    fi
fi
ssh_home=$(awk -F: -v user="$account" '$1 == user {print $6}' /etc/passwd)
[ -n "$ssh_home" ] && [ -d "$ssh_home" ]
# Permit public-key login to a newly created account without a usable password.
printf '%s:*\n' "$account" | chpasswd -e
mkdir -p "$ssh_home/.ssh" /run/sshd /var/lib/ttstack /etc/ssh
chmod 700 "$ssh_home/.ssh"
# Preserve pre-existing user entries even during first-time provisioning.
printf '%s' "$initial_keys" >> "$ssh_home/.ssh/authorized_keys"
chmod 600 "$ssh_home/.ssh/authorized_keys"
chown -R "$account" "$ssh_home/.ssh"
printf '%s' "$host_private" > /etc/ssh/ssh_host_ed25519_key
printf '%s' "$host_public" > /etc/ssh/ssh_host_ed25519_key.pub
chmod 600 /etc/ssh/ssh_host_ed25519_key
chmod 644 /etc/ssh/ssh_host_ed25519_key.pub
if [ "$sudo_access" = yes ]; then
    command -v sudo >/dev/null
    mkdir -p /etc/sudoers.d
    printf '%s ALL=(ALL:ALL) NOPASSWD: ALL\n' "$account" > /etc/sudoers.d/ttstack-initial
    chmod 440 /etc/sudoers.d/ttstack-initial
    visudo -cf /etc/sudoers.d/ttstack-initial >/dev/null
fi
# The managed initial configuration deliberately uses one known host identity.
cat > /etc/ssh/sshd_config <<'SSHD'
Port 22
HostKey /etc/ssh/ssh_host_ed25519_key
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
UsePAM no
Subsystem sftp internal-sftp
SSHD
sshd -t
printf '1\n' > "$marker"
chmod 600 "$marker"
