import { useCallback, useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { PrivateEmail } from "../../../shared/ui/PrivateEmail";
import { clearInboxCache } from "../../inbox/model/githubTasks";
import {
  disconnectJira,
  JIRA_CHANGE_EVENT,
  jiraConnected,
  listJiraProjects,
  loadHiddenJiraProjectIds,
  notifyJiraChange,
  saveHiddenJiraProjectIds,
  saveJiraConfig,
  type JiraDeployment,
  type JiraProject,
  type JiraStatus,
} from "../../inbox/model/jira";

const TOKEN_HELP = {
  cloud: "https://id.atlassian.com/manage-profile/security/api-tokens",
  datacenter:
    "https://confluence.atlassian.com/enterprise/using-personal-access-tokens-1026032365.html",
} as const;

function deploymentOf(status: JiraStatus | null): JiraDeployment {
  return status?.deployment === "datacenter" ? "datacenter" : "cloud";
}

export function JiraSettings() {
  const [status, setStatus] = useState<JiraStatus | null>(null);
  const [deployment, setDeployment] = useState<JiraDeployment>("cloud");
  const [site, setSite] = useState("");
  const [email, setEmail] = useState("");
  const [token, setToken] = useState("");
  const [checking, setChecking] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [projects, setProjects] = useState<JiraProject[]>([]);
  const [hiddenIds, setHiddenIds] = useState(loadHiddenJiraProjectIds);
  const cloud = deployment === "cloud";

  const loadProjects = useCallback(async () => {
    try {
      setProjects(await listJiraProjects());
    } catch (err) {
      setProjects([]);
      setError(String(err instanceof Error ? err.message : err));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void jiraConnected()
      .then(async (next) => {
        if (cancelled) return;
        setStatus(next);
        if (next.connected) setDeployment(deploymentOf(next));
        if (next.connected) await loadProjects();
      })
      .catch((err: unknown) => {
        if (!cancelled)
          setError(String(err instanceof Error ? err.message : err));
      })
      .finally(() => {
        if (!cancelled) setChecking(false);
      });
    const onChange = () => setHiddenIds(loadHiddenJiraProjectIds());
    window.addEventListener(JIRA_CHANGE_EVENT, onChange);
    return () => {
      cancelled = true;
      window.removeEventListener(JIRA_CHANGE_EVENT, onChange);
    };
  }, [loadProjects]);

  const connect = async () => {
    if (busy || checking || !site.trim() || !token.trim()) return;
    if (cloud && !email.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const next = await saveJiraConfig({
        site,
        email: cloud ? email : "",
        token,
        deployment,
      });
      setStatus(next);
      setDeployment(deploymentOf(next));
      setToken("");
      clearInboxCache();
      saveHiddenJiraProjectIds([]);
      await loadProjects();
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setStatus(await disconnectJira());
      setProjects([]);
      setToken("");
      clearInboxCache();
      notifyJiraChange();
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
    } finally {
      setBusy(false);
    }
  };

  const fields = [
    {
      label: cloud ? "Jira site" : "Jira URL",
      value: site,
      set: setSite,
      type: "text",
      placeholder: cloud ? "yourteam.atlassian.net" : "jira.example.com/jira",
    },
    ...(cloud
      ? [
          {
            label: "Atlassian email",
            value: email,
            set: setEmail,
            type: "email",
            placeholder: "you@example.com",
          },
        ]
      : []),
    {
      label: cloud ? "Jira API token" : "Personal access token",
      value: token,
      set: setToken,
      type: "password",
      placeholder: cloud ? "API token" : "Personal access token",
    },
  ] as const;

  const connectedStatus =
    status?.connected && deploymentOf(status) === deployment ? status : null;

  return (
    <div className="px-4 py-3.5">
      {checking ? (
        <p className="text-[12px] text-content/45">Checking Jira connection…</p>
      ) : (
        <div className="flex flex-col gap-3">
          <div
            role="radiogroup"
            aria-label="Jira product"
            className="flex gap-1"
          >
            {(
              [
                ["cloud", "Jira Cloud"],
                ["datacenter", "Jira Data Center"],
              ] as const
            ).map(([value, label]) => (
              <button
                key={value}
                type="button"
                role="radio"
                aria-checked={deployment === value}
                disabled={busy}
                onClick={() => setDeployment(value)}
                className={`h-7 rounded-md border px-2.5 text-[12px] ${
                  deployment === value
                    ? "border-content/25 bg-content/10 text-content"
                    : "border-content/10 text-content/55 hover:text-content"
                }`}
              >
                {label}
              </button>
            ))}
          </div>
          {connectedStatus ? (
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="min-w-0 text-[12px] text-content/65">
                <p className="break-all">{connectedStatus.site}</p>
                {connectedStatus.email ? (
                  deployment === "datacenter" &&
                  !connectedStatus.email.includes("@") ? (
                    <p className="break-all">{connectedStatus.email}</p>
                  ) : (
                    <p className="flex min-w-0">
                      <PrivateEmail
                        key={connectedStatus.email}
                        email={connectedStatus.email}
                      />
                    </p>
                  )
                ) : null}
              </div>
              <SecondaryButton onClick={() => void disconnect()} disabled={busy}>
                {busy ? "Disconnecting" : "Disconnect"}
              </SecondaryButton>
            </div>
          ) : (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void connect();
              }}
              className="flex flex-col gap-3"
            >
              <p className="text-[12px] leading-relaxed text-content/45">
                {cloud
                  ? "Connect your Jira Cloud site using your Atlassian email and an API token without scopes. Disconnect deletes the saved credentials."
                  : "Connect Jira Data Center with a personal access token from your profile. Use the site root, including a context path such as /jira. Disconnect deletes the saved credentials."}
              </p>
              {fields.map((field) => (
                <label
                  key={field.label}
                  className="flex flex-col gap-1 text-[12px] text-content/65"
                >
                  {field.label}
                  <input
                    aria-label={field.label}
                    type={field.type}
                    value={field.value}
                    onChange={(event) => field.set(event.target.value)}
                    placeholder={field.placeholder}
                    disabled={busy}
                    required
                    autoComplete="off"
                    spellCheck={false}
                    className="h-8 w-full rounded-md border border-content/10 bg-transparent px-2 text-content outline-none focus:border-content/20"
                  />
                </label>
              ))}
              <div className="flex items-center gap-3">
                <SecondaryButton
                  type="submit"
                  disabled={
                    busy ||
                    !site.trim() ||
                    !token.trim() ||
                    (cloud && !email.trim())
                  }
                >
                  {busy ? "Connecting" : "Connect"}
                </SecondaryButton>
                <button
                  type="button"
                  onClick={() => void openUrl(TOKEN_HELP[deployment])}
                  className="text-[12px] text-content/65 hover:text-content"
                >
                  {cloud ? "Create API token" : "Create a personal access token"}
                </button>
              </div>
            </form>
          )}
        </div>
      )}
      {error ? (
        <p role="alert" className="mt-3 text-[12px] text-red-400/90">
          {error}
        </p>
      ) : null}
      {connectedStatus ? (
        <div className="mt-4 flex flex-col gap-2">
          <div className="flex items-center justify-between">
            <span className="text-[13px] font-medium text-content">
              Projects
            </span>
            <SecondaryButton
              disabled={busy || checking}
              onClick={() => void loadProjects()}
            >
              Refresh projects
            </SecondaryButton>
          </div>
          <p className="text-[12px] text-content/45">
            Unchecked projects stay out of the inbox.
          </p>
          {projects.map((project) => (
            <label
              key={project.id}
              className="flex items-center gap-2 text-[13px] text-content"
            >
              <input
                type="checkbox"
                checked={!hiddenIds.includes(project.id)}
                disabled={busy}
                onChange={() => {
                  const next = hiddenIds.includes(project.id)
                    ? hiddenIds.filter((id) => id !== project.id)
                    : [...hiddenIds, project.id];
                  clearInboxCache();
                  saveHiddenJiraProjectIds(next);
                }}
              />
              {project.name}{" "}
              <span className="text-content/40">{project.key}</span>
            </label>
          ))}
        </div>
      ) : null}
    </div>
  );
}
