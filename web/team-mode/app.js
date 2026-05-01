// app.js — wire-up + bootstrap. The other app-*.js files are loaded by
// index.html as plain scripts in dependency order; they share global
// scope so cross-file function references work without ES module imports.

async function loadTeams() {
  state.loadingTeams = true;
  state.teamError = "";
  state.refreshError = "";
  state.diagnostics = null;
  state.diagnosticsError = "";
  state.diagnosticsLoading = false;
  renderShell();
  try {
    const data = await api("/api/teams");
    state.teams = data.teams || [];
    const deepLinkTeam = (state.deepLink || parseDeepLink()).team;
    const nextTeamId = resolveSelectedTeamId(state.teams, state.teamId || deepLinkTeam);
    if (nextTeamId !== state.teamId) {
      state.selectedMessageId = null;
      state.selectedMemberName = null;
      clearMemberDetailCache();
      state.detailError = "";
      state.refreshError = "";
    }
    state.teamId = nextTeamId;
    renderTeamSelect();
    if (state.teamId) {
      await loadTeam(state.teamId);
    } else {
      state.room = null;
      state.members = [];
      state.teamDetail = null;
      state.failedTeamId = null;
      state.selectedMessageId = null;
      state.selectedMemberName = null;
      clearMemberDetailCache();
      state.refreshError = "";
      state.diagnostics = null;
      state.diagnosticsError = "";
      state.diagnosticsLoading = false;
      if (typeof closeTeamEvents === "function") {
        closeTeamEvents();
      }
      renderShell();
    }
  } catch (error) {
    state.teamError = localizedError("failedLoadTeams", error);
    state.teams = [];
    state.teamId = null;
    state.room = null;
    state.members = [];
    state.teamDetail = null;
    state.failedTeamId = null;
    state.selectedMessageId = null;
    state.selectedMemberName = null;
    clearMemberDetailCache();
    state.refreshError = "";
    state.diagnostics = null;
    state.diagnosticsError = "";
    state.diagnosticsLoading = false;
    if (typeof closeTeamEvents === "function") {
      closeTeamEvents();
    }
    renderShell();
  } finally {
    state.loadingTeams = false;
    renderShell();
  }
}

function resolveSelectedTeamId(teams, requestedTeamId) {
  if (requestedTeamId && teams.some((team) => team.id === requestedTeamId)) {
    return requestedTeamId;
  }
  return teams[0]?.id || null;
}

function applyTeamDeepLink(teamId) {
  state.teamId = teamId || null;
  clearDeepLink();
}

function renderTeamSelect() {
  const select = $("teamSelect");
  select.innerHTML = state.teams.length
    ? state.teams
        .map((team) => `<option value="${escapeHtml(team.id)}">${escapeHtml(team.name)}</option>`)
        .join("")
    : `<option value="">${t("noTeams")}</option>`;
  select.value = state.teamId || "";
}

async function loadTeam(teamId) {
  const shouldOpenAtLatest = state.teamId !== teamId || !state.room;
  state.loadingTeam = true;
  state.teamError = "";
  state.detailError = "";
  state.refreshError = "";
  state.diagnostics = null;
  state.diagnosticsError = "";
  state.diagnosticsLoading = true;
  renderShell();
  try {
    const [teamDetail, room, members] = await Promise.all([
      api(`/api/teams/${encodeURIComponent(teamId)}`),
      api(`/api/teams/${encodeURIComponent(teamId)}/rooms/main?limit=200`),
      api(`/api/teams/${encodeURIComponent(teamId)}/members`),
    ]);
    state.teamId = teamId;
    state.teamDetail = teamDetail;
    state.room = room;
    state.members = members.members || [];
    if (shouldOpenAtLatest) {
      state.timelineForceScrollBottom = true;
    }
    state.failedTeamId = null;
    applyDeepLink();
    if (!state.selectedMessageId && !state.selectedMemberName) {
      state.selectedMemberName = leadMemberName() || state.members[0]?.name || null;
      state.detailTab = "session";
    }
    renderShell();
    if (typeof openTeamEvents === "function") {
      await openTeamEvents(teamId);
    }
    await loadDiagnostics(teamId);
  } catch (error) {
    state.teamError = localizedError("failedLoadTeamData", error);
    state.refreshError = localizedError("refreshFailed", error);
    state.room = null;
    state.members = [];
    state.teamDetail = null;
    state.failedTeamId = teamId;
    state.detailError = localizedError("failedLoadTeamDetail", error);
    state.selectedMessageId = null;
    state.selectedMemberName = null;
    state.diagnosticsLoading = false;
    if (typeof closeTeamEvents === "function") {
      closeTeamEvents();
    }
    renderShell();
  } finally {
    state.loadingTeam = false;
    renderShell();
  }
}

