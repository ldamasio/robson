# Windows/Cygwin Path Fix

## Issue

When using Cygwin/Git Bash with Podman on Windows:

```bash
-v "$(pwd):/work"
# Returns: /cygdrive/c/app/notes/robson/infra/ansible
# Podman ERROR: no such file or directory
```

## Solution

Use Windows-style path explicitly:

```bash
-v "C:/app/notes/robson/infra/ansible:/work"
```

## All Commands Fixed

Replace `$(pwd)` with full Windows path in ALL Podman commands:

```bash
# WRONG (Cygwin)
podman run -v "$(pwd):/work" ...

# CORRECT (Windows)
podman run -v "C:/app/notes/robson/infra/ansible:/work" ...
```

## Why

- Cygwin translates paths to `/cygdrive/c/...`
- Podman runs outside Cygwin (Windows/WSL)
- Podman doesn't understand `/cygdrive` paths
- Windows paths `C:/...` work for both

## Quick Reference

All Ansible commands in `docs/plan/infra/` have been updated to use Windows paths.
