# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities through GitHub's **Report a
vulnerability** flow in the repository Security tab. Do not include API keys,
private audio, or other personal data in a public issue. A private report lets
maintainers investigate before a fix is published.

The project does not currently promise a response-time SLA. Please include the
affected version, operating system, reproduction steps, and impact where safe.

## Scope notes

OpenMix imports local MP3 and WAV files, stores metadata and paths in the user
config directory, and sends the selected tracks' metadata to OpenRouter when
the user chooses **Mix now** with a key configured. OpenRouter keys are stored in the
system credential store where available. OpenMix has no add-on or plugin API.

