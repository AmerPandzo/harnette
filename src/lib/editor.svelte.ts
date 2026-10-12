// "Open in editor": the editors found on this Mac and the one the user picked, shared by every view.

import { invoke } from "@tauri-apps/api/core";
import { remember, remembered } from "./types";

export type Editor = { id: string; name: string };

export const editors = $state({
  list: [] as Editor[],
  chosen: remembered("kormilant.editor", ""),
  error: "",
});

let loading: Promise<void> | null = null;

/** Loads the installed editors once; the first one found is the default until the user picks. */
export function loadEditors(): Promise<void> {
  loading ??= invoke<Editor[]>("list_editors")
    .then((list) => {
      editors.list = list;
      if (!list.some((e) => e.id === editors.chosen)) editors.chosen = list[0]?.id ?? "system";
    })
    .catch(() => {
      editors.list = [{ id: "system", name: "Default app" }];
      editors.chosen = "system";
    });
  return loading;
}

export function chooseEditor(id: string) {
  editors.chosen = id;
  remember("kormilant.editor", id);
}

export const editorName = () => editors.list.find((e) => e.id === editors.chosen)?.name ?? "editor";

/** path is relative to the repo; line is 1-based. */
export async function openInEditor(repo: string, path: string, line?: number) {
  await loadEditors();
  try {
    await invoke("open_in_editor", { repo, path, line: line ?? null, editor: editors.chosen });
    editors.error = "";
  } catch (e) {
    editors.error = String(e);
  }
}
