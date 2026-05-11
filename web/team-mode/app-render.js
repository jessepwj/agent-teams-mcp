function renderShell() {
  renderStaticText();
  if (typeof renderDashboardShell === "function") {
    renderDashboardShell();
  }
  renderEmptyOrErrorBanner();
  renderHeader();
  renderLeftPane();
  renderTimeline();
  renderComposer();
  renderDetail();
  bindDiagnosticsRetryButton();
  renderFooter();
}

/// Recipient candidates for the composer's @ dropdown.
///
/// Order: lead first (so it's the natural default), then alphabetized
/// active workers. Synthetic `user` is excluded — sending @user to yourself
/// is rejected by the server's self-mention guard, so showing it is
/// guaranteed-fail UX.
function composerRecipientCandidates() {
  const all = state.members || [];
  const lead = all.find((m) => m.kind === "lead");
  const workers = all
    .filter((m) => m.kind !== "lead")
    .filter((m) => m.name !== "user")
    .filter((m) => (m.status || "active") === "active")
    .sort((a, b) => a.name.localeCompare(b.name));
  return lead ? [lead, ...workers] : workers;
}

function renderComposer() {
  const form = document.getElementById("composerForm");
  if (!form) return;
  const select = document.getElementById("composerMention");
  const input = document.getElementById("composerInput");
  const button = document.getElementById("composerSend");
  if (!select || !input || !button) return;

  // Populate placeholder + button text on every render so language toggle
  // takes effect immediately.
  input.placeholder = t("composerPlaceholder");
  if (!state.composerSending) {
    button.textContent = t("composerSend");
    button.disabled = false;
  } else {
    button.textContent = t("composerSending");
    button.disabled = true;
  }

  const candidates = composerRecipientCandidates();
  const candidateNames = new Set(candidates.map((m) => m.name));
  const previous = (state.composerMentions || []).filter((name) => candidateNames.has(name));
  const chosen = previous.length ? previous : candidates[0] ? [candidates[0].name] : [];

  // Repopulate the <select> only when the membership list changed; otherwise
  // re-renders during typing would reset the focused option to the first
  // entry and surprise the user.
  const desired = candidates.map((m) => `${m.name}:${deriveWorkerStatusMeta(m).kind}`).join(",");
  if (select.dataset.populatedFor !== desired) {
    select.innerHTML = candidates
      .map(
        (m) =>
          `<option value="${escapeAttr(m.name)}">${escapeHtml(composerRecipientLabel(m))}</option>`,
      )
      .join("");
    select.dataset.populatedFor = desired;
  }
  if (chosen.length) {
    state.composerMentions = chosen;
    select.value = chosen[0];
    Array.from(select.options || []).forEach((option) => {
      option.selected = chosen.includes(option.value);
    });
  }

  form.style.display = state.teamId && candidates.length ? "" : "none";
}

function composerRecipientLabel(member) {
  if (member.kind === "lead") {
    return `${member.name} (${label("lead")})`;
  }
  const meta = deriveWorkerStatusMeta(member);
  return `${member.name} · ${meta.label}`;
}

async function submitComposer() {
  const input = document.getElementById("composerInput");
  const select = document.getElementById("composerMention");
  const status = document.getElementById("composerStatus");
  if (!input || !select || !status) return;

  if (!state.teamId) {
    setComposerStatus("error", t("composerNoTeam"));
    return;
  }
  const text = (input.value || "").trim();
  if (!text) {
    setComposerStatus("error", t("composerEmpty"));
    return;
  }
  const recipients = Array.from(select.selectedOptions || [])
    .map((option) => option.value)
    .filter(Boolean);
  if (!recipients.length && select.value) {
    recipients.push(select.value);
  }
  if (!recipients.length) {
    setComposerStatus("error", t("composerNoRecipient"));
    return;
  }

  // Recipients are added explicitly so the server doesn't have to scan the
  // body; user-typed @ mentions are also honored and deduped server-side.
  const prefix = recipients
    .filter((recipient) => !text.includes(`@${recipient}`))
    .map((recipient) => `@${recipient}`)
    .join(" ");
  const body = prefix ? `${prefix} ${text}` : text;

  state.composerSending = true;
  setComposerStatus("info", t("composerSending"));
  renderComposer();

  try {
    await apiPost(
      `/api/teams/${encodeURIComponent(state.teamId)}/rooms/main/messages`,
      { body, mentions: recipients },
    );
    input.value = "";
    setComposerStatus("success", `${t("composerSentTo")} ${recipients.map((name) => `@${name}`).join(", ")}`);
    state.timelineForceScrollBottom = true;
    // Refresh room messages so the just-sent message appears immediately.
    await loadTeam(state.teamId);
  } catch (err) {
    setComposerStatus("error", `${t("composerSendFailed")}${err.message || err}`);
  } finally {
    state.composerSending = false;
    renderComposer();
  }
}

