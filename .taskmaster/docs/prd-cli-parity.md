# Speck — CLI parity with Colima (profiles, ssh, ssh-config, template)

## Overview

Speck 1.4.5 covers the everyday Colima commands (`start/stop/restart/delete/status/list/version/prune/completion`)
plus `recreate`. Four Colima features are still missing and are in scope:

1. Profiles (`colima -p <name>`): several independent instances.
2. `spk ssh`: a shell (or one command) inside the VM.
3. `spk ssh-config`: an OpenSSH config block, so `ssh speck` and VS Code Remote-SSH work.
4. `spk template` / `spk start --edit`: edit the instance config in `$EDITOR`.

Out of scope (decided 2026-10-17): `nerdctl`, `kubernetes`, `update`.
Speck uses dockerd, K3s is a separate milestone, and upgrades go through `brew upgrade`.

Locked constraints that apply (CLAUDE.md, D-21):
- The Docker API stays a byte proxy. Nothing in this PRD touches the API middleware.
- Any new vsock consumer must drain eagerly and use 4 MB socket buffers.
- Gates: `scripts/conformance-smoke.sh` and `scripts/egress-smoke.sh` must stay green.
- Commits carry no AI attribution.

Current state (verified in code on 2026-10-17):
- `resolve_speck_home()` in `crates/speck-cli/src/main.rs` returns `$SPECK_HOME` or `~/.speck`.
- A custom `SPECK_HOME` runs in the foreground only. It gets no launchd agent and no docker context
  (`docker_context::manages_context()` is true only when `SPECK_HOME` is unset).
- The launchd label `io.speck.vm` is hard-coded in `commands/up.rs` and `commands/down.rs`.
- The docker context name `speck` is hard-coded in `docker_context.rs`.
- `spk list` (`commands/status.rs::run_list`) prints one hard-coded `default` row.
- The guest mounts `$SPECK_HOME` over VirtioFS at `/var/run` (`vminitd.rs::mount_speck_home`).
- The guest `tcp_forwarder` (vsock 9006) connects to any `ip:port` target line, including guest loopback.
- The guest rootfs is Alpine and is built by `.github/workflows/build-rootfs.yml`. It has no SSH server.

---

## 1. Profiles

### Problem
Only one fully managed instance can exist. A second instance needs `SPECK_HOME=...` and runs in the
foreground only, without auto-start or a docker context. Colima users expect `colima start -p work`.

### Requirements
- Global flag `-p, --profile <name>` on every command, plus the env var `SPECK_PROFILE`. The flag wins.
- Name rules: `[a-z0-9][a-z0-9-]{0,31}`. `default` means the default profile.
- Home directory mapping:
  - `default` → `~/.speck` (unchanged, so existing installs keep their data).
  - `<name>` → `~/.speck/profiles/<name>`.
  - `SPECK_HOME` keeps working as a raw override for tests. If both `SPECK_HOME` and a profile are set,
    exit with an error.
- Per-profile names, derived in one place (a `Profile` struct in speck-cli):
  - launchd label: `io.speck.vm` for default, `io.speck.vm.<name>` otherwise; the plist name follows the label.
  - docker context: `speck` for default, `speck-<name>` otherwise.
  - socket: `<home>/speck.sock`.
- The launchd plist passes `--profile <name>` to the daemon, so a restart at login boots the right profile.
- Profiles get the full lifecycle: `start`, `stop`, `restart`, `delete`, `recreate`, `status`, `logs`,
  `doctor`, `env`, `init --set-docker-host` and `prune` all act on the selected profile.
- `spk start -p <name>` selects that profile's docker context, like Colima. `spk stop -p <name>` switches
  back to `default` only when that profile's context is the active one.
- `spk delete -p <name>` also removes the profile directory, the plist and the docker context.
- `spk list` shows one row per profile (default plus every directory in `~/.speck/profiles/`), with the
  same columns and `-j` JSON as today.
- Downloaded guest assets (kernel, initrd, rootfs image) are shared between profiles through a common
  cache directory, so a new profile does not download ~200 MB again. Data disk, config, CA dir and logs
  stay per profile.
- Two running profiles that publish the same host port conflict. This is reported as a normal
  "port already in use" error and documented. It is not prevented.
- Current profile (decided 2026-10-18), like `kubectl config use-context`:
  - `spk profile use <name>` stores the name in `~/.speck/current-profile`.
  - Precedence: `-p` > `SPECK_PROFILE` > current-profile file > `default`.
  - `spk profile current` prints the resolved name (for scripts and shell prompts).
  - Every lifecycle command names the profile it acts on (`Stopping profile "work"…`), because the
    stored current profile makes it easy to forget which VM a bare `spk stop` hits.
  - The command is `spk profile`, not `spk context`: Speck already creates docker contexts
    (`speck`, `speck-<name>`), and two kinds of "context" would confuse users and docs.
