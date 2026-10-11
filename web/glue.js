// Browser features the web version of Rainbow uses from Rust (see src/web.rs):
// IndexedDB for its own files, and the File System Access API for the songs folder.

const DB = "rainbow";
const FILES = "files";
const HANDLES = "handles";

let db = null;
let folder = null;

function request(r) {
  return new Promise((ok, fail) => {
    r.onsuccess = () => ok(r.result);
    r.onerror = () => fail(r.error);
  });
}

async function open() {
  if (!db) {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => {
      r.result.createObjectStore(FILES);
      r.result.createObjectStore(HANDLES);
    };
    db = await request(r);
  }
  return db;
}

async function store(name, mode) {
  return (await open()).transaction(name, mode).objectStore(name);
}

// Every saved file, as [key, bytes] pairs. Also restores the songs folder from last time.
export async function idb_load() {
  try {
    const files = await store(FILES, "readonly");
    const [keys, values] = await Promise.all([request(files.getAllKeys()), request(files.getAll())]);
    try {
      folder = (await request((await store(HANDLES, "readonly")).get("songs"))) || null;
    } catch (e) {}
    return keys.map((k, i) => [k, values[i]]);
  } catch (e) {
    console.warn("Rainbow: couldn't read saved files", e);
    return [];
  }
}

// (Byte arrays from Rust are views of its memory, so they're copied before any waiting.)
export function idb_put(key, bytes) {
  bytes = bytes.slice();
  store(FILES, "readwrite").then((s) => s.put(bytes, key)).catch((e) => console.warn("Rainbow: couldn't save", key, e));
}

export function idb_delete(key) {
  store(FILES, "readwrite").then((s) => s.delete(key)).catch(() => {});
}

// ── The songs folder ──

export function has_folder_api() {
  return typeof window.showDirectoryPicker === "function";
}

export function folder_name() {
  return folder ? folder.name : null;
}

// Asks for a folder; resolves to its name, or null if cancelled.
export async function pick_folder() {
  try {
    const handle = await window.showDirectoryPicker({ id: "songs", mode: "readwrite" });
    folder = handle;
    (await store(HANDLES, "readwrite")).put(handle, "songs");
    return handle.name;
  } catch (e) {
    return null;
  }
}

export function forget_folder() {
  folder = null;
  store(HANDLES, "readwrite").then((s) => s.delete("songs")).catch(() => {});
}

// Whether we may write to the folder, asking if needed (the browser only asks just after a click).
export async function folder_allowed() {
  if (!folder) return false;
  const opts = { mode: "readwrite" };
  if ((await folder.queryPermission(opts)) === "granted") return true;
  try {
    return (await folder.requestPermission(opts)) === "granted";
  } catch (e) {
    return false;
  }
}

async function dir_of(parts, create) {
  let dir = folder;
  for (const p of parts) dir = await dir.getDirectoryHandle(p, { create });
  return dir;
}

// Whether a file or folder exists at this path (a list of names) inside the songs folder.
export async function folder_has(parts) {
  try {
    const dir = await dir_of(parts.slice(0, -1), false);
    const name = parts[parts.length - 1];
    try {
      await dir.getFileHandle(name);
      return true;
    } catch (e) {
      await dir.getDirectoryHandle(name);
      return true;
    }
  } catch (e) {
    return false;
  }
}

// Writes a file (making folders as needed). With `append`, adds to the end of an existing file.
export async function folder_write(parts, bytes, append) {
  bytes = bytes.slice();
  const dir = await dir_of(parts.slice(0, -1), true);
  const file = await dir.getFileHandle(parts[parts.length - 1], { create: true });
  const out = await file.createWritable({ keepExistingData: append });
  if (append) await out.seek((await file.getFile()).size);
  await out.write(bytes);
  await out.close();
}

export async function folder_read(parts) {
  try {
    const dir = await dir_of(parts.slice(0, -1), false);
    const file = await (await dir.getFileHandle(parts[parts.length - 1])).getFile();
    return new Uint8Array(await file.arrayBuffer());
  } catch (e) {
    return null;
  }
}

// ── Other ──

// Saves bytes as an ordinary browser download.
export function save_file(name, bytes) {
  const url = URL.createObjectURL(new Blob([bytes]));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60000);
}

export function sleep(ms) {
  return new Promise((ok) => setTimeout(ok, ms));
}

export function loaded() {
  const el = document.getElementById("loading");
  if (el) el.remove();
}
