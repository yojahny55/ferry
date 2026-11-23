# Security Policy

## Supported versions

Security fixes go into the latest release.

## Reporting a vulnerability

Please **don't** open a public issue. Report it privately through GitHub:
[Report a vulnerability](https://github.com/yojahny55/ferry/security/advisories/new).

Include what you found, how to reproduce it, and what an attacker could do with it. You'll get
an acknowledgment within a week, and a fix or a plan as soon as the problem is understood.

## Scope

Ferry is a file manager, so the interesting cases include:

- File names or contents that make Ferry run something it shouldn't (Ferry passes files to
  `xdg-open`, `gio`, `cp`/`mv`, and the system's thumbnailers).
- Operations that overwrite or delete files other than the ones the user acted on.
- Writes to the bookmarks file, settings, or thumbnail cache that corrupt them or escape their location.