function setComposerStatus(level, text) {
  const status = document.getElementById("composerStatus");
  if (!status) return;
  status.textContent = text;
  status.className = `composer-status ${level === "success" ? "success" : level === "error" ? "error" : "muted"}`;
}

function renderStaticText() {
  document.documentElement.lang = state.language === "zh" ? "zh-CN" : "en";
  document.title = t("brandKicker");
  applyColumnWidths();
  $("brandKicker").textContent = t("brandKicker");
  $("brandTitle").textContent = t("brandTitle");
  $("teamLabel").textContent = t("team");
  $("searchLabel").textContent = t("search");
  $("searchInput").placeholder = t("searchPlaceholder");
  $("languageToggleButton").textContent = t("switchLanguage");
  $("languageToggleButton").title = t("languageToggleTitle");
  $("reloadButton").title = t("reload");
  $("leftSplitter").title = t("resizeLeftPane");
  $("leftSplitter").setAttribute("aria-label", t("resizeLeftPane"));
  $("rightSplitter").title = t("resizeRightPane");
  $("rightSplitter").setAttribute("aria-label", t("resizeRightPane"));
  $("roomsTitle").textContent = t("rooms");
  $("membersTitle").textContent = t("members");
  $("filtersTitle").textContent = t("filters");
  $("clearFiltersButton").textContent = t("resetFilters");
  $("focusLeadButton").textContent = t("leadActivity");
  $("timelineTitle").textContent = t("timeline");
  $("detailPaneTitle").textContent = t("detailPane");
  $("sessionTabButton").textContent = t("sessionTab");
  $("detailTabButton").textContent = t("detailTab");
  $("diagnosticsTabButton").textContent = t("diagnosticsTab");
}

function renderHeader() {
  const team = activeTeam();
  $("teamSelect").value = state.teamId || "";
  $("timelineSubtitle").textContent = state.loadingTeam
    ? t("loadingTeamData")
    : team
      ? `${team.name} · ${state.members.length} ${t("members")} · ${messageCountLabel(allMessages().length)}`
      : state.loadingTeams
        ? t("loadingTeams")
        : t("noTeamsAvailable");

  const summary = [];
  if (state.senderFilter) summary.push(`${t("sender")}=${state.senderFilter}`);
  if (state.mentionFilter) summary.push(`${t("mentioned")}=${state.mentionFilter}`);
  if (state.search.trim()) summary.push(`${t("searchFilter")}=${state.search.trim()}`);
  $("filterSummary").textContent = summary.length ? summary.join(" · ") : t("noFilters");
  $("statusSummary").textContent = team ? `${t("readOnly")} · ${team.name}` : t("readOnly");

  const liveParts = [];
  if (state.loadingTeams || state.loadingTeam) liveParts.push(t("loading"));
  else liveParts.push(t("ready"));
  if (typeof dashboardTransportSummary === "function") liveParts.push(dashboardTransportSummary());
  if (state.teamError) liveParts.push(t("error"));
  if (state.refreshError && !state.loadingTeam) liveParts.push(t("refreshFailedFlag"));
  $("liveStatus").textContent = `${t("livePrefix")}：${liveParts.join(" · ")}`;
}

