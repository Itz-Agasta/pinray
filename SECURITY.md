# Security policy

pinray captures screen content and system audio, so a bug can expose something the user didn't mean to share. This page explains which versions get fixes and how to report a problem privately.

## Supported versions

pinray is pre-1.0, so only the latest release gets security fixes.

| Version | Supported |
| ------- | --------- |
| 0.2.x (latest) | ✅ |
| older | ❌ |

## Report a vulnerability

Don't open a public issue. Report it privately through [GitHub's private vulnerability reporting](https://github.com/Itz-Agasta/pinray/security/advisories/new), or email rupamgolui@proton.me.

Include the pinray version, your OS and version, the backend (`session.backend_info()` output), the steps to reproduce, and what an attacker could see or do.

Examples of what counts:

- Capturing content outside what the user selected or the session was configured for
- Bypassing or skipping an OS permission prompt (portal dialog, macOS Screen Recording)
- Memory safety bugs in the platform crates' `unsafe` code

## What happens next

You'll get a reply within 7 days. Once a fix is ready, it ships in a patch release and the advisory is published, crediting you unless you'd rather not be named.