async function loadDiagnostics(teamId) {
  try {
    const data = await api(`/api/teams/${encodeURIComponent(teamId)}/diagnostics`);
    if (state.teamId !== teamId) {
      return;
    }
    state.diagnostics = data;
    state.diagnosticsError = "";
  } catch (error) {
    if (state.teamId !== teamId) {
      return;
    }
    state.diagnostics = null;
    state.diagnosticsError = localizedError("diagnosticsUnavailable", error);
  } finally {
    if (state.teamId === teamId) {
      state.diagnosticsLoading = false;
      renderShell();
    }
  }
}

function applyDeepLink() {
  const deepLink = state.deepLink || parseDeepLink();
  if (deepLink.message) {
    state.selectedMessageId = deepLink.message;
    state.selectedMemberName = null;
    state.detailTab = "detail";
    clearMemberDetailCache();
  } else if (deepLink.member) {
    state.selectedMemberName = deepLink.member;
    state.selectedMessageId = null;
    state.detailTab = "session";
  }
}

function clearMemberDetailCache() {
  state.memberDetailKey = "";
  state.memberDetailLoadingKey = "";
  state.memberDetail = null;
  state.memberActivity = null;
  state.memberConversationKey = "";
  state.memberConversationLoadingKey = "";
  state.memberConversationScrollKey = "";
  state.memberConversation = null;
}

function memberDetailKey(teamId, name) {
  return `${teamId || ""}:${name || ""}`;
}

let consecutiveRefreshFailures = 0;

async function refreshCurrentTeam() {
  if (state.loadingTeams || state.loadingTeam || !state.teamId) {
    return;
  }
  const teamId = state.teamId;
  try {
    const [teamDetail, room, members] = await Promise.all([
      api(`/api/teams/${encodeURIComponent(teamId)}`),
      api(`/api/teams/${encodeURIComponent(teamId)}/rooms/main?limit=200`),
      api(`/api/teams/${encodeURIComponent(teamId)}/members`),
    ]);
    if (state.teamId !== teamId) {
      return;
    }
    state.teamDetail = teamDetail;
    state.room = room;
    state.members = members.members || [];
    if (typeof reconcileDashboardWorkersFromMembers === "function") {
      reconcileDashboardWorkersFromMembers(teamId, state.members);
    }
    state.refreshError = "";
    consecutiveRefreshFailures = 0;
    if (state.selectedMemberName && !state.members.some((member) => member.name === state.selectedMemberName)) {
      state.selectedMemberName = leadMemberName() || state.members[0]?.name || null;
      state.selectedMessageId = null;
      clearMemberDetailCache();
      setDeepLink("member", state.selectedMemberName);
    }
    renderShell();
    if (state.selectedMemberName) {
      if (state.detailTab === "session") {
        refreshMemberConversation(state.selectedMemberName, { force: true });
      } else {
        refreshMemberDetail(state.selectedMemberName, { force: true });
      }
    }
  } catch (error) {
    if (state.teamId !== teamId) {
      return;
    }
    consecutiveRefreshFailures += 1;
    // Team was deleted server-side — drop teamId immediately and reload the
    // team list so the user lands on the next live team (or the empty
    // state). Avoids 404-spam while the team is gone.
    if (error && error.status === 404) {
      state.teamId = null;
      state.teamDetail = null;
      state.members = [];
      state.room = null;
      state.selectedMemberName = null;
      state.selectedMessageId = null;
      clearMemberDetailCache();
      if (typeof closeTeamEvents === "function") {
        closeTeamEvents();
      }
      setDeepLink("team", null);
      consecutiveRefreshFailures = 0;
      loadTeams();
      return;
    }
    // Network error / daemon offline: after 3 consecutive failures, reset
    // to the team picker so the next render shows a clean "no teams"
    // state instead of a frozen stale UI silently retrying.
    if (consecutiveRefreshFailures >= 3) {
      state.teamId = null;
      state.teamDetail = null;
      state.members = [];
      state.room = null;
      state.selectedMemberName = null;
      state.selectedMessageId = null;
      clearMemberDetailCache();
      setDeepLink("team", null);
      consecutiveRefreshFailures = 0;
      loadTeams();
      return;
    }
    state.refreshError = localizedError("refreshFailed", error);
    renderFooter();
    renderHeader();
  }
}

