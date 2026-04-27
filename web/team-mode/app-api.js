async function api(path) {
  const response = await fetch(path, { headers: { Accept: "application/json" } });
  if (!response.ok) {
    const err = new Error(`${response.status} ${response.statusText}`);
    err.status = response.status;
    throw err;
  }
  return response.json();
}

async function apiPost(path, payload) {
  const response = await fetch(path, {
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
});
