function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/// Stable HSL hue (0..360) derived from a sender name.
///
/// We use a simple djb2-style hash so the same name always lands on the
/// same hue across sessions. Special-casing keeps the two highest-signal
/// senders visually distinct regardless of name:
///   * `lead`  -> 188 (cyan, matches the brand accent so the eye still
///                 reads it as "the coordinator")
///   * `user`  -> 28  (warm orange — the human input shouldn't blend into
///                 the wash of AI workers)
/// All other names hash freely. The pre-skipped slice of hue space (the
/// orange/cyan bands) stays large enough that random workers still cover
/// blue/green/purple/red without clashing with the reserved colors.
function senderHue(name) {
  if (!name) return 200;
  if (name === "lead") return 188;
  if (name === "user") return 28;
  let hash = 5381;
  for (let i = 0; i < name.length; i += 1) {
    hash = ((hash << 5) + hash + name.charCodeAt(i)) & 0xffffffff;
  }
  // Skip the 14..50 (orange) and 175..205 (cyan) bands so workers don't
  // accidentally collide with `user` / `lead`. We have a comfortable
  // ~290° of remaining hue real estate, which is plenty for normal team sizes.
  const usable = 360 - 36 - 30; // 294 degrees
  let bucket = Math.abs(hash) % usable;
  if (bucket >= 14) bucket += 36; // jump past 14..50
  if (bucket >= 175) bucket += 30; // jump past 175..205
  return bucket;
}

/// Render a senderName as the colored pill used everywhere a member's
/// name appears (timeline rows, detail panels, member list). Caller is
/// responsible for HTML-escaping `senderName` if it ever flows from
/// untrusted input — names are validated to a strict slug, so within
/// this codebase they're safe to interpolate.
function renderSenderBadge(senderName, senderKind, extraClass = "") {
  const hue = senderHue(senderName);
  const kindClass =
    senderName === "user"
      ? "is-user"
      : senderKind === "lead" || senderName === "lead"
        ? "is-lead"
        : "";
  const cls = ["sender-badge", kindClass, extraClass].filter(Boolean).join(" ");
  return `<span class="${cls}" style="--sender-hue: ${hue}">${escapeHtml(senderName)}</span>`;
}

function escapeAttr(value) {
  return escapeHtml(value);
}