function startAutoRefresh() {
  if (refreshTimer) {
    return;
  }
  const intervalFn = window.setInterval || globalThis.setInterval;
  if (typeof intervalFn !== "function") {
    return;
  }
  refreshTimer = intervalFn(refreshCurrentTeam, REFRESH_INTERVAL_MS);
}

function allMessages() {
  return state.room?.messages || [];
}

function filteredMessages() {
  const search = state.search.trim().toLowerCase();
  return allMessages().filter((message) => {
    if (state.senderFilter && message.sender !== state.senderFilter) return false;
    if (state.mentionFilter) {
      const mentions = message.mentions || [];
      const recipients = message.effectiveRecipients || [];
      if (!mentions.includes(state.mentionFilter) && !recipients.includes(state.mentionFilter)) {
        return false;
      }
    }
    if (!search) return true;
    return [
      message.sender,
      message.kind,
      message.body,
      ...(message.mentions || []),
      ...(message.effectiveRecipients || []),
    ]
      .join(" ")
      .toLowerCase()
      .includes(search);
  });
}

function bindColumnResizer(splitterId, side) {
  const splitter = $(splitterId);
  if (!splitter) {
    return;
  }

  let dragState = null;

  const move = (event) => {
    if (!dragState) {
      return;
    }
    const delta = event.clientX - dragState.startX;
    const nextWidth =
      side === "left" ? dragState.startWidth + delta : dragState.startWidth - delta;
    state.columnWidths[side] = clamp(
      Math.round(nextWidth),
      COLUMN_LIMITS[side].min,
      COLUMN_LIMITS[side].max,
    );
    applyColumnWidths();
  };

  const stop = () => {
    if (!dragState) {
      return;
    }
    dragState = null;
    splitter.classList?.remove("dragging");
    saveColumnWidths();
    window.removeEventListener?.("pointermove", move);
    window.removeEventListener?.("pointerup", stop);
    window.removeEventListener?.("pointercancel", stop);
  };

  splitter.addEventListener("pointerdown", (event) => {
    if (window.matchMedia?.("(max-width: 820px)").matches) {
      return;
    }
    event.preventDefault?.();
    dragState = {
      startX: event.clientX,
      startWidth: resolvedPaneWidth(side),
    };
    splitter.classList?.add("dragging");
    splitter.setPointerCapture?.(event.pointerId);
    window.addEventListener?.("pointermove", move);
    window.addEventListener?.("pointerup", stop);
    window.addEventListener?.("pointercancel", stop);
  });

  splitter.addEventListener("keydown", (event) => {
    if (!["ArrowLeft", "ArrowRight"].includes(event.key)) {
      return;
    }
    event.preventDefault?.();
    const direction = event.key === "ArrowRight" ? 1 : -1;
    const delta = side === "left" ? direction * 16 : direction * -16;
    state.columnWidths[side] = clamp(
      resolvedPaneWidth(side) + delta,
      COLUMN_LIMITS[side].min,
      COLUMN_LIMITS[side].max,
    );
    applyColumnWidths();
    saveColumnWidths();
  });
}

