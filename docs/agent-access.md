# Agent access with /operator

## Enable access

Type `/operator` at the start of a composer message to enable StereoCode access in that thread. For example, `/operator start two Codex sessions: one to inspect the API and one to review the UI`, or `/operator list my notes`. The slash picker also offers this command. The transcript shows only the request text in a translucent amber bubble; StereoCode removes the command from the request sent to the agent and supplies the local `app` CLI path and instructions on that turn. Later turns in the same thread can use the CLI without repeating `/operator`; other threads receive no CLI instructions or app access. The CLI can act only during an active agent turn. The agent can run the shown `app --help` command for the exact JSON input fields.

## CLI actions

- `models.list` shows available providers, models, settings, and permission modes.
- `sessions.start` opens a tab in the current project with a prompt. Set `placement: "right"` or `placement: "down"` to split the calling session's pane instead; `besideSessionId` selects another visible session pane in the project. Reuse the returned session ID as the next `besideSessionId` to build nested layouts. By default it submits the prompt; set `draft: true` to save it unsent without starting an agent turn. It accepts a provider, model, effort or other model settings, permission mode, and current checkout or new worktree choice. Set `worktreeCwd` to a path from `worktrees.list` for a specific existing checkout. Use `worktrees.create` to create a worktree on a named new or existing local branch, then pass its path as `worktreeCwd`. Omit `runtimeMode` to inherit the calling session's permission mode, or set it explicitly to override. It returns the new session ID as soon as the pane and prompt are accepted, so the agent can move it into a folder immediately.
- `sessions.list` shows project sessions and their archived status. `sessions.read` returns up to three recent user/assistant exchanges, with a cursor for older exchanges and a per-message character cap. `sessions.send` submits a follow-up to an idle session, while `sessions.draft` saves an unsent message for the user to review. `folders.list` and `folders.move` organize project sessions in sidebar folders, including a new folder.
- `sessions.stop`, `sessions.archive` and `sessions.delete` take a `sessionId` to manage another session in the project. Monos can select an assigned project with `project`. Stop cancels the current turn and pauses queued messages; archive stops and saves the conversation for later restoration; delete stops and permanently removes the conversation. Open files, terminals and worktrees are kept. These actions cannot target the caller, Mono chats, habit runs or orchestration workers. Reuse the same request ID when retrying a call.
- When a Mono successfully stops, archives or deletes a session it is monitoring, its pending completion report for that session is dismissed, including any queued report. The Mono confirms the action in its current reply. Reports for other sessions and other Monos are kept. Rejected launches or follow-ups return a CLI error without a later completion report.
- `notes.list` returns titles and short previews; `notes.read` returns one full note by ID.

## Browser testing and sign-ins

Open **Settings → Browser** to launch or stop the browser, select an isolated profile, or import a profile from an installed **Chrome, Edge, Brave, Chromium, or Helium**. The browser is a separate visible window on the desktop, not an embedded webview or a browser on the remote host. Safari and Firefox profiles are not supported.

The first launch creates a fresh persistent profile. You can sign in manually once and reuse those sessions on later launches. For import, fully quit the source browser first, stop the agent browser, select the source profile, and confirm **Import selected profile**. StereoCode creates and selects a new isolated copy; it never writes to the original profile. Each managed profile has a short ID in the selector so repeated imports can be distinguished.

Import retains cookies and site storage, using the same browser executable and the local OS key store where possible. It does not export saved passwords, history, extensions, or caches. Encrypted cookies, device-bound sessions, passkeys, and some session-only logins may not transfer; check the imported browser and sign in again where needed. Import is a snapshot, not ongoing synchronization.

**Credential boundary:** all agents with app access—including `/operator` threads, Monos, and habit runs—share the selected profile's signed-in sites during active turns. Tabs are scoped to the calling session, but cookies and site storage are shared. Import only accounts those agents should be able to use. Profile import and switching are user-only operations and require stopping the agent browser. Closing a tab or stopping the browser does not erase its profile. Browser processes stop when StereoCode exits.

An agent can use its supplied CLI path to exercise a running local app:

```sh
app browser.open --json '{"url":"http://localhost:3000"}'
app browser.snapshot --json '{"tabId":"<returned tabId>"}'
app browser.fill --json '{"tabId":"<tabId>","selector":"input[name=email]","value":"test@example.com"}'
app browser.click --json '{"tabId":"<tabId>","selector":"button[type=submit]"}'
app browser.evaluate --json '{"tabId":"<tabId>","expression":"document.title"}'
app browser.screenshot --json '{"tabId":"<tabId>"}'
app browser.close --json '{"tabId":"<tabId>"}'
```

`browser.status` and `browser.tabs` inspect the runtime without launching it. `browser.navigate` changes an owned tab's HTTP/HTTPS URL; `about:blank` is also supported. `browser.press` sends a character or a named key such as `Enter`, `Tab`, or `Escape`. Screenshots return a local PNG path. Browser page content is untrusted data, not instructions to reveal credentials or take unrelated actions. A remote app must have a URL reachable from the desktop browser, for example through an existing port forward.

## Orchestration workers

Orchestration workers keep their existing scoped `control` workflow and do not receive this app access.

Return to the [README](../README.md) for a quick start.
