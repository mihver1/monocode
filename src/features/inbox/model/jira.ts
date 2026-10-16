import { invoke } from "@tauri-apps/api/core";
import { recordInboxSelfActivity } from "./inboxSelfActivity";

export type JiraProject = {
  id: string;
  key: string;
  name: string;
};

export type JiraIssue = {
  provider: "jira";
  kind: "jira";
  id: string;
  identifier: string;
  number: number;
  title: string;
  url: string;
  state: string;
  /** Jira status category: `new`, `indeterminate` or `done`. */
  stateType: string;
  updatedAt: string;
  labels: { name: string; color: string }[];
  assignees: { login: string; avatarUrl?: string }[];
  draft: boolean;
  repo: string;
  teamId: string;
  teamName: string;
  projectPath: string;
};

export type JiraIssueDetails = {
  body: string;
  author: string;
  authorAvatarUrl?: string;
};

export type JiraIssueComment = {
  id: string;
  kind: string;
  author: string;
  authorAvatarUrl?: string;
  body: string;
  createdAt: string;
  url: string;
  state: string;
  path: string;
  line: number | null;
  resolved: boolean;
  threadId: string;
  replies: JiraIssueComment[];
};

export type JiraIssueThread = {
  comments: JiraIssueComment[];
  truncated: boolean;
  reviewDecision: string;
  baseRefName: string;
  headRefName: string;
};

export type JiraDeployment = "cloud" | "datacenter";

export type JiraStatus = {
  connected: boolean;
  connectionId: string;
  site: string;
  email: string;
  deployment: JiraDeployment;
};

const SELECTION_KEY = "monocode.jiraProjectSelection.v2";
const LEGACY_KEYS = [
  "monocode.jiraHiddenProjects",
  "monocode.jiraProjectSelectionSet",
];
export const JIRA_CHANGE_EVENT = "monocode:jira-change";

type ProjectSelection = {
  connectionId: string;
  selectedIds: string[];
  configured: boolean;
  needsReview: boolean;
};
let connectionId = "";
let configGeneration = 0;
let selectionRevision = 0;
let volatileSelection: ProjectSelection | null = null;

export function jiraProjectSelectionCacheKey(): string {
  return JSON.stringify([
    connectionId,
    selectionRevision,
    readProjectSelection(),
  ]);
}

// Keyed by issue key (ENG-42): the REST paths and browse URLs both take it.
const detailsByKey = new Map<string, JiraIssueDetails>();
const threadByKey = new Map<string, JiraIssueThread>();
const threadInflight = new Map<string, Promise<JiraIssueThread>>();

let cacheGeneration = 0;

export function clearJiraCache() {
  cacheGeneration += 1;
  detailsByKey.clear();
  threadByKey.clear();
  threadInflight.clear();
}

export async function jiraConnected(): Promise<JiraStatus> {
  const generation = configGeneration;
  const status = await invoke<JiraStatus>("jira_status");
  if (generation === configGeneration) syncConnection(status);
  return status;
}

function syncConnection(status: JiraStatus) {
  const next = status.connected ? status.connectionId || "" : "";
  if (next === connectionId) return;
  connectionId = next;
  // Initialize independently of discovery. A denylist cannot be converted
  // without its original catalog; require confirmation instead of widening it.
  if (next) writeProjectSelection(readProjectSelection());
  clearJiraCache();
  notifyJiraChange();
}

export async function saveJiraConfig(config: {
  site: string;
  email: string;
  token: string;
  deployment?: JiraDeployment;
}): Promise<JiraStatus> {
  const status = await invoke<JiraStatus>("jira_set_config", {
    site: config.site.trim(),
    email: config.email.trim(),
    token: config.token.trim(),
    deployment: config.deployment ?? "cloud",
  });
  configGeneration += 1;
  syncConnection(status);
  clearJiraCache();
  notifyJiraChange();
  return status;
}

export async function disconnectJira(): Promise<JiraStatus> {
  const status = await invoke<JiraStatus>("jira_set_config", {
    site: "",
    email: "",
    token: "",
    deployment: "cloud",
  });
  configGeneration += 1;
  syncConnection(status);
  clearJiraCache();
  clearJiraProjectSelection();
  return status;
}

export async function listJiraProjects(): Promise<JiraProject[]> {
  const status = await jiraConnected();
  if (
    !status.connected ||
    !connectionId ||
    status.connectionId !== connectionId
  )
    return [];
  const expectedConnection = connectionId;
  const projects = await invoke<JiraProject[]>("jira_list_projects", {
    connectionId: expectedConnection,
  });
  if (expectedConnection !== connectionId)
    throw new Error("Jira connection changed during discovery");
  return projects;
}

/**
 * Only explicitly selected IDs still present in the current catalog may be fetched.
 */
export function jiraProjectIdsForFetch(
  projects: readonly JiraProject[],
  selectedIds: readonly string[],
): string[] {
  const selected = new Set(selectedIds);
  return projects
    .filter((project) => selected.has(project.id) && /^\d+$/.test(project.id))
    .map((project) => project.id);
}