function bindEvents() {
  bindColumnResizer("leftSplitter", "left");
  bindColumnResizer("rightSplitter", "right");
  bindConversationEvents();
  if (typeof bindDashboardEvents === "function") {
    bindDashboardEvents();
  }

  window.addEventListener?.("resize", () => {
    if (rightPaneUsesBalancedWidth()) {
      applyColumnWidths();
    }
  });

  $("teamSelect").addEventListener("change", (event) => {
    applyTeamDeepLink(event.target.value || null);
    state.selectedMessageId = null;
    state.selectedMemberName = null;
    state.detailTab = "session";
    clearMemberDetailCache();
    if (state.teamId) {
      loadTeam(state.teamId);
    } else {
      state.room = null;
      state.members = [];
      renderShell();
    }
  });

  $("searchInput").addEventListener("input", (event) => {
    state.search = event.target.value;
    renderShell();
  });

  $("languageToggleButton").addEventListener("click", () => {
    state.language = state.language === "zh" ? "en" : "zh";
    renderShell();
  });

  $("detailTabButton").addEventListener("click", () => {
    state.detailTab = "detail";
    renderShell();
  });

  $("sessionTabButton").addEventListener("click", async () => {
    state.detailTab = "session";
    if (!state.selectedMemberName) {
      state.selectedMemberName = leadMemberName() || state.members[0]?.name || null;
      state.selectedMessageId = null;
      if (state.selectedMemberName) {
        setDeepLink("member", state.selectedMemberName);
      }
    }
    renderShell();
    if (state.selectedMemberName) {
      await refreshMemberConversation(state.selectedMemberName);
    }
  });

  $("diagnosticsTabButton").addEventListener("click", async () => {
    state.detailTab = "diagnostics";
    renderShell();
    if (state.teamId && !state.diagnostics && !state.diagnosticsLoading && !state.diagnosticsError) {
      state.diagnosticsLoading = true;
      renderShell();
      await loadDiagnostics(state.teamId);
    }
  });

  $("reloadButton").addEventListener("click", () => {
    if (state.teamId) {
      loadTeam(state.teamId);
    } else {
      loadTeams();
    }
  });

  $("clearFiltersButton").addEventListener("click", () => {
    state.search = "";
    state.senderFilter = "";
    state.mentionFilter = "";
    $("searchInput").value = "";
    renderShell();
  });

  $("focusLeadButton").addEventListener("click", async () => {
    const lead = leadMemberName();
    if (lead) {
      state.selectedMessageId = null;
      state.selectedMemberName = lead;
      state.detailTab = "session";
      clearMemberDetailCache();
      state.detailError = "";
      setDeepLink("member", lead);
      renderShell();
    }
  });

  window.addEventListener("hashchange", () => {
    state.deepLink = parseDeepLink();
    if (state.deepLink.team && state.deepLink.team !== state.teamId) {
      loadTeam(state.deepLink.team);
      return;
    }
    if (state.teamId) {
      applyDeepLink();
      renderShell();
    }
  });

  const composerForm = document.getElementById("composerForm");
  if (composerForm) {
    composerForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitComposer();
    });
  }
  const composerInput = document.getElementById("composerInput");
  if (composerInput) {
    // Enter sends; Shift+Enter inserts a newline.
    composerInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        submitComposer();
      }
    });
  }
  const composerSelect = document.getElementById("composerMention");
  if (composerSelect) {
    composerSelect.addEventListener("change", (event) => {
      state.composerMentions = Array.from(event.target.selectedOptions || [])
        .map((option) => option.value)
        .filter(Boolean);
      if (!state.composerMentions.length && event.target.value) {
        state.composerMentions = [event.target.value];
      }
    });
  }
}

Object.assign(globalThis, {
  loadTeams,
  loadTeam,
  loadDiagnostics,
  refreshCurrentTeam,
  startAutoRefresh,
  allMessages,
  filteredMessages,
  bindEvents,
});

window.addEventListener("DOMContentLoaded", async () => {
  bindEvents();
  const deepLink = parseDeepLink();
  state.deepLink = deepLink.team || deepLink.message || deepLink.member ? deepLink : null;
  renderShell();
  await loadTeams();
  startAutoRefresh();
});
