# Manual testing on the corporate Mac (v1.4 Daily Driver)

Speck's Core Value can only be proven on a real corporate network (Cloudflare WARP,
TLS-inspecting proxy, split-DNS). The dev Mac has none of these, so these checklists
are run by hand on the work Mac. Each run produces a short results table that goes into
`.planning/phases/<phase>/` (or `.planning/STATE.md` for M0).

Automated gates run on the dev Mac: `scripts/conformance-smoke.sh`, `scripts/egress-smoke.sh`.

## Install a HEAD build (until OPS-04 publishes a matching initrd)

The public `initrd-1.3.0` release predates the transparent proxy, Phase 25 and
the v1.4 guest fixes, and lacks `/sbin/mke2fs`, so its first boot cannot
format the data disk. Do not boot it. Copy the HEAD build from the dev Mac and
put the initrd in place **before** the first `spk up` (assets are only
downloaded when missing).

On the dev Mac:

```bash
cargo build --release --target aarch64-apple-darwin -p speck-cli
cargo xtask sign release
# copy these two files to the work Mac (AirDrop, scp, USB):
#   target/aarch64-apple-darwin/release/spk
#   ~/.speck/initrd/initrd.cpio.gz
```

On the work Mac:

```bash
rm -f ~/bin/spk && cp spk ~/bin/spk     # rm first: overwriting a signed binary in place → SIGKILL
spk --version
mkdir -p ~/.speck/initrd
cp initrd.cpio.gz ~/.speck/initrd/initrd.cpio.gz
printf '1.3.0\n' > ~/.speck/initrd/initrd.version
spk up --wait                           # downloads kernel + rootfs only
docker context create speck --docker "host=unix://$HOME/.speck/speck.sock"
```

Keep Colima installed. Switch back at any time with `docker context use colima`.

If `spk up` cannot download assets (proxy blocks GitHub), copy `~/.speck/kernel/`,
`~/.speck/initrd/` and `~/.speck/rootfs.img` + `~/.speck/rootfs.version` from the dev Mac too.

Corporate CA handling (v1.4):

- Certificates macOS trusts through admin trust settings (WARP, Zscaler,
  MDM roots) are exported to the VM automatically. Opt out with
  `ca.keychain: false`.
- To make containers trust them too (apk/pip/npm/curl behind TLS
  inspection), set `ca.inject_into_containers: true` in `~/.speck/config.yaml`.

## M0 — Baseline (before any v1.4 code changes)

Goal: learn what the corporate network really needs. In particular, decide whether
an explicit proxy/PAC (TLS-04) is required.

1. Network facts (no Speck involved):

   ```bash
   scutil --proxy                  # HTTPEnable/HTTPSEnable/ProxyAutoConfigEnable = 1 → explicit proxy/PAC
   scutil --dns | head -40         # resolvers, split-DNS match domains
   env | grep -i _proxy            # proxy env in your shell
   security find-certificate -a -c "<corporate CA name>" -p > ~/corp-ca.pem
   ```

2. Speck health:

   ```bash
   spk doctor
   spk doctor dns github.com
   spk doctor dns <internal hostname>
   ```

3. Container egress, with WARP on:

   ```bash
   EGRESS_INTERNAL_HOST=<internal https host> \
   EGRESS_INTERNAL_TCP=<internal db host>:5432 \
     ./scripts/egress-smoke.sh
   ```

4. The same with CA injection into containers. Add to `~/.speck/config.yaml`, then
   `spk restart` and run step 3 again (the Keychain export already covers the
   VM itself; `extra_certs` is only needed if the CA is not admin-trusted):

   ```yaml
   ca:
     inject_into_containers: true
     # extra_certs:
     #   - /Users/<you>/corp-ca.pem
   ```

5. The same egress run under Colima, for comparison:

   ```bash
   EGRESS_DOCKER_HOST=unix://$HOME/.colima/default/docker.sock ./scripts/egress-smoke.sh
   ```

Record: a ✅/❌ table per check for Speck without and with CA injection, and
Colima, plus the `scutil --proxy` output. On the dev Mac (no VPN) the HEAD
build passes 20/20.

## M1 — Real workloads (after Phases 26 + 27)

| Workload | Command | Result |
|---|---|---|
| Pull from private registry | `docker pull <ECR/Artifactory image>` | |
| Build with internal deps | `docker build .` in a work repo (pulls packages through the proxy) | |
| buildx | `docker buildx build --load .` | |
| Compose stack | `docker compose up -d` of a work stack, then `curl localhost:<port>` | |
| testcontainers | one integration test suite of a work project | |
| Infra tools | `docker run --rm -v ~/.aws:/root/.aws <terraform image> plan` / `kubectl` | |
| ssh from container | `docker run --rm alpine sh -c 'apk add openssh-client && ssh -T git@github.com'` | |

## M2 — Resilience (after Phase 28)

After each event, run without touching DNS or proxy settings:

```bash
docker run --rm alpine wget -qO- https://<internal host> >/dev/null && echo OK
```

| Event | OK without reset? | Notes |
|---|---|---|
| WARP off → on | | |
| WARP on → off | | |
| Wi-Fi network switch | | |
| Sleep 10 min → wake | | also `docker run --rm alpine date` vs host `date` |
| Sleep overnight → wake | | |
| macOS reboot (Speck auto-starts?) | | |
| `spk restart` | | |

## M3 — Dogfood week (Phase 30)

Use Speck as the only runtime: `docker context use speck`, `colima stop`.
Keep an incident log:

| Date/time | Symptom | What I did to recover | Fixed by reset? |
|---|---|---|---|

Exit criterion: zero manual DNS/proxy resets over 5 working days.
Every incident becomes a Task Master task plus a regression test.
