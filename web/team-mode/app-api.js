// BUG-7 fix: a single web port may serve multiple projects (one per CC).
// The team_create response embeds the owning project_root in the URL as
// ?project=<urlencoded>. Read it once on page load and propagate it on
// every API request so the server's multi-project router can pick the
// right `.agent-teams/` data dir. Without this, all fetches go to the
// startup-time default project and a CC in another project sees empty
// data.
function readWebProjectRoot() {
  try {
    const params = new URLSearchParams(globalThis.location?.search || "");
    const project = params.get("project");
    return project && project.length > 0 ? project : null;
  } catch (_) {
    return null;
  }
}

const WEB_PROJECT_ROOT = readWebProjectRoot();

function withProjectQuery(path) {
  if (!WEB_PROJECT_ROOT) {
    return path;
  }
  const encoded = encodeURIComponent(WEB_PROJECT_ROOT);
  const separator = path.includes("?") ? "&" : "?";
  return `${path}${separator}project=${encoded}`;
}

async function api(path) {
  const response = await fetch(withProjectQuery(path), {
    headers: { Accept: "application/json" },
  });
  if (!response.ok) {
    const err = new Error(`${response.status} ${response.statusText}`);
    err.status = response.status;
    throw err;
  }
  return response.json();
}

async function apiPost(path, payload) {
  const response = await fetch(withProjectQuery(path), {
    method: "POST",
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      Accept: "application/json",
    },
    body: JSON.stringify(payload),
  });
  // Always try to parse JSON — both success (Created) and structured error
  // bodies come back as JSON. If parsing fails fall back to status text.
  let parsed = null;
  try {
    parsed = await response.json();
  } catch (_) {
    parsed = null;
  }
  if (!response.ok) {
    const detail = parsed && parsed.error ? parsed.error : `${response.status} ${response.statusText}`;
    throw new Error(detail);
  }
  return parsed;
}

Object.assign(globalThis, {
  api,
  apiPost,
  // Exposed so app-conversation / SSE callers can build their EventSource
  // URLs with the same project query — fetch() goes through api()/apiPost()
  // already, but EventSource doesn't share that wrapper.
  withProjectQuery,
  WEB_PROJECT_ROOT,
});