function renderLeftPane() {
  const roomList = $("roomList");
  if (!state.teamId) {
    roomList.innerHTML = `<div class="empty">${t("noTeams")}</div>`;
  } else {
    roomList.innerHTML = `<button class="list-button active" type="button">main</button>`;
  }

  const memberList = $("memberList");
  if (!state.teamId) {
    memberList.innerHTML = `<div class="empty">${t("noMembers")}</div>`;
  } else if (state.members.length === 0) {
    memberList.innerHTML = `<div class="empty">${t("noMembers")}</div>`;
  } else {
    memberList.innerHTML = membersForDisplay()
      .map((member) => {
        const active = member.name === state.selectedMemberName ? " active" : "";
        const hue = senderHue(member.name);
        return `
          <button class="list-button${active}" type="button" data-member="${escapeHtml(member.name)}" style="--sender-hue: ${hue}">
            <div class="message-head">
              ${renderSenderBadge(member.name, member.kind)}
              ${renderMemberStatus(member)}
            </div>
            <div class="subtle">${escapeHtml(label(member.roleLabel || ""))}</div>
          </button>
        `;
      })
      .join("");
  }

  document.querySelectorAll("[data-member]").forEach((button) => {
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      state.selectedMemberName = button.getAttribute("data-member");
      state.selectedMessageId = null;
      state.detailTab = "session";
      clearMemberDetailCache();
      state.detailError = "";
      setDeepLink("member", state.selectedMemberName);
      renderShell();
    });
  });
}

function renderTimeline() {
  const scrollSnapshot = captureTimelineScroll();
  const messages = filteredMessages();
  const totalMessages = allMessages().length;
  const hasActiveFilters = Boolean(state.senderFilter || state.mentionFilter || state.search.trim());
  $("timelineStats").innerHTML = hasActiveFilters
    ? `<span class="chat-count">${messages.length}/${messageCountLabel(totalMessages)}</span>`
    : `<span class="chat-count">${messageCountLabel(totalMessages)}</span>`;

  if (!state.teamId) {
    $("messageList").innerHTML = `<div class="empty">${t("noMessages")}</div>`;
    restoreTimelineScroll(scrollSnapshot);
    return;
  }

  if (state.loadingTeam) {
    $("messageList").innerHTML = `<div class="empty">${t("loadingMessages")}</div>`;
    restoreTimelineScroll(scrollSnapshot);
    return;
  }

  if (allMessages().length === 0) {
    $("messageList").innerHTML = `<div class="empty">${t("noMessages")}</div>`;
    restoreTimelineScroll(scrollSnapshot);
    return;
  }

  if (messages.length === 0) {
    $("messageList").innerHTML = `<div class="empty">${t("noMessagesMatch")}</div>`;
    restoreTimelineScroll(scrollSnapshot);
    return;
  }

  $("messageList").innerHTML = messages
    .map((message) => {
      const active = message.id === state.selectedMessageId ? " active" : "";
      if (isSystemChatMessage(message)) {
        return `
          <article class="chat-system${active}" data-message="${escapeHtml(message.id)}">
            <div class="chat-system-time">${fmtTime(message.createdAt)}</div>
            ${renderTimelineMessageBody(message, "chat-system-text")}
          </article>
        `;
      }
      const status = renderChatDeliveryStatus(message);
      const senderHueValue = senderHue(message.sender);
      const avatarKind =
        message.sender === "user"
          ? "user"
          : message.senderKind === "lead" || message.sender === "lead"
            ? "lead"
            : "worker";
      const mentionsUser =
        message.sender !== "user" &&
        Array.isArray(message.mentions) &&
        message.mentions.includes("user");
      let toUserClass = "";
      if (mentionsUser) {
        toUserClass = " chat-message-to-user";
        if (!state.mentionPulsedIds.has(message.id)) {
          toUserClass += " chat-message-pulse";
          state.mentionPulsedIds.add(message.id);
        }
      }
      const toYouBadge = mentionsUser
        ? `<span class="chat-meta-to-you" title="${escapeAttr(t("toYouTooltip"))}">${escapeHtml(t("toYouBadge"))}</span>`
        : "";
      return `
        <article class="chat-message${active}${toUserClass}" data-message="${escapeHtml(message.id)}" style="--sender-hue: ${senderHueValue}">
          <button class="chat-avatar ${avatarKind} sender-tinted" type="button" data-sender="${escapeHtml(message.sender)}" aria-label="${escapeHtml(message.sender)}" style="--sender-hue: ${senderHueValue}">
            ${escapeHtml(avatarInitials(message.sender))}
          </button>
          <div class="chat-content">
            <div class="chat-meta">
              <button class="link-button chat-name" type="button" data-sender="${escapeHtml(message.sender)}" title="${escapeHtml(message.sender)}" style="color: hsl(${senderHueValue}, 80%, 75%)">${escapeHtml(message.sender)}</button>
              <span class="chat-time">${fmtTime(message.createdAt)}</span>
              ${toYouBadge}
              ${status}
            </div>
            ${renderTimelineMessageBody(message, "chat-bubble", { forceFull: mentionsUser })}
          </div>
        </article>
      `;
    })
    .join("");

  document.querySelectorAll("[data-message]").forEach((row) => {
    row.addEventListener("click", () => {
      state.selectedMessageId = row.getAttribute("data-message");
      state.selectedMemberName = null;
      state.detailTab = "detail";
      state.detailError = "";
      setDeepLink("message", state.selectedMessageId);
      renderShell();
    });
  });

  document.querySelectorAll("[data-mention]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      state.mentionFilter = button.getAttribute("data-mention");
      renderShell();
    });
  });

  document.querySelectorAll("[data-sender]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      state.senderFilter = button.getAttribute("data-sender");
      state.mentionFilter = "";
      renderShell();
    });
  });

  document.querySelectorAll("[data-message-expand]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      const id = button.getAttribute("data-message-expand");
      if (!id) return;
      if (state.timelineExpandedMessages.has(id)) {
        state.timelineExpandedMessages.delete(id);
      } else {
        state.timelineExpandedMessages.add(id);
      }
      renderTimeline();
    });
  });

  restoreTimelineScroll(scrollSnapshot);
}

