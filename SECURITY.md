# Security policy

## Reporting a vulnerability

**Please do not open a public issue, discussion or PR for a security problem.**

Report it privately through GitHub Security Advisories:
<https://github.com/cutweave/cutweave/security/advisories/new>

Include what you found, the affected version or commit, reproduction steps or a proof of concept, and the impact you expect. Do not include real API keys or other people's data.

You can expect an acknowledgement within 5 working days. We will keep you updated on the fix and credit you in the advisory unless you prefer otherwise.

## Disclosure

We follow a **90-day coordinated disclosure** window: we aim to ship a fix and publish the advisory within 90 days of your report. We may ask for more time for complex issues, and will publish earlier if the issue is already being exploited.

## Supported versions

Cutweave is pre-1.0.

| Version | Supported |
|---|---|
| Latest release (or `main`) | Yes |
| Anything older | No |

## Scope

In scope:

- **Local API** (JSON-RPC / HTTP on loopback): authentication, token handling, scopes, origin checks, anything reachable by another local user, process or web page.
- **MCP server**: tool permissions and annotations, spending-authority bypass, prompt injection through untrusted text in tool results that leads to unauthorized actions.
- **Plugin sandbox**: escapes from the WASM, QuickJS composition, WGSL or iframe sandboxes; permission bypass (filesystem, network, keys); malicious plugin or data-pack handling.
- **AI keys and gateway**: any path by which an API key leaves the engine (renderer, plugins, logs, MCP results, crash reports, project files) or is used without the required authorization.
- **File handling**: memory-safety or code-execution bugs when opening projects, media or imported formats.
- **Update and packaging**: signature or integrity bypass.

Out of scope: vulnerabilities in third-party services or models themselves, issues requiring a user to already run attacker-controlled code with full user privileges, denial of service from deliberately huge local media, and social engineering.

## Maintainers

Advisories are handled by the maintainers (see [GOVERNANCE.md](GOVERNANCE.md)).
