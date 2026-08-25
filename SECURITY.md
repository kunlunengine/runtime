# Security Policy

Kunlun Runtime embeds a native JavaScript engine and exposes capability-gated host APIs. Memory
safety, engine provenance, FFI correctness, capability enforcement, and process isolation are all
security-sensitive boundaries.

## Supported Versions

Kunlun Runtime is currently an early engineering preview. Until the project publishes a stable
release, security fixes are made only on the latest `main` branch.

| Version | Supported |
| --- | --- |
| Latest `main` | Yes, on a best-effort basis |
| Older commits and preview artifacts | No |
| Host OS `system-jsc` backend | Development use only |

The M0/M1 runtime is not a security sandbox for hostile code. A JSC realm and host capability checks
do not replace a process, container, or microVM boundary.

## Reporting a Vulnerability

Do not open a public issue, discussion, or pull request for a suspected vulnerability.

Email **kunlunengine@zixiaolabs.com** with the subject
`[SECURITY] kunlunengine/runtime: <short summary>`. Do not attach sensitive files to the initial
email. Instead, include your GitHub username and ask the maintainers to invite you to a private
GitHub Security Advisory. Verify that the resulting advisory URL is under
`https://github.com/kunlunengine/runtime/security/advisories/` before transferring sensitive
material through it, and coordinate a different encrypted transfer method before sending if that
channel is unsuitable.

Include as much of the following as is safe to share:

- affected commit, version, engine revision, platform, and architecture;
- vulnerability class, impact, and the security boundary crossed;
- minimal reproduction steps or proof of concept;
- relevant logs, crash reports, sanitizer output, or stack traces;
- whether the issue is already public or reported upstream;
- your preferred name and credit, or a request to remain anonymous.

Remove credentials, tokens, private data, and unrelated secrets from all material.

## What to Expect

These are response targets, not guarantees:

- acknowledgment within 3 business days;
- initial severity and scope assessment within 7 business days;
- an update at least every 14 days while remediation is active.

We will validate the report, determine affected versions, and coordinate remediation and disclosure.
Please allow a reasonable remediation window before publishing details. When appropriate, we will
prepare a GitHub Security Advisory, request a CVE, notify affected users, and credit the reporter.

## Research Guidelines

Good-faith research should minimize access to other people's data, avoid service disruption and
persistence, and stop once the vulnerability is demonstrated. Do not use a finding to access,
modify, retain, or disclose data beyond what is necessary to establish impact.

This project does not currently operate a paid bug bounty. Reports about vulnerabilities in WebKit
or another dependency may also need coordinated upstream disclosure, but please report runtime-
specific exposure to us so that we can assess the pinned engine and mitigation path.
