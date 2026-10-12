// Images the human attaches for Claude: kept in memory until sent, then saved under
// `.ai-tasks/<slug>.assets/` by the backend, which passes them to the agent.

import { invoke } from "@tauri-apps/api/core";

export type Pending = { id: number; name: string; mime: string; data: string };

export const IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];
let next = 1;

export const preview = (p: Pending) => `data:${p.mime};base64,${p.data}`;

function base64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export async function fromFile(f: File): Promise<Pending> {
  if (!IMAGE_TYPES.includes(f.type)) throw new Error(`${f.name || "That file"} is not a PNG, JPEG, GIF or WebP image.`);
  return { id: next++, name: f.name || "pasted image", mime: f.type, data: base64(await f.arrayBuffer()) };
}

/** A file dropped on the window (Tauri gives us its path, not its bytes). */
export async function fromPath(path: string): Promise<Pending> {
  const [mime, data] = await invoke<[string, string]>("read_image_file", { path });
  return { id: next++, name: path.split("/").pop() ?? path, mime, data };
}

/** Images on the clipboard of a paste event; empty when it is plain text. */
export function pastedFiles(e: ClipboardEvent): File[] {
  return [...(e.clipboardData?.files ?? [])].filter((f) => IMAGE_TYPES.includes(f.type));
}

/** Save them for this task; returns the repo-relative paths to pass to the run. */
export async function upload(repo: string, slug: string, items: Pending[]): Promise<string[]> {
  const paths: string[] = [];
  for (const p of items) paths.push(await invoke<string>("attach_image", { repo, slug, mime: p.mime, data: p.data }));
  return paths;
}

const cache = new Map<string, Promise<string>>();
/** A saved image as a data URL, for the feed. */
export function savedPreview(repo: string, path: string): Promise<string> {
  const key = `${repo}\n${path}`;
  if (!cache.has(key)) cache.set(key, invoke<string>("image_data", { repo, path }));
  return cache.get(key)!;
}
