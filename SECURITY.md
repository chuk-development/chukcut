# Security policy

## Supported versions

The project is pre-1.0. Only the `master` branch receives fixes.

## Reporting a vulnerability

**Do not open a public issue.**

Use GitHub's private reporting — *Security* → *Report a vulnerability* on this
repository — or email **security@chuk.dev**.

Please include what an attacker gains, the steps to reproduce, and the build and
platform you saw it on. A proof-of-concept project file or media sample helps a
great deal.

Expect an acknowledgement within a few days. Since this is a small project,
please allow a reasonable window to ship a fix before disclosing publicly.

## What is in scope

chukcut parses untrusted input by design, which is where the risk lives:

- **Media files.** Anything that turns a crafted video into code execution,
  memory corruption in our own decode paths, or a crash we handle badly.
  Vulnerabilities inside FFmpeg itself belong upstream; report those to the
  FFmpeg project, and tell us so we can bump the version.
- **Project files** (`.chukcut`). Deserialising a document must not write
  outside the project directory, execute anything, or load arbitrary code.
- **Effect packages.** These are loaded from a URL the user supplies and contain
  shaders and metadata. Path traversal out of the package, shader translation
  that escapes its sandbox, or anything reaching the file system is in scope.
- **The IPC boundary.** Every capability is a registered Tauri command. A
  command that lets the webview read or write paths the user did not choose is
  a bug, and an important one.

## What is not in scope

- Requiring the user to run a build they compiled themselves from modified source.
- Denial of service by a deliberately malformed file that only crashes the app
  without corrupting anything — file a normal bug for that.
- Codec patent questions. Those are in [`NOTICE.md`](NOTICE.md), not here.
