// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { InboxFiltersMenu } from "./InboxFiltersMenu";
import { DEFAULT_INBOX_FILTERS } from "../model/inboxFilters";

let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

const projects = [
  { id: "10000", key: "ENG", name: "Engineering" },
  { id: "10001", key: "OPS", name: "Operations" },
];

it("renders Jira checkboxes from the allowlist and saves explicit toggles", async () => {
  const onJiraProjectsChange = vi.fn();
  const render = async (selectedJiraProjectIds: string[]) => {
    await act(async () =>
      root.render(
        createElement(InboxFiltersMenu, {
          x: 20,
          y: 20,
          projects: [],
          linearProjects: [],
          linearTeams: [],
          hiddenLinearTeamIds: [],
          jiraProjects: projects,
          selectedJiraProjectIds,
          source: "jira",
          filters: DEFAULT_INBOX_FILTERS,
          onChange: vi.fn(),
          onLinearTeamsChange: vi.fn(),
          onJiraProjectsChange,
          onClose: vi.fn(),
        }),
      ),
    );
  };
  const project = (name: string) =>
    [
      ...document.querySelectorAll<HTMLButtonElement>(
        '[role="menuitemcheckbox"]',
      ),
    ].find((entry) => entry.textContent === name)!;
  await render([]);
  expect(project("Engineering").getAttribute("aria-checked")).toBe("false");
  expect(project("Operations").getAttribute("aria-checked")).toBe("false");
  await act(async () => project("Engineering").click());
  expect(onJiraProjectsChange).toHaveBeenLastCalledWith(["10000"]);
  await render(["10000"]);
  expect(project("Engineering").getAttribute("aria-checked")).toBe("true");
  expect(project("Operations").getAttribute("aria-checked")).toBe("false");
  await act(async () => project("Engineering").click());
  expect(onJiraProjectsChange).toHaveBeenLastCalledWith([]);
  const clear = [
    ...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
  ].find((entry) => entry.textContent === "Clear filters")!;
  await act(async () => clear.click());
  expect(onJiraProjectsChange).toHaveBeenLastCalledWith(["10000", "10001"]);
});
