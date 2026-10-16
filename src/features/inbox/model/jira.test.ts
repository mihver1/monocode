// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  clearJiraProjectSelection,
  clearJiraCache,
  disconnectJira,
  jiraConnected,
  jiraIssueDetails,
  jiraIssueThread,
  jiraIssueComment,
  jiraProjectIdsForFetch,
  jiraProjectSelectionConfigured,
  jiraProjectSelectionNeedsReview,
  listJiraIssues,
  listJiraProjects,
  loadSelectedJiraProjectIds,
  peekJiraIssueDetails,
  peekJiraIssueThread,
  saveJiraConfig,
  saveSelectedJiraProjectIds,
  type JiraIssue,
  type JiraStatus,
} from "./jira";
import {
  clearInboxCache,
  listInboxItems,
  inboxItemStatus,
  inboxStartDraft,
  peekInboxList,
} from "./githubTasks";
import { inboxTrackerDescription } from "./inboxContext";
import {
  clearPendingInboxSelfActivity,
  consumeInboxSelfActivity,
} from "./inboxSelfActivity";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const issue: JiraIssue = {
  provider: "jira",
  kind: "jira",
  id: "10042",
  identifier: "ENG-42",
  number: 42,
  title: "Fix auth",
  url: "https://acme.atlassian.net/browse/ENG-42",
  state: "In Progress",
  stateType: "indeterminate",
  updatedAt: "2026-09-23T10:00:00Z",
  labels: [],
  assignees: [],
  draft: false,
  repo: "ENG",
  teamId: "10000",
  teamName: "Engineering",
  projectPath: "",
};
const projects = [
  { id: "10000", key: "ENG", name: "Engineering" },
  { id: "10001", key: "OPS", name: "Operations" },
];
const query = { assignedToMe: true, state: "open", search: "" } as const;
const connectedStatus: JiraStatus = {
  connected: true,
  connectionId: "cloud-acme-ada",
  site: "https://acme.atlassian.net",
  email: "ada@example.com",
  deployment: "cloud",
};
let status: JiraStatus;

beforeEach(async () => {
  clearInboxCache();
  clearPendingInboxSelfActivity();
  localStorage.clear();
  vi.mocked(invoke).mockReset();
  status = { ...connectedStatus };
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "jira_status") return status;
    if (command.endsWith("_status")) return { connected: false };
    if (command === "jira_list_projects") return projects;
    if (command === "jira_list_issues") return [issue];
    if (command === "jira_issue_details")
      return { body: "Reproduction steps", author: "Ada" };
    if (command === "jira_issue_thread")
      return { comments: [], truncated: false };
    if (command === "jira_issue_comment")
      return `${issue.url}?focusedCommentId=7`;
    if (command === "jira_set_config")
      return {
        connected: false,
        connectionId: "",
        site: "",
        email: "",
        deployment: "cloud",
      };
    throw new Error(`Unexpected command: ${command}`);
  });
  await jiraConnected();
  vi.mocked(invoke).mockClear();
});

