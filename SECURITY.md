# Security policy

MobaRust is currently a desktop preview. The latest source and latest platform
installer can differ; report the exact version, OS, architecture, and affected
workflow. Older previews are not promised backported fixes.

Do not put passwords, keys, passphrases, tokens, production targets, profile
exports, terminal transcripts, or unredacted logs in public issues. Use
[GitHub's private vulnerability reporting channel](https://github.com/OthmaneBlial/MobaRust/security/advisories/new)
to send the maintainers a confidential report.

Include a reproduction using disposable fixtures, the impact, and whether the
problem affects source or a packaged build. Do not test on third-party servers
or production infrastructure.

The [threat model](docs/security/threat-model.md),
[dependency audit record](docs/security/dependency-audit.md), and
[safe testing policy](docs/security/safe-testing.md) describe current boundaries
and limits. RDP remains experimental and excluded from normal bundles under
its separate advisory gate. A clean dependency audit is not a guarantee that
application code is vulnerability-free.
