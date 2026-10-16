# Jira inbox

MonoCode reads Jira Cloud and Jira Data Center. Choose the product in
**Settings → Inbox → Jira**. The connection is validated before credentials are
saved. **Disconnect** removes the saved credentials and clears cached Jira
content. Credentials are stored in the app's local data directory; on Unix the
file is created with owner-only permissions.

## Jira Cloud

Enter your Jira site, Atlassian account email, and an API token **without
scopes**, then select **Connect**. Create a token from
[your Atlassian account](https://id.atlassian.com/manage-profile/security/api-tokens).
Scoped tokens are not supported.

A site can be `yourteam.atlassian.net` or a full `https://` URL.

## Jira Data Center

Enter the site root and a personal access token, then select **Connect**.
Create the token from your Jira profile (**Profile → Personal Access Tokens**).
The token is sent as a bearer token. Username and password are not used.

Include a context path when Jira is not served from the host root, for example
`https://jira.example.com/jira`. Pasting a browse URL is fine; MonoCode keeps
the context path and drops the page path. The site must use HTTPS.

Data Center uses REST API v2. Descriptions and comments are Jira wiki markup;
MonoCode converts them to Markdown for the inbox. Comments you post are sent as
wiki text.

## Inbox

Select the Jira source in Inbox to browse issues, read descriptions and comments,
post comments, ask about a ticket, or start work in a local project. Starting work
and asking about a ticket include its description and Jira identifier.

Project selections are shared between Settings and the Inbox filter menu.
Only checked projects are requested from Jira. A new connection starts with
none selected, independently of project discovery. None, an unconfigured
selection, an empty catalog, or failed discovery never triggers a site-wide
issue search. All selects only the projects in the current catalog; newly
discovered projects stay unchecked. Unchecked projects are excluded from
fetching, background notifications, and automation, including responses that
arrive after the selection changes.

Selections are bound to the deployment, server URL (including context path),
and verified account. Changing any of these requires a new selection. Rotating
a token for the same verified account retains its selection. Older credentials
without a verified account ID require confirmation after reconnecting.

Earlier versions stored excluded IDs without their original project catalog.
Those settings now require choosing projects again, with an explanation in
Settings. They start with none selected because reconstructing the complement
from today's catalog could silently include new projects.

Project discovery falls back from `/project/search` to `/project` only on the
first Data Center page returning HTTP 404, 405, or 501. Cloud errors,
authentication failures, rate limits, transport/JSON errors, and later-page
errors are returned unchanged.
The inbox loads up to 40 open issues or 100 issues when including closed history,
ordered by last update within the selected projects. Long comment threads show
the latest 50 comments, with a link to Jira.

Automations offer **Jira → Issue appeared** after connecting. They run in the
automation's selected local project when a Jira issue first appears in the polled
inbox, after its initial snapshot. The project-key filter uses values such as
`ENG`. This is polling, not a webhook for every issue created on the site.