function membersForDisplay() {
  return [...(state.members || [])].sort((a, b) => {
    const aLead = a.kind === "lead" || a.name === "lead";
    const bLead = b.kind === "lead" || b.name === "lead";
    if (aLead !== bLead) return aLead ? -1 : 1;
    return String(a.name || "").localeCompare(String(b.name || ""));
  });
}

function renderTimelineMessageBody(message, className, opts = {}) {
  const fullText = message.body || "";
  const previewText = message.bodyPreview || fullText;
  const hasFullText = fullText && fullText !== previewText;
  const forceFull = Boolean(opts.forceFull);
  const expanded = forceFull || state.timelineExpandedMessages.has(message.id);
  const visibleText = hasFullText && expanded ? fullText : previewText;
  const toggle = hasFullText && !forceFull
    ? `<button class="chat-expand-button" type="button" data-message-expand="${escapeHtml(message.id)}">${expanded ? t("hideFullMessage") : t("showFullMessage")}</button>`
    : "";
  return `
    <div class="${className}">${renderChatText(visibleText || "")}</div>
    ${toggle}
  `;
}


function renderDetail() {
  renderDetailTabButtons();
  if (state.detailError) {
    $("detailTitle").textContent = t("loadError");
    $("detailBody").innerHTML = `
      <div class="empty">${escapeHtml(state.detailError)}</div>
      <button class="chip retry-button" type="button" id="retryDetailButton">${t("retry")}</button>
    `;
    const retryButton = $("retryDetailButton");
    if (retryButton) {
      retryButton.addEventListener("click", async () => {
        state.detailError = "";
        await retryDetail();
      });
    }
    return;
  }
  if (state.detailTab === "session") {
    renderSessionPanel();
    return;
  }
  if (state.detailTab === "diagnostics") {
    renderDiagnosticsPanel();
    return;
  }

  const message = allMessages().find((item) => item.id === state.selectedMessageId);
  if (message) {
    renderMessageDetail(message);
    return;
  }
  if (state.selectedMessageId) {
    $("detailTitle").textContent = t("messageNotFound");
    $("detailBody").innerHTML = `
      <div class="empty">${
        state.loadingTeam
          ? t("loadingSelectedMessage")
          : `${t("message")} ${escapeHtml(state.selectedMessageId)} ${t("messageMissing")}`
      }</div>
    `;
    return;
  }
  if (state.selectedMemberName) {
    renderMemberDetail(state.selectedMemberName);
    return;
  }

  const team = activeTeam();
  if (team) {
    $("detailTitle").textContent = team.name;
    $("detailBody").innerHTML = `
      <div class="detail-card">
        <div class="detail-head">
          <span class="badge lead">${t("teamLabel")}</span>
          <span class="pill">${escapeHtml(label(team.status))}</span>
        </div>
        <div class="subtle">${escapeHtml(team.cwd || "")}</div>
      </div>
    `;
    return;
  }

  $("detailTitle").textContent = t("noSelection");
  $("detailBody").innerHTML = `<div class="empty">${state.teamError ? escapeHtml(state.teamError) : t("pickMessageOrMember")}</div>`;
}

