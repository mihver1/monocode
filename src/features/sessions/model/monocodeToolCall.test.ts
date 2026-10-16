import { describe, expect, it } from "vitest";
import type { Block } from "./session";
import { monoCodeToolCall, monoCodeWorkSummary } from "./monocodeToolCall";

function shell(text: string): Block {
  return { id: text, role: "tool", text, tool: { kind: "shell" } };
}

describe("StereoCode CLI tool calls", () => {
  it("recognizes app actions with absolute, quoted, or bare executables", () => {
    expect(
      monoCodeToolCall(
        shell(
          "/repo/target/debug/StereoCode.app/Contents/MacOS/stereocode app notes.list --json '{}'",
        ),
      )?.label,
    ).toBe("List notes");
    expect(
      monoCodeToolCall(
        shell(
          "'/Applications/StereoCode App/stereocode' app folders.move --input -",
        ),
      )?.label,
    ).toBe("Move a session");
    expect(
      monoCodeToolCall(
        shell('"C:\\Program Files\\StereoCode\\stereocode.exe" app notes.list'),
      )?.label,
    ).toBe("List notes");
    expect(monoCodeToolCall(shell("stereocode app --help"))?.label).toBe(
      "View CLI commands",
    );
    expect(
      monoCodeToolCall(shell("stereocode app sessions.read --json '{}'"))?.label,
    ).toBe("Read a session");
    expect(
      monoCodeToolCall(shell("stereocode app sessions.send --json '{}'"))?.label,
    ).toBe("Continue a session");
    expect(
      monoCodeToolCall(shell("stereocode app sessions.draft --json '{}'"))?.label,
    ).toBe("Save a draft");
    for (const [action, label] of [
      ["sessions.stop", "Stop a session"],
      ["sessions.archive", "Archive a session"],
      ["sessions.delete", "Delete a session"],
    ]) {
      expect(
        monoCodeToolCall(shell(`stereocode app ${action} --json '{}'`))?.label,
      ).toBe(label);
    }
    expect(
      monoCodeToolCall(shell("stereocode app notes.write --input -"))?.label,
    ).toBe("Write a note");
    expect(
      monoCodeToolCall({
        id: "generic",
        role: "tool",
        text: "Run command:",
        tool: { kind: "other", title: "stereocode app sessions.start" },
      })?.label,
    ).toBe("Start a session");
    expect(
      monoCodeToolCall({
        id: "codex-action",
        role: "tool",
        text: "List",
        tool: {
          kind: "execute",
          preview: { kind: "shell", title: "stereocode app notes.list" },
        },
      })?.label,
    ).toBe("List notes");
  });

  it("does not restyle unrelated commands or text mentioning the CLI", () => {
    expect(
      monoCodeToolCall(shell("echo stereocode app notes.list")),
    ).toBeUndefined();
    expect(monoCodeToolCall(shell("stereocode control list"))).toBeUndefined();
    expect(
      monoCodeToolCall({
        id: "prose",
        role: "assistant",
        text: "stereocode app notes.list",
      }),
    ).toBeUndefined();
  });

  it("does not compact compound shell commands or hide a longer shell preview", () => {
    for (const command of [
      "stereocode app notes.list && echo extra",
      "stereocode app notes.list; echo extra",
      "stereocode app notes.list | cat",
      "stereocode app notes.list\necho extra",
      "stereocode app notes.list --json \"$(echo extra)\"",
    ]) {
      expect(monoCodeToolCall(shell(command))).toBeUndefined();
    }
    expect(
      monoCodeToolCall({
        id: "short-title",
        role: "tool",
        text: "stereocode app notes.list",
        tool: {
          kind: "shell",
          title: "stereocode app notes.list",
          preview: {
            kind: "shell",
            title: "stereocode app notes.list && echo extra",
          },
        },
      }),
    ).toBeUndefined();
    expect(
      monoCodeToolCall(
        shell("stereocode app sessions.send --json '{\"prompt\":\"a; b\"}'"),
      )?.command,
    ).toBe("stereocode app sessions.send --json '{\"prompt\":\"a; b\"}'");
  });

  it("names a group only when all its tool calls use StereoCode", () => {
    const calls = [
      shell("stereocode app --help"),
      shell("stereocode app notes.list"),
    ];
    expect(monoCodeWorkSummary(calls, true)).toEqual(expect.any(String));
    expect(monoCodeWorkSummary(calls, false)).toEqual(expect.any(String));
    expect(
      monoCodeWorkSummary([...calls, shell("git status")], true),
    ).toBeUndefined();
  });
});