describe("Jira inbox", () => {
  it("does not search issues before a project selection is configured", async () => {
    expect((await listInboxItems([], query)).items).toEqual([]);
    expect(invoke).not.toHaveBeenCalledWith(
      "jira_list_issues",
      expect.anything(),
    );
  });

  it("does not search issues when discovery returns an empty catalog", async () => {
    saveSelectedJiraProjectIds(["10000"]);
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) =>
      command === "jira_list_projects" ? [] : original(command, args),
    );
    expect((await listInboxItems([], query)).items).toEqual([]);
    expect(invoke).not.toHaveBeenCalledWith(
      "jira_list_issues",
      expect.anything(),
    );
  });

  it.each([
    {
      connectionId: connectedStatus.connectionId,
      configured: false,
      selectedIds: ["10000"],
    },
    { connectionId: "other-account", configured: true, selectedIds: ["10000"] },
    {
      connectionId: connectedStatus.connectionId,
      configured: true,
      selectedIds: ["deleted", "x) OR (1"],
    },
  ])(
    "does not search with unconfigured, foreign, or invalid persisted IDs: %j",
    async (selection) => {
      localStorage.setItem(
        "monocode.jiraProjectSelection.v2",
        JSON.stringify(selection),
      );
      expect((await listInboxItems([], query)).items).toEqual([]);
      expect(invoke).not.toHaveBeenCalledWith(
        "jira_list_issues",
        expect.anything(),
      );
    },
  );

  it("does not search for selected IDs missing from the current catalog", async () => {
    saveSelectedJiraProjectIds(["99999"]);
    expect((await listInboxItems([], query)).items).toEqual([]);
    expect(invoke).not.toHaveBeenCalledWith(
      "jira_list_issues",
      expect.anything(),
    );
  });

  it("loads explicitly selected projects without a local repository", async () => {
    saveSelectedJiraProjectIds(["10000", "10001"]);
    const result = await listInboxItems([], query);
    expect(result).toEqual({ items: [issue], errors: {} });
    expect(invoke).toHaveBeenCalledWith("jira_list_issues", {
      connectionId: connectedStatus.connectionId,
      assignedToMe: true,
      state: "open",
      projectIds: ["10000", "10001"],
      limit: undefined,
    });
    expect(inboxItemStatus(issue)).toBe("Open");
    expect(inboxItemStatus({ ...issue, stateType: "done" })).toBe("Closed");
  });

  it("filters projects before fetching and skips None", async () => {
    saveSelectedJiraProjectIds(["10000"]);
    await listInboxItems([], { ...query, jiraSelectedProjectIds: ["10000"] });
    expect(invoke).toHaveBeenCalledWith(
      "jira_list_issues",
      expect.objectContaining({ projectIds: ["10000"] }),
    );
    saveSelectedJiraProjectIds([]);
    vi.mocked(invoke).mockClear();
    expect(
      (
        await listInboxItems([], {
          ...query,
          jiraSelectedProjectIds: [],
        })
      ).items,
    ).toEqual([]);
    expect(invoke).not.toHaveBeenCalledWith(
      "jira_list_issues",
      expect.anything(),
    );
    expect(jiraProjectIdsForFetch(projects, ["deleted"])).toEqual([]);
  });

  it.each([
    { selected: [] },
    { selected: ["10000"] },
    { selected: ["10000", "10001"] },
  ])(
    "does not automatically enable a new project after selection $selected",
    async ({ selected }) => {
      saveSelectedJiraProjectIds(selected);
      const original = vi.mocked(invoke).getMockImplementation()!;
      vi.mocked(invoke).mockImplementation(async (command, args) => {
        if (command === "jira_list_projects")
          return [...projects, { id: "10002", key: "NEW", name: "New" }];
        if (command === "jira_list_issues")
          return [issue, { ...issue, id: "new", teamId: "10002" }];
        return original(command, args);
      });
      const result = await listInboxItems([], query);
      expect(loadSelectedJiraProjectIds()).toEqual(selected);
      expect(result.items.some((item) => item.teamId === "10002")).toBe(false);
      const searches = vi
        .mocked(invoke)
        .mock.calls.filter(([command]) => command === "jira_list_issues");
      expect(searches).toHaveLength(selected.length ? 1 : 0);
      if (selected.length)
        expect(searches[0][1]).toMatchObject({ projectIds: selected });
    },
  );

  it.each(["[]", '["10000"]', '["10000","10001"]', "invalid"])(
    "requires confirmation for legacy excluded IDs %s without granting projects",
    async (legacy) => {
      localStorage.removeItem("monocode.jiraProjectSelection.v2");
      localStorage.setItem("monocode.jiraHiddenProjects", legacy);
      localStorage.setItem("monocode.jiraProjectSelectionSet", "1");
      status = { ...connectedStatus, connectionId: "migration-connection" };
      await jiraConnected();
      expect(jiraProjectSelectionNeedsReview()).toBe(true);
      expect(jiraProjectSelectionConfigured()).toBe(false);
      expect((await listInboxItems([], query)).items).toEqual([]);
      expect(invoke).not.toHaveBeenCalledWith(
        "jira_list_issues",
        expect.anything(),
      );
      saveSelectedJiraProjectIds(["10000"]);
      expect(jiraProjectSelectionNeedsReview()).toBe(false);
      expect(localStorage.getItem("monocode.jiraHiddenProjects")).toBeNull();
    },
  );

  it.each([
    {
      ...connectedStatus,
      connectionId: "datacenter-ada",
      deployment: "datacenter" as const,
    },
    {
      ...connectedStatus,
      connectionId: "cloud-other-site",
      site: "https://other.atlassian.net",
    },
    {
      ...connectedStatus,
      connectionId: "cloud-other-account",
      email: "other@example.com",
    },
  ])("does not transfer project IDs to $connectionId", async (next) => {
    saveSelectedJiraProjectIds(["10000"]);
    status = next;
    vi.mocked(invoke).mockResolvedValueOnce(status);
    await saveJiraConfig({ ...status, token: "new-credentials" });
    expect(loadSelectedJiraProjectIds()).toEqual([]);
    expect(jiraProjectSelectionConfigured()).toBe(false);
    // A late click from the old connection's Settings cannot select IDs here.
    saveSelectedJiraProjectIds(["10000"], connectedStatus.connectionId);
    expect(loadSelectedJiraProjectIds()).toEqual([]);
    expect((await listInboxItems([], query)).items).toEqual([]);
    expect(invoke).not.toHaveBeenCalledWith(
      "jira_list_issues",
      expect.anything(),
    );
  });

  it("retains explicit selection on reconnect to the same verified account", async () => {
    saveSelectedJiraProjectIds(["10000"]);
    vi.mocked(invoke).mockResolvedValueOnce(status);
    await saveJiraConfig({ ...status, token: "rotated-token" });
    expect(loadSelectedJiraProjectIds()).toEqual(["10000"]);
    expect(jiraProjectSelectionConfigured()).toBe(true);
  });

  it("keeps None effective in memory when storage rejects the write", async () => {
    saveSelectedJiraProjectIds(["10000"]);
    const setItem = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {
        throw new Error("Quota exceeded");
      });
    try {
      saveSelectedJiraProjectIds([]);
      expect(loadSelectedJiraProjectIds()).toEqual([]);
      expect((await listInboxItems([], query)).items).toEqual([]);
      expect(invoke).not.toHaveBeenCalledWith(
        "jira_list_issues",
        expect.anything(),
      );
    } finally {
      setItem.mockRestore();
      clearJiraProjectSelection();
    }
  });

  it("keeps failed discovery as an error and preserves the explicit selection", async () => {
    saveSelectedJiraProjectIds(["10000"]);
    const original = vi.mocked(invoke).getMockImplementation()!;
    let fail = true;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "jira_list_projects" && fail)
        throw new Error("Jira rate limited");
      return original(command, args);
    });
    expect((await listInboxItems([], query)).errors).toEqual({
      jira: "Jira rate limited",
    });
    expect(loadSelectedJiraProjectIds()).toEqual(["10000"]);
    expect(invoke).not.toHaveBeenCalledWith(
      "jira_list_issues",
      expect.anything(),
    );
    fail = false;
    await listInboxItems([], query, { force: true });
    expect(invoke).toHaveBeenCalledWith(
      "jira_list_issues",
      expect.objectContaining({ projectIds: ["10000"] }),
    );
  });

  it("rejects an old query that attempts to override None", async () => {
    saveSelectedJiraProjectIds([]);
    await listInboxItems([], { ...query, jiraSelectedProjectIds: ["10000"] });
    await listJiraIssues({ ...query, projectIds: ["10000"] });
    expect(invoke).not.toHaveBeenCalledWith(
      "jira_list_issues",
      expect.anything(),
    );
  });

  it.each([
    "None",
    "other selection",
    "account switch",
    "None then reselect",
    "cache invalidation",
    "storage update without an event",
  ])("discards an issue response after %s", async (action) => {
    saveSelectedJiraProjectIds(["10000"]);
    let resolve!: (issues: JiraIssue[]) => void;
    let started!: () => void;
    const searching = new Promise<void>((done) => {
      started = done;
    });
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "jira_list_issues") {
        started();
        return new Promise<JiraIssue[]>((done) => {
          resolve = done;
        });
      }
      return original(command, args);
    });
    const pending = listInboxItems([], query);
    const rejected = expect(pending).rejects.toThrow(/changed|superseded/);
    await searching;
    if (action === "account switch") {
      status = { ...status, connectionId: "other-account" };
      await jiraConnected();
    } else if (action === "cache invalidation") {
      clearInboxCache();
    } else {
      saveSelectedJiraProjectIds(action === "other selection" ? ["10001"] : []);
      if (action === "None then reselect")
        saveSelectedJiraProjectIds(["10000"]);
    }
    resolve([issue]);
    await rejected;
    expect(peekInboxList([], query)).toBeNull();
  });

  it("discards discovery that finishes after an account switch", async () => {
    let resolve!: (value: typeof projects) => void;
    vi.mocked(invoke)
      .mockImplementationOnce(async () => status)
      .mockImplementationOnce(
        () =>
          new Promise((done) => {
            resolve = done;
          }),
      );
    const pending = listJiraProjects();
    await vi.waitFor(() => expect(resolve).toBeTypeOf("function"));
    status = { ...status, connectionId: "other-account" };
    await jiraConnected();
    resolve(projects);
    await expect(pending).rejects.toThrow("changed");
  });

  it("keeps GitHub items when Jira settings cannot be read", async () => {
    const original = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "jira_status")
        throw new Error("Jira settings are invalid");
      if (command === "git_github_repositories") return ["acme/web"];
      if (command === "git_github_work_items") {
        return (args as { kind: string }).kind === "issue"
          ? [{ ...issue, kind: "issue", repo: "acme/web" }]
          : [];
      }
      return original(command, args);
    });
    const result = await listInboxItems([{ path: "/repo" }], query);
    expect(result.items).toHaveLength(1);
    expect(result.items[0].provider).toBe("github");
    expect(result.errors).toEqual({ jira: "Jira settings are invalid" });
  });

  it("provides Jira description and identifier to sessions", async () => {
    const description = await inboxTrackerDescription(issue);
    expect(invoke).toHaveBeenCalledWith("jira_issue_details", {
      key: "ENG-42",
    });
    expect(inboxStartDraft(issue, description)).toContain("ENG-42 Fix auth");
    expect(inboxStartDraft(issue, description)).toContain("Reproduction steps");
    vi.mocked(invoke).mockClear();
    expect(await inboxTrackerDescription(issue, "Provided description")).toBe(
      "Provided description",
    );
    expect(invoke).not.toHaveBeenCalled();
    await expect(
      inboxTrackerDescription({ ...issue, identifier: "" }),
    ).rejects.toThrow("Missing Jira issue key");
  });

  it("invalidates comments and suppresses notifications for the author's own comment", async () => {
    await jiraIssueThread(issue.identifier);
    expect(peekJiraIssueThread(issue.identifier)).not.toBeNull();
    await jiraIssueComment(
      { id: issue.id, key: issue.identifier },
      "  Fixed\n\nPlease check  ",
    );
    expect(invoke).toHaveBeenCalledWith("jira_issue_comment", {
      key: "ENG-42",
      body: "Fixed\n\nPlease check",
    });
    expect(peekJiraIssueThread(issue.identifier)).toBeNull();
    expect(consumeInboxSelfActivity(issue)).toBe(true);
    expect(consumeInboxSelfActivity(issue)).toBe(false);
  });

  it("clears credentials and cached descriptions on disconnect and reconnect", async () => {
    await jiraIssueDetails(issue.identifier);
    await disconnectJira();
    expect(invoke).toHaveBeenCalledWith("jira_set_config", {
      site: "",
      email: "",
      token: "",
      deployment: "cloud",
    });
    expect(peekJiraIssueDetails(issue.identifier)).toBeNull();
    await jiraIssueDetails(issue.identifier);
    await saveJiraConfig({
      site: " acme ",
      email: " ada@example.com ",
      token: " token ",
    });
    expect(invoke).toHaveBeenCalledWith("jira_set_config", {
      site: "acme",
      email: "ada@example.com",
      token: "token",
      deployment: "cloud",
    });
    await saveJiraConfig({
      site: " https://jira.example.com/jira ",
      email: "",
      token: " pat ",
      deployment: "datacenter",
    });
    expect(invoke).toHaveBeenCalledWith("jira_set_config", {
      site: "https://jira.example.com/jira",
      email: "",
      token: "pat",
      deployment: "datacenter",
    });
    expect(peekJiraIssueDetails(issue.identifier)).toBeNull();
  });

  it("does not restore a previous account's cache when a request finishes late", async () => {
    let resolve!: (value: unknown) => void;
    vi.mocked(invoke).mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const pending = jiraIssueDetails(issue.identifier);
    clearJiraCache();
    resolve({ body: "Old account", author: "Ada" });
    await pending;
    expect(peekJiraIssueDetails(issue.identifier)).toBeNull();
  });
});