function renderDetailTabButtons() {
  const sessionButton = $("sessionTabButton");
  const detailButton = $("detailTabButton");
  const diagnosticsButton = $("diagnosticsTabButton");
  if (!sessionButton || !detailButton || !diagnosticsButton) {
    return;
  }
  sessionButton.classList.toggle("active", state.detailTab === "session");
  detailButton.classList.toggle("active", state.detailTab === "detail");
  diagnosticsButton.classList.toggle("active", state.detailTab === "diagnostics");
}

function renderSessionPanel() {
  if (state.selectedMemberName) {
    renderMemberConversation(state.selectedMemberName);
    return;
  }
  const lead = leadMemberName() || state.members[0]?.name || null;
  if (lead) {
    state.selectedMemberName = lead;
    state.selectedMessageId = null;
    renderMemberConversation(lead);
    return;
  }
  $("detailTitle").textContent = t("processSession");
  $("detailBody").innerHTML = `<div class="empty">${t("pickMessageOrMember")}</div>`;
}

function renderDiagnosticsPanel() {
  const diagnosticsMarkup = renderDiagnosticsSections();
  $("detailTitle").textContent = t("teamDiagnostics");
  $("detailBody").innerHTML = state.teamId
    ? diagnosticsMarkup || `<div class="empty">${t("noDiagnosticsLoaded")}</div>`
    : `<div class="empty">${t("noTeamForDiagnostics")}</div>`;
  bindDiagnosticsRetryButton();
}

async function renderMessageDetail(message) {
  const thread = allMessages().filter((item) => item.threadId === message.threadId);
  const rootMessage = thread.find((item) => item.replyTo === null) || thread[0] || null;
  $("detailTitle").textContent = `${t("message")} ${message.id}`;
  $("detailBody").innerHTML = `
    <div class="detail-card">
      <div class="detail-head">
        ${renderSenderBadge(message.sender, message.senderKind)}
        <span class="pill">${escapeHtml(label(message.kind))}</span>
        <span class="pill">${escapeHtml(label(message.deliveryStatus))}</span>
      </div>
      <div class="detail-meta">
        <span class="pill">${fmtTime(message.createdAt)}</span>
        <span class="pill">${t("read")} ${message.readCount || 0}</span>
        <span class="pill">${t("acked")} ${message.ackedCount || 0}</span>
        <span class="pill">${t("thread")} ${message.threadReplyCount || 0}</span>
      </div>
      <div class="message-body">${escapeHtml(message.body || "")}</div>
    </div>
    <div class="detail-card">
      <div class="section-title">${t("routing")}</div>
      <div class="detail-grid">
        <div><span class="muted">${t("sender")}</span><div>${escapeHtml(message.sender)}</div></div>
        <div><span class="muted">${t("kind")}</span><div>${escapeHtml(label(message.kind))}</div></div>
        <div><span class="muted">${t("delivery")}</span><div>${escapeHtml(label(message.deliveryStatus))}</div></div>
        <div><span class="muted">${t("replyTo")}</span><div>${escapeHtml(message.replyTo || t("none"))}</div></div>
        <div><span class="muted">${t("threadId")}</span><div>${escapeHtml(message.threadId || t("none"))}</div></div>
      </div>
      <div class="detail-list-block">
        <div class="muted">${t("mentions")}</div>
        <div class="detail-pills">${
          (message.mentions || []).length
            ? (message.mentions || []).map((mention) => `<span class="pill">@${escapeHtml(mention)}</span>`).join("")
            : `<span class="empty-inline">${t("none")}</span>`
        }</div>
        <div class="muted">${t("effectiveRecipients")}</div>
        <div class="detail-pills">${
          (message.effectiveRecipients || []).length
            ? (message.effectiveRecipients || []).map((recipient) => `<span class="pill">${escapeHtml(recipient)}</span>`).join("")
            : `<span class="empty-inline">${t("none")}</span>`
        }</div>
      </div>
    </div>
    <div class="detail-card">
      <div class="section-title">${t("threadMessages")}</div>
      ${
        thread.length
          ? thread
              .map(
                (item) => `
                  <div class="detail-item">
                    <div class="detail-head">
                      ${renderSenderBadge(item.sender, item.senderKind)}
                      <span class="pill">${fmtTime(item.createdAt)}</span>
                    </div>
                    <div class="message-body">${escapeHtml(item.body || item.bodyPreview || "")}</div>
                  </div>
                `,
              )
              .join("")
          : `<div class="empty">${t("noMessages")}</div>`
      }
    </div>
    <details class="detail-card">
      <summary>${t("rawJson")}</summary>
      <pre class="code-block">${escapeHtml(JSON.stringify(message, null, 2))}</pre>
    </details>
  `;

  if (rootMessage) {
    const rootSummary = $("detailBody").querySelector(".detail-card");
    if (rootSummary) {
      const foot = document.createElement("div");
      foot.className = "subtle";
      foot.textContent = `${t("threadRoot")}：${rootMessage.id}`;
      rootSummary.appendChild(foot);
    }
  }
}