- `spk profile` subcommands:
  - `ls`: same as `spk list`, with a `*` marking the current profile.
  - `use <name>`, `current`.
  - `create <name> [--cpus N] [--memory SIZE] [--disk SIZE] [--no-autostart]`: creates the directory
    and its `config.yaml` without starting the VM.
  - `rm <name>`: same as `spk delete -p <name>`.
- CI / ephemeral profiles:
  - `--no-autostart` (on `profile create` and `start`, stored in the profile's config as
    `autostart: false`): the daemon runs detached in the background without a launchd plist, so the
    profile does not come back after a reboot. `spk stop`/`delete` work the same way for it.
  - `spk start --wait` works for every profile: it returns once the Docker API answers, or exits
    non-zero after a timeout.
  - Typical job: `SPECK_PROFILE=ci-$JOB spk start --no-autostart --wait`, run tests, `spk delete --force`.
- The Unix socket path (`<home>/speck.sock`) must fit macOS's 104-byte `sun_path` limit; a too long
  path fails early with a clear error.

### Acceptance Criteria
- `spk start -p work` boots a second VM next to the default one. `docker --context speck-work ps` and
  `docker --context speck ps` both work at the same time.
- `spk list` shows both profiles with their state, CPUs, memory and disk.
- `spk profile use work`, then a bare `spk status` reports `work`; `SPECK_PROFILE=default spk status`
  and `-p default` override it.
- A `--no-autostart` profile starts, answers `docker ps`, has no plist in `~/Library/LaunchAgents`, and
  `spk delete --force -p <name>` removes it completely.
- After a reboot (or `launchctl kickstart`), both running profiles come back.
- `spk delete -p work` leaves the default profile untouched, and `spk list` no longer shows `work`.
- Unit tests cover name validation, home/label/context mapping and the `SPECK_HOME` + profile conflict.
- The existing default install upgrades without moving files or re-downloading assets.

---

## 2. SSH server in the guest

### Problem
There is no way to get a shell inside the VM to debug dockerd, containerd, the netstack or mounts.
`ssh-config` (section 4) also needs a real SSH server.

### Requirements
- Add `dropbear` (small, Alpine package) to the rootfs in `.github/workflows/build-rootfs.yml` and
  release a new rootfs version. Bump the rootfs version the CLI downloads.
- vminitd starts dropbear after switch_root, under the existing restart-with-backoff supervisor pattern.
  - It listens on guest `127.0.0.1:22` only. No port is reachable from the guest network or containers.
  - It logs in as `root` with the login shell `/bin/sh`.
  - It accepts public-key auth only: no passwords, no root password.
  - The host key is generated on first boot and kept on the data disk, so it is stable across restarts.
  - `authorized_keys` is read from the VirtioFS-mounted Speck home (`/var/run/ssh/authorized_keys` in the
    guest, which is `<home>/ssh/authorized_keys` on the Mac).
- If dropbear is missing (old rootfs), vminitd logs one line and continues. Nothing else is affected.

### Acceptance Criteria
- After boot, the guest console log shows dropbear started on 127.0.0.1:22.
- `docker run --rm alpine nc -z 172.17.0.1 22` and `... nc -z 172.16.0.2 22` both fail.
- Conformance and egress gates stay green.

---

## 3. Host side: keys, ssh socket and `spk ssh`

### Requirements
- On `spk start`, if `<home>/ssh/id_ed25519` does not exist, generate an ed25519 key pair with
  `ssh-keygen -t ed25519 -N "" -C speck-<profile>`. Write the public key to `<home>/ssh/authorized_keys`.
  Files are 0600 and the directory is 0700.
- The daemon exposes `<home>/ssh.sock`, a Unix socket. Each connection is bridged to vsock 9006 with the
  target line `127.0.0.1:22\n`, reusing the published-port forwarding path. It follows the eager-drain
  rule and the 4 MB buffer rule.
- New command `spk ssh-proxy`: hidden from `--help`. It connects to `<home>/ssh.sock` and copies
  stdin/stdout both ways. This is the OpenSSH `ProxyCommand`.
- New command `spk ssh [-- <command>...]`: execs the system `ssh` with:
  - `-i <home>/ssh/id_ed25519`, `-o IdentitiesOnly=yes`;
  - `-o ProxyCommand="<spk binary> ssh-proxy -p <profile>"`;
  - `-o UserKnownHostsFile=<home>/ssh/known_hosts` and `-o StrictHostKeyChecking=accept-new`;
  - the user `root@speck`.
  With no arguments it opens an interactive shell. With `-- cmd`, it runs the command and passes its
  exit code through.
- When the VM is not running, `spk ssh` exits non-zero with "Speck (<profile>) is not running; run `spk start`".

