# Initial VM SSH access

QEMU and prepared Firecracker guests accept initial OpenSSH public keys and
`ssh: {"user":"user","sudo":true}` in each VM specification. CLI equivalents are
`--ssh-key PATH --ssh-user user --ssh-sudo`. Supplying keys without `ssh` selects
`root` without an additional sudo grant. An explicit SSH configuration requires a
key. Docker has no managed SSH bootstrap. The scheduler requires `ssh_bootstrap`.

For example, with a prepared image named `fc-ssh` already available on a host:

```sh
tt env create work --image fc-ssh --engine firecracker \
  --cpu 1 --mem 512 --lifetime 0 \
  --ssh-key ~/.ssh/id_ed25519.pub --ssh-user user --ssh-sudo
tt env show work
```

Use the displayed endpoint after SSH becomes ready. A remote endpoint also needs
the agent's [public-address configuration](#reachable-endpoint). `ssh.user` is
1–32 characters, starting with a lowercase letter and followed by lowercase
letters, digits, `_` or `-`. CLI account/sudo options accompany `--ssh-key`.

TTstack receives no login private key. The caller owns key generation, custody
and download. Initial provisioning creates the account, installs its public keys,
generates an independent Ed25519 host identity and, when requested, grants
`USER ALL=(ALL:ALL) NOPASSWD: ALL`. This is unrestricted guest administration.
TTstack does not protect guest applications/configuration from that account.
Host credentials and other VMs remain outside its authority.
Initial provisioning writes the guest's SSH daemon configuration for key-only
login using that Ed25519 identity; prepared images must support this setup.

## Image contract

Guests need OpenSSH and account tools; `sudo:true` also requires sudo/visudo.
QEMU needs cloud-init and a working SSH service. Its private NoCloud ISO runs the
initial script. Firecracker needs an ext4 image containing
`/etc/ttstack/ssh-bootstrap-version` with content `1`. Its init mounts the read-only
configuration drive and executes `ttstack-ssh.sh` before starting sshd independently
of applications. That filename is reserved; callers cannot supply it themselves.
The configuration drive reserves 4 MiB, including an SSH-only Firecracker seed.
The built-in `fc-alpine` smoke image does not meet this contract.

`/var/lib/ttstack/ssh-initialized` records successful first provisioning. Normal
restart preserves the retained disk and does not restore deleted authorized keys,
rewrite SSH configuration, or rotate keys. Images must contain neither this
completion marker nor shared login/host private keys. Private seed scripts and
QEMU ISOs are host files with mode 0600; include retained runtime metadata in
private backups. Deleting the VM removes its seed. There is no key-update API.

## Reachable endpoint

Configure `tt-agent --ssh-public-address IPv4` with the actual client-reachable
resource-host address. If the agent shares the public network namespace, its
normal mapped port is used. An agent in a private network namespace additionally
uses `--ssh-ingress-netns /proc/1/ns/net --ssh-ingress-target AGENT_PRIVATE_IP`.
The two ingress options must be supplied together. `nsenter` and nftables must be
available; the namespace path must refer to the host network namespace.

The agent installs only the VM's SSH DNAT rule in the outer `ip tt-ssh` table,
identified by its VM UUID, and removes that rule on VM deletion. It does not expose
the full mapped-port pool or application/management ports. Operators must provide
routing, forwarding and firewall permission for that path, reserve a nonoverlapping
mapped-port range per agent sharing a public address, and test it from an external
client. Configure that range with `--port-start` and `--port-end` (inclusive;
defaults 20000 and 65535). Port allocation still checks only the agent's own
inventory and local namespace, so disjoint ranges must be assigned by the operator.
Public ingress currently supports IPv4. Caddy HTTP routes do not carry SSH.

The agent persists the ingress configuration and port range before creating any
VM. Changing or removing that configuration while VM records remain is rejected:
restore the original settings, delete the owned VMs and then reconfigure. This
keeps the location needed to remove each VM's outer rule. For database compatibility
and upgrade requirements, consult the [deployment guide](deployment.md#resource-update-schema-gate).
Remove only rules with established ownership, never flush the shared table.

VM `ssh` metadata contains `user`, boolean `sudo`, `host`, `port`, initial public
`host_key`, `ready`, Unix-seconds `checked_at`, and `initialized`. Missing public
configuration leaves `host` empty; callers must not infer an address from the
controller or management endpoint. Initial readiness requires the provisioned
host identity to be observed using ssh-keyscan; later observations check the SSH
banner and do not reconcile user key/config edits. This measures guest service
reachability from the agent, not external routing or initial-key validity forever.
The advertised host key is the initial identity, not a key-rotation registry.

Agents require ssh-keygen and ssh-keyscan to advertise `ssh_bootstrap`.
Controller schema v5 and agent schema v6 preserve SSH options, host identity,
runtime/network bindings and the disk-backup lifecycle. Disk restore invalidates
old SSH observations and exposes `ssh.observation_error` while the initial identity
is unconfirmed; see the [restore policy](disk-backup.md#ssh-observations-after-restore).
Upgrade them together following the target revision's compatibility requirements.
Existing VMs without SSH options are not rekeyed or reimaged.

See [live validation](validation/expert-ssh-2026-09-28.md) and the
[API reference](rest-api.md). Restart diagnostics and automatic recovery policy
remain separate from initial SSH access.
