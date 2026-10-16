<p align="center">
  <img src="public/monocode.png" alt="StereoCode" width="88" />
</p>

<h1 align="center">StereoCode</h1>

<p align="center">
  <strong>A GUI for your coding agents.</strong>
</p>

<p align="center">
  <img width="1680" height="1050" alt="Upstream MonoCode with agent sessions open side by side" src="https://github.com/user-attachments/assets/2cd4a6ec-eb1e-4b45-8627-a76442ea3874" />
</p>

Works with your subscriptions on Claude Code, Codex, Cursor, Grok Build, OpenCode, Antigravity, Pi, omp, fx, Hermes Agent, and Devin. If they’re installed and logged in, StereoCode can run them. Tabs are sessions. The composer is the input. StereoCode does not sell tokens.

## Install

Install and sign in to at least one supported agent first. See [provider setup](docs/providers.md) for instructions.

Build StereoCode from this checkout using the [build setup and packaging instructions](docs/building.md). Generated packages use the StereoCode name; the desktop executable is `stereocode` and the bundle identifier is `tech.privcloud.stereo`. See [Linux setup](docs/install.md) for dependencies and package instructions.

Existing [upstream MonoCode downloads](https://github.com/hardbeat920/monocode/releases/latest) are not StereoCode builds.

## Get started

Open a project folder, choose an agent, and send a message. Each tab is a session. Split panes to work with sessions side by side.

- Keep conversations organized by project.
- Use separate Git worktrees for parallel tasks.
- Edit files and review changes in the app.
- Start a message with `/operator` to let an agent open sessions and manage worktrees and notes.

Monos are experimental agents that work across your projects. They keep a memory and can run scheduled tasks you set up with them.

StereoCode is still early. If something breaks, [report a bug](https://github.com/hardbeat920/monocode/issues).

## Guides

- [Provider setup](docs/providers.md)
- [Linux installation](docs/install.md)
- [Agent access with /operator](docs/agent-access.md)
- [Remote sessions, experimental](docs/remote-access.md)

## Build from source

You’ll need Node.js 20+ and a stable Rust toolchain. See [build setup](docs/building.md) for platform dependencies and packaging.

```bash
npm install
npm run tauri dev
```

Small, focused contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## Contributors

StereoCode builds on MonoCode. Thanks to everyone who contributes to the upstream project!

[![MonoCode contributors](https://contrib.rocks/image?repo=hardbeat920/monocode)](https://github.com/hardbeat920/monocode/graphs/contributors)

Contributor image by [contrib.rocks](https://contrib.rocks).

## License

[MIT](LICENSE). Provider names and logos are trademarks of their owners. See [NOTICE](NOTICE).
