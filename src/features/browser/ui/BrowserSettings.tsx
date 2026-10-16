import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import type { BrowserProfile, BrowserStatus } from "../model/browser";

const selectClass =
  "w-full rounded-lg border border-content/15 bg-content/3 px-3 py-2 text-[13px] outline-none focus:border-content/35 disabled:opacity-40";

export function BrowserSettings() {
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [sourceId, setSourceId] = useState("");
  const [pending, setPending] = useState<string | null>(
    "Loading browser status…",
  );
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const mounted = useRef(false);
  const busy = useRef(false);

  const loadStatus = useCallback(async () => {
    const next = await invoke<BrowserStatus>("browser_status");
    if (mounted.current) {
      setStatus(next);
      setSourceId((current) =>
        next.sources.some((source) => source.id === current)
          ? current
          : (next.sources[0]?.id ?? ""),
      );
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    busy.current = true;
    void loadStatus()
      .catch((reason: unknown) => {
        if (mounted.current) setError(String(reason));
      })
      .finally(() => {
        busy.current = false;
        if (mounted.current) setPending(null);
      });
    return () => {
      mounted.current = false;
    };
  }, [loadStatus]);

  async function perform(label: string, action: () => Promise<string>) {
    if (busy.current) return;
    busy.current = true;
    setPending(label);
    setError("");
    setNotice("");
    try {
      const message = await action();
      await loadStatus();
      if (mounted.current) setNotice(message);
    } catch (reason) {
      if (mounted.current) setError(String(reason));
    } finally {
      busy.current = false;
      if (mounted.current) setPending(null);
    }
  }

  async function importProfile() {
    const source = status?.sources.find((profile) => profile.id === sourceId);
    if (!source || status?.running) return;
    await perform("Importing browser profile…", async () => {
      const confirmed = await ask(
        `Import “${source.name}” from ${source.browser}?\n\nFully close the source browser before continuing. StereoCode copies this profile into a new isolated profile; the source is unchanged.\n\nCopied logged-in sites become accessible to all agents with app access: /operator threads, Monos, and habit runs. Only import accounts you want those agents to access.\n\nEncrypted cookies, passkeys, and some sessions may not transfer. You may need to sign in again; importing does not guarantee all logins will work.`,
        {
          title: "Import browser profile",
          kind: "warning",
          okLabel: "Import profile",
          cancelLabel: "Cancel",
        },
      );
      if (!confirmed) return "Import cancelled. No profile was copied.";
      const imported = await invoke<BrowserProfile>("browser_import", {
        sourceId: source.id,
      });
      return `Imported and selected “${imported.name}”. Launch the browser to check your sign-ins.`;
    });
  }

  const disabled = pending !== null;
  return (
    <div className="flex flex-col gap-6 text-[13px]">
      <p className="text-content/50">
        Enable browser access in a thread with <code>/operator</code>. Monos and
        habit runs use their existing app access. Agents can navigate, inspect
        pages, interact, take screenshots, and test localhost apps. This opens a
        real, visible Chromium-family browser in a separate window, not an
        embedded preview. Its isolated profile and sign-in sessions persist
        between launches.
      </p>

      <section
        data-setting-id="browser-session"
        className="flex flex-col gap-3"
        aria-labelledby="browser-session-heading"
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 id="browser-session-heading" className="font-medium">
            Browser session
          </h3>
          <SecondaryButton
            disabled={disabled}
            onClick={() =>
              void perform(
                "Refreshing browser status…",
                async () => "Browser status and available profiles refreshed.",
              )
            }
          >
            Refresh
          </SecondaryButton>
        </div>
        <p className="text-content/50">
          {status
            ? status.running
              ? "Browser is running. Stop it before changing or importing profiles."
              : "Browser is stopped."
            : "Browser status is not available yet."}{" "}
          Launch it to sign in manually. If no profile is selected, launching
          creates a fresh isolated profile using an installed supported browser.
        </p>
        <div className="flex flex-wrap gap-2">
          <SecondaryButton
            disabled={disabled || !status || status.running}
            onClick={() =>
              void perform("Launching browser…", async () => {
                await invoke<unknown>("browser_launch");
                return "Browser launched in a separate window. You can sign in there manually.";
              })
            }
          >
            Launch browser
          </SecondaryButton>
          <SecondaryButton
            disabled={disabled || !status?.running}
            onClick={() =>
              void perform("Stopping browser…", async () => {
                await invoke<void>("browser_shutdown");
                return "Browser stopped. Your profile is retained for the next launch.";
              })
            }
          >
            Stop browser
          </SecondaryButton>
        </div>
      </section>

      <section
        data-setting-id="browser-profile"
        className="flex flex-col gap-2"
        aria-labelledby="browser-profile-heading"
      >
        <label
          id="browser-profile-heading"
          htmlFor="browser-profile"
          className="font-medium"
        >
          Selected isolated profile
        </label>
        <select
          id="browser-profile"
          className={selectClass}
          value={status?.selectedProfile?.id ?? ""}
          disabled={
            disabled || !status || status.running || !status.profiles.length
          }
          onChange={(event) => {
            const id = event.target.value;
            void perform("Selecting browser profile…", async () => {
              await invoke<void>("browser_select", { id });
              return "Browser profile selected.";
            });
          }}
        >
          {!status?.selectedProfile && (
            <option value="">No profile selected</option>
          )}
          {status?.profiles.map((profile) => (
            <option key={profile.id} value={profile.id}>
              {profile.name} · {profile.id.slice(0, 8)}
            </option>
          ))}
        </select>
        <p className="text-content/50">
          Profiles are separate from your everyday browser. All agents with app
          access share the selected profile’s cookies, including /operator
          threads, Monos, and habit runs. Tabs are private to each session.
        </p>
      </section>

      <section
        data-setting-id="browser-import"
        className="flex flex-col gap-3"
        aria-labelledby="browser-import-heading"
      >
        <label
          id="browser-import-heading"
          htmlFor="browser-source"
          className="font-medium"
        >
          Import an existing browser profile
        </label>
        <p className="text-content/50">
          Supports Chrome, Edge, Brave, Chromium, and Helium; Safari and Firefox
          profiles are not compatible. Fully close the source browser first.
          Import creates and selects a new isolated copy without changing the
          source. Copied logged-in sites become accessible to all agents with
          app access. Encrypted cookies, passkeys, and some sessions may not
          transfer; you may need to sign in again.
        </p>
        <select
          id="browser-source"
          className={selectClass}
          value={sourceId}
          disabled={
            disabled || !status || status.running || !status.sources.length
          }
          onChange={(event) => setSourceId(event.target.value)}
        >
          {!status?.sources.length && (
            <option value="">No source profiles discovered</option>
          )}
          {status?.sources.map((profile) => (
            <option key={profile.id} value={profile.id}>
              {profile.name}
            </option>
          ))}
        </select>
        <div className="flex">
          <SecondaryButton
            disabled={disabled || !status || status.running || !sourceId}
            onClick={() => void importProfile()}
          >
            Import selected profile…
          </SecondaryButton>
        </div>
      </section>
      {pending && (
        <p role="status" className="text-content/50">
          {pending}
        </p>
      )}
      {error && (
        <p role="alert" className="text-red-400">
          {error}
        </p>
      )}
      {notice && (
        <p role="status" className="text-content/70">
          {notice}
        </p>
      )}
    </div>
  );
}
