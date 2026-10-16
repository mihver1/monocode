// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { JiraSettings } from "./JiraSettings";
import {
  jiraProjectSelectionConfigured,
  loadSelectedJiraProjectIds,
  type JiraStatus,
} from "../../inbox/model/jira";
import { saveMaskEmails } from "../model/displayPrefs";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

let container: HTMLDivElement;
let root: Root;
let status: JiraStatus;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(invoke).mockReset();
  status = {
    connected: false,
    connectionId: "",
    site: "",
    email: "",
    deployment: "cloud",
  };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "jira_status") return status;
    if (command === "jira_set_config") {
      const config = args as {
        site: string;
        email: string;
        token: string;
        deployment?: JiraStatus["deployment"];
      };
      status = {
        connected: Boolean(config.token),
        connectionId: config.token
          ? `${config.deployment}:${config.site}:${config.email}`
          : "",
        site: config.site,
        email: config.email,
        deployment: config.deployment ?? "cloud",
      };
      return status;
    }
    if (command === "jira_list_projects")
      return [{ id: "10000", key: "ENG", name: "Engineering" }];
    throw new Error(`Unexpected command: ${command}`);
  });
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  localStorage.clear();
  vi.unstubAllGlobals();
});

async function input(label: string, value: string) {
  const field = container.querySelector<HTMLInputElement>(
    `input[aria-label="${label}"]`,
  )!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function submit() {
  await act(async () => {
    container
      .querySelector("form")!
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
}

it("connects Jira, synchronizes project filters, and disconnects", async () => {
  saveMaskEmails(true);
  await act(async () => root.render(createElement(JiraSettings)));
  await input("Jira site", "acme.atlassian.net");
  await input("Atlassian email", "ada@example.com");
  await input("Jira API token", "secret");
  await submit();
  expect(invoke).toHaveBeenCalledWith("jira_set_config", {
    site: "acme.atlassian.net",
    email: "ada@example.com",
    token: "secret",
    deployment: "cloud",
  });
  expect(container.textContent).toContain("ada@example.com");
  const email = container.querySelector<HTMLButtonElement>(
    '[aria-label="Reveal email"]',
  )!;
  expect(email.querySelector("span")?.className).toContain("blur-[5px]");
  await act(async () => email.click());
  expect(email.getAttribute("aria-label")).toBe("Hide email");
  await act(async () => email.click());
  expect(email.getAttribute("aria-label")).toBe("Reveal email");
  expect(container.querySelector('input[type="password"]')).toBeNull();
  const project = container.querySelector<HTMLInputElement>(
    'input[type="checkbox"]',
  )!;
  expect(project.checked).toBe(false);
  expect(loadSelectedJiraProjectIds()).toEqual([]);
  expect(container.textContent).toContain("0 of 1 selected");
  await act(async () => project.click());
  expect(loadSelectedJiraProjectIds()).toEqual(["10000"]);
  expect(project.checked).toBe(true);
  const disconnect = [...container.querySelectorAll("button")].find(
    (button) => button.textContent === "Disconnect",
  )!;
  await act(async () => disconnect.click());
  expect(invoke).toHaveBeenCalledWith("jira_set_config", {
    site: "",
    email: "",
    token: "",
    deployment: "cloud",
  });
  expect(
    container.querySelector<HTMLInputElement>('input[type="password"]')!.value,
  ).toBe("");
});

it("connects Jira Data Center with a personal access token", async () => {
  await act(async () => root.render(createElement(JiraSettings)));
  const datacenter = [...container.querySelectorAll("button")].find(
    (button) => button.textContent === "Jira Data Center",
  )!;
  await act(async () => datacenter.click());
  expect(
    container.querySelector('input[aria-label="Atlassian email"]'),
  ).toBeNull();
  await input("Jira URL", "https://jira.example.com/jira/browse/ENG-1");
  await input("Personal access token", "pat");
  await submit();
  expect(invoke).toHaveBeenCalledWith("jira_set_config", {
    site: "https://jira.example.com/jira/browse/ENG-1",
    email: "",
    token: "pat",
    deployment: "datacenter",
  });
  expect(container.textContent).toContain("Jira Data Center");
  expect(container.querySelector("form")).toBeNull();
});

it("keeps projects unchecked after failed initial discovery and successful Refresh", async () => {
  const original = vi.mocked(invoke).getMockImplementation()!;
  let fail = true;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "jira_list_projects" && fail)
      throw new Error("Discovery unavailable");
    return original(command, args);
  });
  await act(async () => root.render(createElement(JiraSettings)));
  await input("Jira site", "acme.atlassian.net");
  await input("Atlassian email", "ada@example.com");
  await input("Jira API token", "secret");
  await submit();
  expect(container.querySelector('[role="alert"]')?.textContent).toContain(
    "Discovery unavailable",
  );
  expect(jiraProjectSelectionConfigured()).toBe(false);
  fail = false;
  const refresh = [...container.querySelectorAll("button")].find(
    (button) => button.textContent === "Refresh projects",
  )!;
  await act(async () => refresh.click());
  expect(
    container.querySelector<HTMLInputElement>('input[type="checkbox"]')!
      .checked,
  ).toBe(false);
  expect(container.textContent).toContain("0 of 1 selected");
  expect(container.querySelector('[role="alert"]')).toBeNull();
});

it("keeps newly discovered projects unchecked after All and None", async () => {
  status = {
    connected: true,
    connectionId: "existing-connection",
    site: "acme",
    email: "ada@example.com",
    deployment: "cloud",
  };
  const original = vi.mocked(invoke).getMockImplementation()!;
  let projects = [{ id: "10000", key: "ENG", name: "Engineering" }];
  vi.mocked(invoke).mockImplementation(async (command, args) =>
    command === "jira_list_projects" ? projects : original(command, args),
  );
  await act(async () => root.render(createElement(JiraSettings)));
  const button = (text: string) =>
    [...container.querySelectorAll("button")].find(
      (entry) => entry.textContent === text,
    )!;
  await act(async () => button("All").click());
  expect(loadSelectedJiraProjectIds()).toEqual(["10000"]);
  projects = [...projects, { id: "10001", key: "OPS", name: "Operations" }];
  await act(async () => button("Refresh projects").click());
  expect(
    [
      ...container.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'),
    ].map((entry) => entry.checked),
  ).toEqual([true, false]);
  await act(async () => button("None").click());
  projects = [...projects, { id: "10002", key: "NEW", name: "New" }];
  await act(async () => button("Refresh projects").click());
  expect(loadSelectedJiraProjectIds()).toEqual([]);
  expect(container.textContent).toContain("0 of 3 selected");
});

it("shows authentication errors without claiming a successful connection", async () => {
  await act(async () => root.render(createElement(JiraSettings)));
  await input("Jira site", "acme");
  await input("Atlassian email", "ada@example.com");
  await input("Jira API token", "bad-token");
  vi.mocked(invoke).mockRejectedValueOnce(
    new Error("Jira email or API token is invalid"),
  );
  await submit();
  expect(container.querySelector('[role="alert"]')?.textContent).toContain(
    "invalid",
  );
  expect(container.querySelector("form")).not.toBeNull();
  expect(container.textContent).not.toContain("Projects");
});
