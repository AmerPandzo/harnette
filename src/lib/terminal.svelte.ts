// "Open in terminal": the terminals found on this Mac (Stećak first) and the one the user picked.

import { invoke } from "@tauri-apps/api/core";
import type { Editor } from "./editor.svelte";
import { remember, remembered } from "./types";

export const terminals = $state({
  list: [] as Editor[],
  chosen: remembered("harnette.terminal", ""),
  error: "",
});

let loading: Promise<void> | null = null;

/** Loads the installed terminals once; the first one found is the default until the user picks. */
export function loadTerminals(): Promise<void> {
  loading ??= invoke<Editor[]>("list_terminals")
    .then((list) => {
      terminals.list = list;
      if (!list.some((t) => t.id === terminals.chosen)) terminals.chosen = list[0]?.id ?? "terminal";
    })
    .catch(() => {});
  return loading;
}

export function chooseTerminal(id: string) {
  terminals.chosen = id;
  remember("harnette.terminal", id);
}

export const terminalName = () => terminals.list.find((t) => t.id === terminals.chosen)?.name ?? "Terminal";

/** A shell in `repo`, or `claude --resume <resume>` there. */
export async function openTerminal(repo: string, resume?: string) {
  await loadTerminals();
  try {
    await invoke("open_terminal", { repo, terminal: terminals.chosen, resume: resume || null });
    terminals.error = "";
  } catch (e) {
    terminals.error = String(e);
  }
}