function renderMemberDetail(name) {
  const teamId = state.teamId;
  if (!teamId || !name) return;

  const key = memberDetailKey(teamId, name);
  if (state.memberDetailKey === key && state.memberDetail && state.memberActivity) {
    renderMemberDetailContent(name, state.memberDetail, state.memberActivity);
    return;
  }

  $("detailTitle").textContent = isLeadName(name) ? t("leadActivity") : `${t("member")} ${name}`;
  $("detailBody").innerHTML = `
    <div class="empty">${t("loadingMemberDetail")}</div>
  `;
  refreshMemberDetail(name);
}


function renderMemberDetailContent(name, data, activity) {
  const profile = data.profile || {};
  const execution = data.execution || {};
  const summary = data.activity || {};
  const activityItems = Array.isArray(activity.items) ? activity.items : [];
  const envKeys = Array.isArray(execution.envKeys) ? execution.envKeys : [];
  const redactedEnv = execution.redactedEnv || {};
  const isLead = profile.kind === "lead" || isLeadName(name);
  const currentMember = state.members.find((member) => member.name === name);
  const statusMember = currentMember || {
    name: profile.name || name,
    kind: profile.kind,
    status: profile.status,
    sessionState: execution.sessionState,
    lastActivityAt: summary.lastActivityAt,
  };
  $("detailTitle").textContent = isLead ? t("leadActivity") : `${t("member")} ${name}`;
  $("detailBody").innerHTML = `
      <div class="detail-card">
        <div class="detail-head">
          ${renderSenderBadge(profile.name || name, profile.kind)}
          <span class="badge ${isLead ? "lead" : "worker"}">${escapeHtml(label(profile.kind || "member"))}</span>
          ${renderMemberStatus(statusMember)}
          <span class="pill">${escapeHtml(label(profile.roleLabel || ""))}</span>
        </div>
        <div class="subtle">${escapeHtml(profile.name || name)} · ${fmtTime(profile.joinedAt)}</div>
      </div>
      <div class="detail-card">
        <div class="section-title">${isLead ? t("leadCoordination") : t("profile")}</div>
        <div class="detail-grid">
          <div><span class="muted">${t("name")}</span><div>${escapeHtml(profile.name || name)}</div></div>
          <div><span class="muted">${t("kind")}</span><div>${escapeHtml(label(profile.kind || "member"))}</div></div>
          <div><span class="muted">${t("role")}</span><div>${escapeHtml(label(profile.roleLabel || ""))}</div></div>
          <div><span class="muted">${t("status")}</span><div>${escapeHtml(label(profile.status || "unknown"))}</div></div>
        </div>
      </div>
      <div class="detail-card">
        <div class="section-title">${t("executionSnapshot")}</div>
        <div class="detail-grid">
          <div><span class="muted">${t("executionMode")}</span><div>${escapeHtml(label(execution.executionMode || "unknown"))}</div></div>
          <div><span class="muted">${t("sessionState")}</span><div>${escapeHtml(label(execution.sessionState || "unknown"))}</div></div>
          <div><span class="muted">${t("adapter")}</span><div>${escapeHtml(label(execution.adapter || "n/a"))}</div></div>
          <div><span class="muted">${t("model")}</span><div>${escapeHtml(label(execution.model || "n/a"))}</div></div>
          <div><span class="muted">${t("cwd")}</span><div>${escapeHtml(execution.cwd || na())}</div></div>
          <div><span class="muted">${t("systemPrompt")}</span><div>${execution.hasSystemPrompt ? t("folded") : t("none")}</div></div>
        </div>
        <div class="detail-list-block">
          <div class="muted">${t("environment")}</div>
          <div class="detail-pills">${
            envKeys.length
              ? envKeys.map((key) => `<span class="pill">${escapeHtml(key)}=${escapeHtml(redactedEnv[key] ?? na())}</span>`).join("")
              : `<span class="empty-inline">${t("none")}</span>`
          }</div>
        </div>
      </div>
      <div class="detail-card">
        <div class="section-title">${isLead ? t("leadCoordination") : t("recentActivity")}</div>
        <div class="detail-list-block">
          <div class="detail-pills">
            <span class="pill">${summary.sentCount || 0} ${t("sent")}</span>
            <span class="pill">${summary.receivedCount || 0} ${t("received")}</span>
            <span class="pill">${summary.mentionedCount || 0} ${t("mentioned")}</span>
          </div>
          ${
            activityItems.length
              ? activityItems
                  .slice(0, 8)
                  .map(
                    (item) => `
                      <div class="detail-item">
                        <div class="detail-head">
                          <span class="badge worker">${escapeHtml(label(item.itemType))}</span>
                          <span class="pill">${fmtTime(item.createdAt)}</span>
                        </div>
                        <div class="message-body">${escapeHtml(localText(item.summary))}</div>
                      </div>
                    `,
                  )
                  .join("")
              : `<div class="empty">${t("noMessages")}</div>`
          }
        </div>
      </div>
      <details class="detail-card">
        <summary>${t("rawJson")}</summary>
        <pre class="code-block">${escapeHtml(JSON.stringify(data, null, 2))}</pre>
      </details>
    `;
}