function fmtTime(value) {
  if (!value) return na();
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function parseDeepLink() {
  const hash = window.location.hash.replace(/^#/, "");
  const params = new URLSearchParams(hash);
  return {
    team: params.get("team") || "",
    message: params.get("message") || "",
    member: params.get("member") || "",
  };
}

function setDeepLink(type, value) {
  const params = new URLSearchParams();
  if (state.teamId) {
    params.set("team", state.teamId);
  }
  if (!type || !value) {
    const nextHash = params.toString();
    window.location.hash = nextHash ? nextHash : "";
    state.deepLink = parseDeepLink();
    return;
  }

  params.set(type, value);
  state.deepLink = {
    team: state.teamId || "",
    message: type === "message" ? value : "",
    member: type === "member" ? value : "",
  };
  window.location.hash = params.toString();
}

function clearDeepLink() {
  if (!state.teamId) {
    window.location.hash = "";
    state.deepLink = null;
    return;
  }
  const params = new URLSearchParams();
  params.set("team", state.teamId);
  window.location.hash = params.toString();
  state.deepLink = parseDeepLink();
}

function activeTeam() {
  return (
    state.teams.find((team) => team.id === state.teamId) ||
    (state.teamDetail?.team?.id === state.teamId ? state.teamDetail.team : null)
  );
}

function leadMemberName() {
  const team = activeTeam();
  if (!team) return null;
  const lead = state.members.find((member) => member.kind === "lead");
  return lead?.name || team.leadMemberId || null;
}

function captureTimelineScroll() {
  const list = $("messageList");
  if (!list) {
    return { list: null, nearBottom: true, top: 0 };
  }
  const distanceToBottom = list.scrollHeight - list.scrollTop - list.clientHeight;
  const nearBottom =
    !Number.isFinite(distanceToBottom) || distanceToBottom <= TIMELINE_BOTTOM_THRESHOLD_PX;
  return {
    list,
    nearBottom,
    top: list.scrollTop || 0,
  };
}

function restoreTimelineScroll(snapshot) {
  const list = snapshot?.list || $("messageList");
  if (!list) {
    return;
  }
  const shouldStickToBottom = Boolean(state.timelineForceScrollBottom || snapshot?.nearBottom);
  const shouldClearForce = !state.loadingTeam;
  nextFrame(() => {
    if (shouldStickToBottom) {
      list.scrollTop = list.scrollHeight;
    } else {
      list.scrollTop = snapshot?.top || 0;
    }
    if (shouldClearForce) {
      state.timelineForceScrollBottom = false;
    }
  });
}

function isSystemChatMessage(message) {
  return message.kind === "status" || String(message.body || "").startsWith("[SYSTEM]");
}

function avatarInitials(name) {
  const trimmed = String(name || "?").trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/[\s._-]+/).filter(Boolean);
  if (parts.length >= 2) {
    return `${parts[0][0] || ""}${parts[1][0] || ""}`.toUpperCase();
  }
  return trimmed.slice(0, 2).toUpperCase();
}

/// Map a raw `sessionState` (from the read_model) to a visual presentation:
/// tone class controls colour, anim controls dot motion, labelKey is the
/// raw label that flows through the i18n `label()` lookup. The lead member
/// is the only state where "static" is appropriate even though the process
/// is healthy — because lead is not a managed worker, "running" semantics
/// don't apply and the cyan tone is the brand accent.
function memberStatusMeta(sessionState) {
  const raw = String(sessionState || "unknown").toLowerCase();
  switch (raw) {
    case "coordinator":
      return { tone: "lead", anim: "static", raw };
    case "running":
      return { tone: "good", anim: "pulse", raw };
    case "starting":
      return { tone: "warn", anim: "spin", raw };
    case "dead":
    case "failed":
      return { tone: "bad", anim: "static", raw };
    case "stopped":
    case "paused":
      return { tone: "muted", anim: "static", raw };
    case "not_spawned":
    case "not-spawned":
      return { tone: "muted", anim: "static", raw: "not_spawned" };
    default:
      return { tone: "muted", anim: "static", raw: "unknown" };
  }
}

function renderMemberStatus(sessionState) {
  const meta = memberStatusMeta(sessionState);
  const text = label(meta.raw);
  return `<span class="status-pill status-${meta.tone}" title="${escapeAttr(text)}">
    <span class="status-dot status-dot-${meta.anim}" aria-hidden="true"></span>
    <span class="status-text">${escapeHtml(text)}</span>
  </span>`;
}

function renderChatDeliveryStatus(message) {
  const status = message.deliveryStatus;
  if (!["failed", "expired", "partial"].includes(status)) {
    return "";
  }
  const className = status === "partial" ? "warn" : "fail";
  return `<span class="chat-status ${className}">${escapeHtml(label(status))}</span>`;
}

function renderChatText(text) {
  const escaped = escapeHtml(text || "");
  return escaped.replace(
    /@([\p{L}\p{N}_.-]+)/gu,
    '<button class="link-button mention-token" type="button" data-mention="$1">@$1</button>',
  );
}

Object.assign(globalThis, {
  escapeHtml,
  senderHue,
  renderSenderBadge,
  escapeAttr,
  fmtTime,
  parseDeepLink,
  setDeepLink,
  clearDeepLink,
  activeTeam,
  leadMemberName,
  captureTimelineScroll,
  restoreTimelineScroll,
  isSystemChatMessage,
  avatarInitials,
  memberStatusMeta,
  renderMemberStatus,
  renderChatDeliveryStatus,
  renderChatText,
});