### Acceptance Criteria
- `spk ssh -- uname -a` prints the guest kernel and exits 0. `spk ssh -- false` exits 1.
- `spk ssh` gives an interactive shell where `ps` shows dockerd and containerd.
- `spk ssh -p work -- hostname` reaches the work profile VM.
- Unit test: the generated ssh argv contains the expected options for a given profile.

---

## 4. `spk ssh-config`

### Requirements
- New command `spk ssh-config` prints an OpenSSH config block for the selected profile:
  ```
  Host speck            # speck-<name> for other profiles
    User root
    HostName speck
    IdentityFile <home>/ssh/id_ed25519
    IdentitiesOnly yes
    UserKnownHostsFile <home>/ssh/known_hosts
    StrictHostKeyChecking accept-new
    ProxyCommand <absolute spk path> ssh-proxy --profile <name>
  ```
- The spk path should survive brew upgrades. Use `/opt/homebrew/bin/spk` (the brew symlink) when the
  running binary lives under the Homebrew Cellar. Otherwise use `current_exe()`.
- Usage is documented as `spk ssh-config >> ~/.ssh/config`, like Colima. The command never edits
  `~/.ssh/config` itself.

### Acceptance Criteria
- After `spk ssh-config >> ~/.ssh/config`, `ssh speck uname -a` works.
- VS Code Remote-SSH connects to host `speck` (manual check, `docs/manual-testing.md`).
- Unit test: the output for default and for a named profile matches the expected text.

---

## 5. `spk template` and `spk start --edit`

### Requirements
- New command `spk template` opens `<home>/config.yaml` in `$VISUAL`, then `$EDITOR`, then `vi`.
- If the file does not exist, first write a commented template with every supported key and its default.
  Derive the defaults from the same code as the config loader, so they cannot drift.
- After the editor exits, validate the file with the existing loader (unknown keys, value ranges).
  On error, print the error and offer to re-open the editor or keep the file as it is. Never silently
  discard the user's edits.
- `spk start --edit` runs the same flow, then starts the profile.
- Edits to CPUs, memory or disk only apply on the next start. Print one line saying so when the VM is
  running. The disk only grows; this follows the existing rule.

### Acceptance Criteria
- `EDITOR=true spk template` on a fresh profile creates a valid commented `config.yaml`.
- An invalid edit (for example `vm: {cpus: abc}`) is reported with the loader's message, and the file is kept.
- `spk start --edit -p work` edits the work profile config and starts it.
- Unit test: the generated template parses back into the default config.

---

## 5a. Machine-readable output (`-o`)

### Problem
Only `spk status --json` and `spk list --json` print JSON, each with its own flag. Scripts and CI need
one consistent way to read Speck's state.

### Requirements
- Global flag `-o, --output <plain|json|yaml>`, default `plain`.
- Supported by the reporting commands: `status`, `list`/`profile ls`, `profile current`, `config`,
  `version`, `doctor`. Other commands reject `-o json|yaml` with a clear error.
- The existing `--json`/`-j` flags stay as aliases for `-o json`.
- In json/yaml mode stdout holds only the document; warnings and progress go to stderr.
- Exit codes do not depend on the format (`config` exits 1 on an invalid config, `doctor` non-zero on
  a failed check).
- The document shape is the same in json and yaml (one serde struct per command).

### Acceptance Criteria
- `spk status -o json | jq .` and `spk config -o yaml | yq .` parse.
- `spk config -o json` on an invalid config prints a document with the error and exits 1.
- Unit tests cover the serialized shape of each command's document.

---

## 6. Gates, docs and release

### Requirements
- New smoke script `scripts/cli-smoke.sh` runs against a live install:
  - start a throwaway profile;
  - check `list`, `ssh -- true`, `ssh-config` output and `ssh speck-<name> true` with a temp ssh config;
  - run `template` with `EDITOR=true`;
  - delete the profile.
- README: a profiles section, `spk ssh`, `spk ssh-config`, `spk template`, and an updated Colima parity list.
- `docs/manual-testing.md`: VS Code Remote-SSH check and the two-profile reboot check.
- Ship in one release: a new rootfs tag, a new initrd tag if vminitd changed, then the CLI release
  and the tap formula update (the existing release flow).

### Acceptance Criteria
- `cli-smoke.sh`, `conformance-smoke.sh` and `egress-smoke.sh` are all green on the brew-installed build.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --check` are clean.

---

## Implementation order

Profiles (1) come first, because ssh, ssh-config and template all depend on per-profile paths and names.
After that: 2 → 3 → 4, with 5 and 5a independent. Section 6 comes last.

## Open questions (defaults chosen; change before implementation if needed)
- SSH user: `root` (simple, matches the guest). Colima uses the Mac user name with sudo.
- Profile home: `~/.speck/profiles/<name>`. Colima uses `~/.colima/<name>`. Ours keeps the default
  profile at `~/.speck`, so existing data does not move.
