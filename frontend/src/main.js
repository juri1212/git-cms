import "./style.css";

const tokenKey = "git-cms-token";
const sessionKey = "git-cms-session";
const signedOut = document.querySelector("#signed-out");
const editor = document.querySelector("#editor");
const user = document.querySelector("#user");
const form = document.querySelector("#save-form");
const filePicker = document.querySelector("#file-picker");
const filename = document.querySelector("#filename");
const content = document.querySelector("#content");
const save = document.querySelector("#save");
const status = document.querySelector("#status");
let fileLoadRequest = 0;

function setStatus(message, isError = false) {
  status.textContent = message;
  status.classList.toggle("error", isError);
}

function getToken() {
  return localStorage.getItem(tokenKey);
}

function apiPath(path) {
  return path.split("/").map(encodeURIComponent).join("/");
}

async function api(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    headers: {
      Authorization: `Bearer ${getToken()}`,
      "Content-Type": "application/json",
      ...options.headers,
    },
  });

  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error?.message || body.message || `Request failed (${response.status})`);
  }
  return response.json();
}

function saveTokenFromCallback() {
  const url = new URL(window.location.href);
  const token = url.searchParams.get("token");
  if (!token) return;

  localStorage.setItem(tokenKey, token);
  url.searchParams.delete("token");
  window.history.replaceState({}, "", url);
}

async function showEditor() {
  const me = await api("/api/me");
  user.textContent = `Signed in as ${me.login} · ${me.repository}`;
  signedOut.hidden = true;
  editor.hidden = false;
  await loadAvailableFiles();
}

async function listFiles(path = "") {
  const query = path ? `?path=${encodeURIComponent(path)}` : "";
  const result = await api(`/api/files${query}`);
  const entries = await Promise.all(
    result.entries.map(async (entry) => {
      if (entry.kind === "directory") return listFiles(entry.path);
      return [entry];
    }),
  );
  return entries.flat();
}

async function loadAvailableFiles() {
  filePicker.disabled = true;
  filePicker.innerHTML = '<option value="">Loading available files…</option>';
  try {
    const files = await listFiles("content");
    files.sort((a, b) => a.path.localeCompare(b.path));
    filePicker.innerHTML = '<option value="">Select a file…</option>';
    for (const file of files) {
      const option = document.createElement("option");
      option.value = file.path;
      option.textContent = file.path;
      filePicker.append(option);
    }
    filePicker.disabled = files.length === 0;
    if (files.length === 0) setStatus("No editable files are available.");
  } catch (error) {
    filePicker.innerHTML = '<option value="">Unable to load files</option>';
    setStatus(`Could not load available files: ${error.message}`, true);
  }
}

filePicker.addEventListener("change", async () => {
  const path = filePicker.value;
  if (!path) return;

  const request = ++fileLoadRequest;
  filePicker.disabled = true;
  setStatus(`Loading ${path}…`);
  try {
    const file = await api(`/api/files/${apiPath(path)}`);
    if (request !== fileLoadRequest) return;
    filename.value = file.path;
    content.value = file.content;
    setStatus(`Loaded ${file.path}.`);
  } catch (error) {
    if (request === fileLoadRequest) setStatus(`Could not load ${path}: ${error.message}`, true);
  } finally {
    if (request === fileLoadRequest) filePicker.disabled = false;
  }
});

async function getSession(path, initialContent) {
  const existing = sessionStorage.getItem(sessionKey);
  if (existing) return { session: JSON.parse(existing), created: false };

  const session = await api("/api/sessions", {
    method: "POST",
    body: JSON.stringify({
      title: `Edit ${path}`,
      initial_file: {
        path,
        content: initialContent,
        message: `Create ${path}`,
      },
    }),
  });
  sessionStorage.setItem(sessionKey, JSON.stringify(session));
  return { session, created: true };
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const path = filename.value.trim();
  if (!path.startsWith("content/")) {
    setStatus("Select a file from content/ before saving.", true);
    return;
  }

  save.disabled = true;
  setStatus("Saving…");
  try {
    const { session, created } = await getSession(path, content.value);
    const result = created
      ? { ...session, pull_request_number: session.pull_request.number }
      : await api(`/api/sessions/${session.session_id}/files/${apiPath(path)}`, {
          method: "PUT",
          body: JSON.stringify({
            content: content.value,
            message: `Update ${path}`,
          }),
        });
    setStatus(`Saved to draft PR #${result.pull_request_number}. Commit ${result.commit.slice(0, 7)}.`);
  } catch (error) {
    setStatus(error.message, true);
  } finally {
    save.disabled = false;
  }
});

document.querySelector("#sign-out").addEventListener("click", () => {
  localStorage.removeItem(tokenKey);
  sessionStorage.removeItem(sessionKey);
  window.location.assign("/");
});

saveTokenFromCallback();
if (getToken()) {
  showEditor().catch((error) => {
    localStorage.removeItem(tokenKey);
    setStatus(`Sign-in expired: ${error.message}`, true);
  });
}