function isLeadName(name) {
  return leadMemberName() === name || state.members.find((member) => member.name === name)?.kind === "lead";
}

async function retryDetail() {
  if (state.failedTeamId) {
    const teamId = state.failedTeamId;
    state.failedTeamId = null;
    state.detailError = "";
    state.teamError = "";
    state.refreshError = "";
    await loadTeam(teamId);
    return;
  }
  if (state.selectedMessageId) {
    const message = allMessages().find((item) => item.id === state.selectedMessageId);
    if (message) {
      renderMessageDetail(message);
      return;
    }
  }
  if (state.selectedMemberName) {
    await refreshMemberDetail(state.selectedMemberName, { force: true });
  }
}

function renderFooter() {
  const messages = allMessages();
  const visible = filteredMessages().length;
  const statusBits = [`${visible}/${messages.length} ${t("messagesVisible")}`];
  if (state.teamError) statusBits.push(t("teamLoadFailed"));
  if (state.refreshError) statusBits.push(state.refreshError);
  $("countsSummary").textContent = statusBits.join(" · ");
  const revision = state.bundleRevision || document.querySelector('meta[name="bundle-revision"]')?.getAttribute("content") || "";
  const bundleSummary = $("bundleRevisionSummary");
  if (bundleSummary) {
    bundleSummary.textContent = revision ? `Bundle ${revision.slice(0, 8)}` : "Bundle unknown";
    bundleSummary.title = revision ? `Bundle revision ${revision}` : "Bundle revision unavailable";
  }
}

function renderEmptyOrErrorBanner() {
  const banner = $("banner");
  if (!banner) return;
  if (state.teamError) {
    banner.className = "banner error";
    banner.textContent = state.teamError;
    banner.hidden = false;
    return;
  }
  if (!state.teamId && !state.loadingTeams) {
    banner.className = "banner empty";
    banner.textContent = t("noTeams");
    banner.hidden = false;
    return;
  }
  banner.hidden = true;
}

Object.assign(globalThis, {
  renderShell,
  composerRecipientCandidates,
  composerRecipientLabel,
  renderComposer,
  submitComposer,
  setComposerStatus,
  renderStaticText,
  renderHeader,
  renderLeftPane,
  renderTimeline,
  renderDetail,
  renderDetailTabButtons,
  renderSessionPanel,
  renderDiagnosticsPanel,
  renderMessageDetail,
  renderMemberDetail,
  renderMemberDetailContent,
  isLeadName,
  retryDetail,
  renderFooter,
  renderEmptyOrErrorBanner,
});
