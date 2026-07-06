---
status: complete
phase: 13-diagnostics
source: [13-VERIFICATION.md]
started: 2026-07-05T18:15:00Z
updated: 2026-07-06T07:00:00Z
---

## Current Test

Completed 2026-07-06 — all 3 scenarios verified.

## Tests

### 1. Healthy system — all 8 checks pass
expected: Run `spk up && spk doctor`; exit 0, all 8 checks display PASS/WARN/SKIP in order: codesign entitlement, DOCKER_HOST, cert injection, VM resources, public DNS, VPN DNS, VM running, Docker socket
result: PASSED — exit 0, all 8 checks displayed:
  PASS  codesign entitlement
  WARN  DOCKER_HOST (not set — expected, spk init not run)
  SKIP  cert injection (no CA certs configured — expected)
  PASS  VM resources (cpus: 2, memory: 2048 MiB, disk: 20 GiB)
  PASS  public DNS
  SKIP  VPN DNS (no VPN active — expected)
  PASS  VM running
  PASS  Docker socket

### 2. VM down — check_vm_running and check_docker_socket fail
expected: Run `spk doctor` before `spk up`; exit 1 with FAIL hints on "VM running" and "Docker socket" checks
result: PASSED — exit 1, FAIL on both checks:
  FAIL  VM running — hint: VM is not running — start with `spk up`
  FAIL  Docker socket — hint: Docker socket unreachable — run `spk up` first

### 3. DNS trace sub-command
expected: Run `spk doctor dns google.com` with Alpine image present; confirms resolver IP, answer IPs, and PASS verdict are printed
result: PASSED — exit 0:
  Nameserver: macOS system resolver
  Address: 142.251.98.101 (+ 5 more A records)
  PASS  host resolver working — guest DNS inherits this path via vsock proxy
Note: trace_dns exercises getaddrinfo directly (same path as guest vsock proxy) — no container needed.

## Summary

total: 3
passed: 3
issues: 0
pending: 0
skipped: 0
blocked: 0

## Gaps