export function listJiraIssues(query: {
  assignedToMe: boolean;
  state: "open" | "all";
  projectIds: string[];
  limit?: number;
}): Promise<JiraIssue[]> {
  const selected = new Set(loadSelectedJiraProjectIds());
  const projectIds = query.projectIds.filter(
    (id) => selected.has(id) && /^\d+$/.test(id),
  );
  if (!connectionId || projectIds.length === 0) return Promise.resolve([]);
  return invoke<JiraIssue[]>("jira_list_issues", {
    connectionId,
    assignedToMe: query.assignedToMe,
    state: query.state,
    projectIds,
    limit: query.limit,
  });
}

export function peekJiraIssueDetails(key: string): JiraIssueDetails | null {
  return detailsByKey.get(key) ?? null;
}

export async function jiraIssueDetails(key: string): Promise<JiraIssueDetails> {
  const generation = cacheGeneration;
  const details = await invoke<JiraIssueDetails>("jira_issue_details", {
    key,
  });
  if (generation === cacheGeneration) detailsByKey.set(key, details);
  return details;
}

export function peekJiraIssueThread(key: string): JiraIssueThread | null {
  return threadByKey.get(key) ?? null;
}

export async function jiraIssueThread(
  key: string,
  options?: { force?: boolean },
): Promise<JiraIssueThread> {
  if (options?.force) {
    threadByKey.delete(key);
    threadInflight.delete(key);
  }
  const pending = threadInflight.get(key);
  if (pending) return pending;
  const generation = cacheGeneration;
  const promise = invoke<JiraIssueThread>("jira_issue_thread", { key })
    .then((thread) => {
      if (
        generation === cacheGeneration &&
        threadInflight.get(key) === promise
      ) {
        threadByKey.set(key, thread);
      }
      return thread;
    })
    .finally(() => {
      if (threadInflight.get(key) === promise) threadInflight.delete(key);
    });
  threadInflight.set(key, promise);
  return promise;
}

export async function jiraIssueComment(
  issue: { id: string; key: string },
  body: string,
): Promise<string> {
  const url = await invoke<string>("jira_issue_comment", {
    key: issue.key,
    body: body.trim(),
  });
  threadByKey.delete(issue.key);
  threadInflight.delete(issue.key);
  recordInboxSelfActivity({ provider: "jira", kind: "jira", id: issue.id });
  return url;
}

/** False until Settings or the Inbox filter saves a project choice. */
export function jiraProjectSelectionConfigured(): boolean {
  return readProjectSelection().configured;
}

export function jiraProjectSelectionNeedsReview(): boolean {
  return readProjectSelection().needsReview;
}

function readProjectSelection(): ProjectSelection {
  if (volatileSelection?.connectionId === connectionId)
    return volatileSelection;
  const empty = {
    connectionId,
    selectedIds: [],
    configured: false,
    needsReview: false,
  };
  try {
    const raw = localStorage.getItem(SELECTION_KEY);
    if (!raw)
      return {
        ...empty,
        needsReview: LEGACY_KEYS.some(
          (key) => localStorage.getItem(key) != null,
        ),
      };
    const parsed = JSON.parse(raw) as Partial<ProjectSelection> | null;
    if (!connectionId || parsed?.connectionId !== connectionId) return empty;
    return {
      ...empty,
      configured: parsed.configured === true,
      needsReview: parsed.needsReview === true,
      selectedIds:
        parsed.configured === true && Array.isArray(parsed.selectedIds)
          ? [
              ...new Set(
                parsed.selectedIds.filter(
                  (id): id is string =>
                    typeof id === "string" && /^\d+$/.test(id),
                ),
              ),
            ]
          : [],
    };
  } catch {
    return empty;
  }
}

export function loadSelectedJiraProjectIds(): string[] {
  return readProjectSelection().selectedIds;
}

function writeProjectSelection(selection: ProjectSelection) {
  try {
    localStorage.setItem(SELECTION_KEY, JSON.stringify(selection));
    volatileSelection = null;
    for (const key of LEGACY_KEYS) localStorage.removeItem(key);
  } catch {
    // A failed write of None must still revoke access for this session.
    volatileSelection = selection;
  }
}

export function saveSelectedJiraProjectIds(
  ids: string[],
  expectedConnectionId = connectionId,
) {
  if (!connectionId || expectedConnectionId !== connectionId) return;
  writeProjectSelection({
    connectionId,
    selectedIds: [...new Set(ids.filter((id) => /^\d+$/.test(id)))],
    configured: true,
    needsReview: false,
  });
  notifyJiraChange();
}

export function clearJiraProjectSelection() {
  writeProjectSelection({
    connectionId,
    selectedIds: [],
    configured: false,
    needsReview: false,
  });
  notifyJiraChange();
}

export function notifyJiraChange() {
  selectionRevision += 1;
  if (typeof window === "undefined") return;
  window.dispatchEvent(new Event(JIRA_CHANGE_EVENT));
}
