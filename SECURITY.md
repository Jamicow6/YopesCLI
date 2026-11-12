# Security policy

## Supported versions

Only the latest release receives security fixes. YopesCLI is pre-1.0, so there
are no long-term support branches.

## Reporting a vulnerability

Do not open a public issue for a security problem. Report it privately through
GitHub's [private vulnerability reporting][advisory] on this repository, or by
email to the maintainer listed on the GitHub profile.

Include the version (`yopes --version`), your platform, and the smallest
reproduction you can manage. You can expect an acknowledgement within a few
days and a fix or a written explanation in the next release.

[advisory]: https://github.com/Jamicow6/YopesCLI/security/advisories/new

## What is in scope

- The credential store: API keys are sealed with argon2 and
  chacha20poly1305 in `src/credentials.rs`.
- The tool surface: anything that lets a model read, write, or execute beyond
  what the user approved, including plan-mode escapes.
- The self-updater: `yopes update` verifies the release checksum before
  replacing the binary.
- The HTTP client: request construction, redirects, and response parsing.

## What is not in scope

- A model doing something harmful that the user approved. Approval prompts and
  tool modes are the control; review what you allow.
- Anything that requires an already-compromised machine or an attacker who can
  write to your config directory.
- Vulnerabilities in upstream dependencies that have no exploitable path
  through YopesCLI. Report those upstream.
