function renderMemberConversation(name) {
  const teamId = state.teamId;
  if (!teamId || !name) return;

  const key = memberDetailKey(teamId, name);
  $("detailTitle").textContent = `${t("processSession")} · ${name}`;

  if (state.memberConversationKey === key && state.memberConversation) {
    renderMemberConversationContent(name, state.memberConversation, {
      forceBottom: state.memberConversationScrollKey !== key,
    });
    return;
  }

  $("detailBody").innerHTML = `<div class="empty">${t("loadingConversation")}</div>`;
  refreshMemberConversation(name);
}

async function refreshMemberConversation(name, { force = false } = {}) {
  const teamId = state.teamId;
  if (!teamId || !name) return;

  const key = memberDetailKey(teamId, name);
  if (state.memberConversationLoadingKey === key) {
    return;
  }
  if (!force && state.memberConversationKey === key && state.memberConversation) {
    return;
  }

  state.memberConversationLoadingKey = key;
  try {
    const data = await api(
      `/api/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(name)}/conversation`,
    );
    if (state.teamId !== teamId || state.selectedMemberName !== name) {
      return;
    }
    state.memberConversationKey = key;
    state.memberConversation = data;
    state.memberConversationLoadingKey = "";
    if (state.detailTab === "session") {
      renderMemberConversationContent(name, data, {
        forceBottom: state.memberConversationScrollKey !== key,
      });
    }
  } catch (error) {
    if (state.teamId !== teamId || state.selectedMemberName !== name) {
      return;
    }
    state.memberConversationLoadingKey = "";
    state.refreshError = localizedError("refreshFailed", error);
    if (state.detailTab === "session" && !(state.memberConversationKey === key && state.memberConversation)) {
      $("detailTitle").textContent = `${t("processSession")} · ${name}`;
      $("detailBody").innerHTML = `<div class="empty">${escapeHtml(localizedError("failedLoadMemberDetail", error))}</div>`;
    }
  }
}

function renderMemberConversationContent(name, data, { forceBottom = false } = {}) {
  const key = memberDetailKey(state.teamId, name);
  $("detailTitle").textContent = `${t("processSession")} · ${name}`;
  const source = data.source || {};
  const sourceMeta = renderConversationSource(source);

  const limitations = (data.limitations || []).length
    ? `<details class="conversation-disclosure"><summary>${t("limitations")}</summary><div class="detail-pills">${(data.limitations || [])
        .map((item) => `<span class="pill">${escapeHtml(localText(item))}</span>`)
        .join("")}</div></details>`
    : "";

  const items = Array.isArray(data.items) ? data.items : [];
  const conversation = items.length
    ? renderConversationTranscript(items)
    : `<div class="empty">${t("noConversation")}</div>`;

  preserveDetailScroll(() => {
    $("detailBody").innerHTML = `${sourceMeta}${conversation}${limitations}`;
  }, { forceBottom });
  if (forceBottom) {
    nextFrame(() => {
      state.memberConversationScrollKey = key;
    });
  } else {
    state.memberConversationScrollKey = key;
  }
}

function renderConversationSource(source) {
  return `
    <details class="conversation-source" open>
      <summary>
        <span>${t("conversationSource")}</span>
        <span class="conversation-source-summary">${escapeHtml(source.sessionId || na())}</span>
      </summary>
      <div class="conversation-source-grid">
        <div><span>${t("model")}</span><strong>${escapeHtml(label(source.provider || "n/a"))}</strong></div>
        <div><span>${t("matchedBy")}</span><strong>${escapeHtml(label(source.confidence || "unknown"))}</strong></div>
        <div><span>${t("latestModified")}</span><strong>${fmtTime(source.updatedAt)}</strong></div>
      </div>
      <div class="conversation-source-path">${escapeHtml(source.cwd || na())}</div>
      <div class="conversation-source-path">${escapeHtml(source.path || na())}</div>
    </details>
  `;
}

function renderConversationTranscript(items) {
  const renderItems = preprocessConversationItems(items);
  const groups = groupConversationItemsIntoTurns(renderItems);
  return `
    <div class="conversation-list">
      ${groups.map(renderConversationGroup).join("")}
    </div>
  `;
}

function preprocessConversationItems(items) {
  const renderItems = [];
  const pendingTools = new Map();
  const setupItems = [];

  const flushSetup = () => {
    if (!setupItems.length) return;
    if (setupItems.length > 1 || renderItems.length === 0) {
      renderItems.push({
        type: "session_setup",
        id: `setup-${setupItems[0].id}`,
        prompts: setupItems.splice(0),
      });
    } else {
      renderItems.push(setupItems.shift());
    }
    setupItems.length = 0;
  };

  for (const item of items) {
    const kind = item.kind || "text";
    const role = item.role || "unknown";
    const id = item.id || `${renderItems.length}`;
    const text = item.text || "";

    if (role === "user" && kind === "text") {
      const userItem = { type: "user_prompt", id, content: text, timestamp: item.timestamp };
      if (isSessionSetupText(text)) {
        setupItems.push(userItem);
      } else {
        flushSetup();
        renderItems.push(userItem);
      }
      continue;
    }

    flushSetup();

    if (kind === "thinking") {
      renderItems.push({
        type: "thinking",
        id,
        thinking: text,
        timestamp: item.timestamp,
      });
      continue;
    }

    if (kind === "tool_use") {
      const toolId = item.toolUseId || id;
      const toolName = item.toolName || item.title || "tool";
      const toolInput = item.input ?? parseMaybeJson(text);
      const toolItem = {
        type: "tool_call",
        id: toolId,
        sourceId: id,
        toolName,
        toolInput,
        toolResult: null,
        status: "pending",
        timestamp: item.timestamp,
      };
      pendingTools.set(toolId, toolItem);
      renderItems.push(toolItem);
      continue;
    }

    if (kind === "tool_result") {
      const toolId = item.toolUseId || "";
      const result = {
        content: text,
        structured: item.result,
        isError: item.isError || role === "error",
        timestamp: item.timestamp,
      };
      const toolItem = pendingTools.get(toolId);
      if (toolItem) {
        toolItem.toolResult = result;
        toolItem.status = result.isError ? "error" : "complete";
        pendingTools.delete(toolId);
      } else {
        renderItems.push({
          type: "tool_call",
          id: toolId || id,
          sourceId: id,
          toolName: item.toolName || item.title || "tool",
          toolInput: null,
          toolResult: result,
          status: result.isError ? "error" : "complete",
          timestamp: item.timestamp,
        });
      }
      continue;
    }

    if (role === "system" || role === "error" || kind === "error") {
      renderItems.push({
        type: "system",
        id,
        subtype: role === "error" || kind === "error" ? "error" : kind,
        content: text || item.title || label(kind),
        timestamp: item.timestamp,
      });
      continue;
    }

    if (text.trim()) {
      renderItems.push({
        type: "text",
        id,
        text,
        timestamp: item.timestamp,
      });
    }
  }

  flushSetup();
  return renderItems;
}

function groupConversationItemsIntoTurns(items) {
  const groups = [];
  let currentTurn = null;
  let turnIndex = 0;

  const flushTurn = () => {
    if (currentTurn && (currentTurn.prompt || currentTurn.items.length)) {
      groups.push(currentTurn);
    }
    currentTurn = null;
  };

  for (const item of items) {
    if (item.type === "session_setup") {
      flushTurn();
      groups.push({ type: "session_setup", items: [item] });
      continue;
    }

    if (item.type === "user_prompt") {
      flushTurn();
      turnIndex += 1;
      currentTurn = {
        type: "work_turn",
        index: turnIndex,
        prompt: item,
        items: [],
      };
    } else {
      if (!currentTurn) {
        turnIndex += 1;
        currentTurn = {
          type: "work_turn",
          index: turnIndex,
          prompt: null,
          items: [],
        };
      }
      currentTurn.items.push(item);
    }
  }
  flushTurn();
  return groups;
}

function renderConversationGroup(group) {
  if (group.type === "session_setup") {
    return group.items.map(renderConversationRenderItem).join("");
  }
  if (group.type === "work_turn") {
    return renderWorkTurn(group);
  }
  const first = group.items?.[0] || {};
  return `
    <div class="assistant-turn" data-turn-id="${escapeHtml(first.id || "")}">
      ${group.items.map(renderConversationRenderItem).join("")}
    </div>
  `;
}

function renderWorkTurn(group) {
  const finalText = findFinalTextItem(group.items);
  const stepCount = group.items.filter((item) => item.type !== "text").length;
  const promptKind = classifyConversationPrompt(group.prompt);
  return `
    <section class="conversation-work-turn" data-turn-id="${escapeHtml(group.prompt?.id || group.items[0]?.id || "")}">
      <div class="work-turn-header">
        <div>
          <div class="work-turn-title">${t("workTurn")} ${group.index}</div>
          <div class="work-turn-subtitle">
            <span>${promptKind.label}</span>
            <span>${stepCount} ${t("executionSteps")}</span>
            <span>${finalText ? t("finalReply") : t("noFinalReply")}</span>
          </div>
        </div>
        ${group.prompt?.timestamp ? `<span class="conversation-timestamp">${fmtTime(group.prompt.timestamp)}</span>` : ""}
      </div>
      ${
        group.prompt
          ? renderUserPromptItem(group.prompt, {
              label: promptKind.label,
              variant: promptKind.variant,
            })
          : ""
      }
      <div class="work-turn-steps assistant-turn">
        ${
          group.items.length
            ? group.items
                .map((item) =>
                  renderConversationRenderItem(item, {
                    isFinalReply: Boolean(finalText && item.id === finalText.id),
                  }),
                )
                .join("")
            : `<div class="empty-inline">${t("noFinalReply")}</div>`
        }
      </div>
    </section>
  `;
}

function findFinalTextItem(items) {
  let hasToolAfter = false;
  for (const item of [...items].reverse()) {
    if (item.type === "tool_call") {
      hasToolAfter = true;
      continue;
    }
    if (item.type === "text" && String(item.text || "").trim() && !hasToolAfter) {
      return item;
    }
  }
  return null;
}

function classifyConversationPrompt(item) {
  const text = String(item?.content || "");
  const isHook =
    /\bhook\b/i.test(text) ||
    /<system-reminder/i.test(text) ||
    /lead[_ -]?pending/i.test(text) ||
    /\binjected\b/i.test(text) ||
    /\[SYSTEM\]/i.test(text);
  return {
    label: isHook ? t("hookInput") : t("receivedInput"),
    variant: isHook ? "hook" : "received",
  };
}

function renderConversationRenderItem(item, options = {}) {
  switch (item.type) {
    case "user_prompt":
      return renderUserPromptItem(item, options);
    case "session_setup":
      return renderSessionSetupItem(item);
    case "thinking":
      return renderThinkingItem(item);
    case "tool_call":
      return renderToolCallItem(item);
    case "system":
      return renderSystemItem(item);
    case "text":
    default:
      return renderTextItem(item, options);
  }
}

function renderUserPromptItem(item, options = {}) {
  const label = options.label || t("receivedInput");
  const variant = options.variant || "received";
  return `
    <div class="conversation-user-prompt ${variant === "hook" ? "conversation-user-prompt-hook" : ""}" data-render-id="${escapeHtml(item.id)}">
      <div class="conversation-step-label">${escapeHtml(label)}</div>
      <div class="message-user-prompt">${renderMarkdownText(item.content || "")}</div>
    </div>
  `;
}

function renderSessionSetupItem(item) {
  return `
    <details class="session-setup-block">
      <summary>${t("sessionSetup")} · ${item.prompts.length}</summary>
      <div class="session-setup-content">
        ${item.prompts
          .map((prompt) => `<div class="message-user-prompt">${renderMarkdownText(prompt.content || "")}</div>`)
          .join("")}
      </div>
    </details>
  `;
}

function renderTextItem(item, options = {}) {
  const isFinalReply = Boolean(options.isFinalReply);
  return `
    <div class="text-block timeline-item ${isFinalReply ? "final-reply-block" : ""}" data-render-id="${escapeHtml(item.id)}">
      ${isFinalReply ? `<div class="conversation-step-label final-reply-label">${t("finalReply")}</div>` : ""}
      <button type="button" class="text-block-copy" data-copy-text="${escapeAttr(item.text || "")}" title="${t("copy")}" aria-label="${t("copy")}">⧉</button>
      ${renderMarkdownText(item.text || "")}
      ${item.timestamp ? `<div class="conversation-timestamp">${fmtTime(item.timestamp)}</div>` : ""}
    </div>
  `;
}

function renderThinkingItem(item) {
  return `
    <details class="thinking-block">
      <summary><span class="thinking-icon">▸</span>${t("thinking")}</summary>
      <div class="thinking-content">${renderMarkdownText(item.thinking || "")}</div>
    </details>
  `;
}

function renderSystemItem(item) {
  return `
    <div class="system-message ${item.subtype === "error" ? "system-message-error" : ""}">
      <span class="system-message-icon">${item.subtype === "error" ? "!" : "⟳"}</span>
      <span class="system-message-text">${escapeHtml(item.content || "")}</span>
    </div>
  `;
}

function renderToolCallItem(item) {
  const hasCollapsedPreview = toolHasCollapsedPreview(item);
  const expandable = true;
  const expandedByDefault = !hasCollapsedPreview && ["Edit", "TodoWrite"].includes(item.toolName);
  const summary = getToolSummary(item);
  const statusLabel = toolStatusLabel(item.status);
  const preview = hasCollapsedPreview ? renderToolCollapsedPreview(item) : "";
  return `
    <div class="tool-row timeline-item ${expandedByDefault ? "expanded" : "collapsed"} status-${escapeHtml(item.status)}" data-tool-row>
      <div class="tool-row-header" role="button" tabindex="0" title="${t("showDetails")}">
        ${item.status === "pending" ? `<span class="tool-spinner" aria-label="${t("running")}"></span>` : ""}
        ${item.status === "aborted" ? `<span class="tool-aborted-icon">×</span>` : ""}
        <span class="tool-name">${escapeHtml(toolDisplayName(item.toolName))}</span>
        <span class="tool-summary" title="${escapeHtml(summary)}">${escapeHtml(summary)}${item.status === "aborted" ? ` <span class="tool-aborted-label">(${t("interrupted")})</span>` : ""}</span>
        <span class="tool-status">${escapeHtml(statusLabel)}</span>
        <span class="expand-chevron" aria-hidden="true">${expandedByDefault ? "▾" : "▸"}</span>
      </div>
      ${preview}
      <div class="tool-row-content">${renderToolExpandedContent(item)}</div>
    </div>
  `;
}

function renderToolExpandedContent(item) {
  if (item.status === "pending" || item.status === "aborted") {
    return renderToolUseContent(item.toolName, item.toolInput);
  }
  return renderToolResultContent(item.toolName, item.toolResult);
}

function renderToolUseContent(toolName, input) {
  if (toolName === "Bash" && input && typeof input === "object") {
    const command = input.command || input.cmd || "";
    if (command) {
      return `<pre class="code-block"><code>${escapeHtml(command)}</code></pre>`;
    }
  }
  return `<pre class="code-block"><code>${escapeHtml(prettyValue(input))}</code></pre>`;
}

function renderToolResultContent(toolName, result) {
  if (!result) {
    return `<div class="tool-no-result">${t("none")}</div>`;
  }
  const content = result.structured ?? result.content ?? "";
  const className = result.isError ? "code-block code-block-error" : "code-block";
  return `<pre class="${className}"><code>${escapeHtml(prettyValue(content))}</code></pre>`;
}

function renderToolCollapsedPreview(item) {
  const resultText = item.toolResult ? prettyValue(item.toolResult.structured ?? item.toolResult.content ?? "") : "";
  const preview = resultText
    .split("\n")
    .filter(Boolean)
    .slice(0, 6)
    .join("\n");
  if (!preview) return "";
  return `<pre class="tool-row-collapsed-preview"><code>${escapeHtml(preview)}</code></pre>`;
}

function toolHasCollapsedPreview(item) {
  return item.status !== "pending" && Boolean(item.toolResult?.content) && ["Bash", "Read", "Grep", "Glob"].includes(item.toolName);
}

function getToolSummary(item) {
  const inputSummary = getToolInputSummary(item.toolName, item.toolInput);
  if (item.status === "pending" || item.status === "aborted") return inputSummary;
  if (item.status === "error") return `${inputSummary} → ${t("failed")}`;
  const lineCount = String(item.toolResult?.content || "").split("\n").filter(Boolean).length;
  if (["Read", "Bash"].includes(item.toolName) && lineCount) return `${inputSummary} → ${lineCount} lines`;
  if (["Glob"].includes(item.toolName) && lineCount) return `${inputSummary} → ${lineCount} files`;
  if (["Grep"].includes(item.toolName) && lineCount) return `${inputSummary} → ${lineCount} matches`;
  return `${inputSummary} → ${t("complete")}`;
}

function getToolInputSummary(toolName, input) {
  const obj = input && typeof input === "object" ? input : {};
  if (["Read", "Write", "Edit"].includes(toolName) && obj.file_path) return fileName(obj.file_path);
  if (toolName === "Bash" && (obj.command || obj.cmd)) return String(obj.command || obj.cmd);
  if (["Glob", "Grep"].includes(toolName) && obj.pattern) return String(obj.pattern);
  if (["Task", "Agent"].includes(toolName) && obj.description) return truncate(String(obj.description), 48);
  if (toolName === "WebSearch" && obj.query) return truncate(String(obj.query), 48);
  if (toolName === "WebFetch" && obj.url) return truncate(String(obj.url), 60);
  const first = Object.values(obj).find((value) => typeof value === "string" && value.trim());
  return first ? truncate(String(first), 60) : "...";
}

function toolStatusLabel(status) {
  if (status === "pending") return t("running");
  if (status === "error") return t("failed");
  if (status === "aborted") return t("interrupted");
  return t("complete");
}

function toolDisplayName(name) {
  return name || "Tool";
}

function isSessionSetupText(text) {
  const trimmed = String(text || "").trimStart();
  return trimmed.startsWith("# AGENTS.md instructions") || trimmed.startsWith("<environment_context>");
}

function parseMaybeJson(text) {
  if (!text || typeof text !== "string") return null;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function prettyValue(value) {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function renderMarkdownText(text) {
  const raw = String(text || "");
  if (!raw.trim()) return `<p>${t("none")}</p>`;
  const parts = [];
  const regex = /```([A-Za-z0-9_-]*)\n?([\s\S]*?)```/g;
  let lastIndex = 0;
  let match;
  while ((match = regex.exec(raw))) {
    if (match.index > lastIndex) {
      parts.push(renderMarkdownParagraphs(raw.slice(lastIndex, match.index)));
    }
    parts.push(`<pre class="code-block"><code>${escapeHtml(match[2] || "")}</code></pre>`);
    lastIndex = regex.lastIndex;
  }
  if (lastIndex < raw.length) {
    parts.push(renderMarkdownParagraphs(raw.slice(lastIndex)));
  }
  return parts.join("");
}

function renderMarkdownParagraphs(text) {
  return String(text)
    .split(/\n{2,}/)
    .map((paragraph) => paragraph.trim())
    .filter(Boolean)
    .map((paragraph) => `<p>${renderInlineMarkdown(paragraph).replace(/\n/g, "<br>")}</p>`)
    .join("");
}

function renderInlineMarkdown(text) {
  return String(text)
    .split(/(`[^`]+`)/g)
    .map((part) => {
      if (part.startsWith("`") && part.endsWith("`") && part.length > 1) {
        return `<code>${escapeHtml(part.slice(1, -1))}</code>`;
      }
      return escapeHtml(part);
    })
    .join("");
}

function preserveDetailScroll(renderFn, { forceBottom = false } = {}) {
  const pane = $("detailPane");
  const previousTop = pane?.scrollTop || 0;
  const previousHeight = pane?.scrollHeight || 0;
  const nearBottom = pane ? pane.scrollHeight - pane.scrollTop - pane.clientHeight < 120 : true;
  renderFn();
  if (!pane) return;
  nextFrame(() => {
    if (forceBottom || nearBottom) {
      pane.scrollTop = pane.scrollHeight;
    } else {
      pane.scrollTop = previousTop + Math.max(0, pane.scrollHeight - previousHeight);
    }
  });
}

function nextFrame(callback) {
  const raf = window.requestAnimationFrame || globalThis.requestAnimationFrame;
  if (typeof raf === "function") {
    raf(callback);
    return;
  }
  const timeout = window.setTimeout || globalThis.setTimeout;
  timeout(callback, 0);
}

function fileName(path) {
  return String(path || "").split(/[\\/]/).pop() || String(path || "");
}

function truncate(text, max) {
  const value = String(text || "");
  return value.length <= max ? value : `${value.slice(0, Math.max(0, max - 3))}...`;
}

async function refreshMemberDetail(name, { force = false } = {}) {
  const teamId = state.teamId;
  if (!teamId || !name) return;

  const key = memberDetailKey(teamId, name);
  if (state.memberDetailLoadingKey === key) {
    return;
  }
  if (!force && state.memberDetailKey === key && state.memberDetail && state.memberActivity) {
    return;
  }

  const requestSeq = (state.detailRequestSeq += 1);
  state.memberDetailLoadingKey = key;

  try {
    const [data, activity] = await Promise.all([
      api(`/api/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(name)}`),
      api(`/api/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(name)}/activity`),
    ]);
    if (
      requestSeq !== state.detailRequestSeq ||
      state.teamId !== teamId ||
      state.selectedMemberName !== name
    ) {
      return;
    }
    state.memberDetailKey = key;
    state.memberDetail = data;
    state.memberActivity = activity;
    state.memberDetailLoadingKey = "";
    state.detailError = "";
    renderMemberDetailContent(name, data, activity);
  } catch (error) {
    if (state.teamId !== teamId || state.selectedMemberName !== name) {
      return;
    }
    state.memberDetailLoadingKey = "";
    if (force && state.memberDetailKey === key && state.memberDetail && state.memberActivity) {
      state.refreshError = localizedError("refreshFailed", error);
      renderHeader();
      renderFooter();
      return;
    }
    state.detailError = localizedError("failedLoadMemberDetail", error);
    state.refreshError = localizedError("refreshFailed", error);
    $("detailTitle").textContent = isLeadName(name) ? t("leadActivity") : `${t("member")} ${name}`;
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
  }
}


function bindConversationEvents() {
  $("detailBody").addEventListener("click", async (event) => {
    const copyButton = event.target.closest?.(".text-block-copy");
    if (copyButton) {
      const text = copyButton.dataset.copyText || "";
      try {
        await navigator.clipboard?.writeText(text);
        copyButton.textContent = "✓";
        copyButton.classList.add("copied");
        copyButton.setAttribute("title", t("copied"));
        window.setTimeout(() => {
          copyButton.textContent = "⧉";
          copyButton.classList.remove("copied");
          copyButton.setAttribute("title", t("copy"));
        }, 1200);
      } catch {
        copyButton.textContent = "!";
      }
      return;
    }

    const header = event.target.closest?.(".tool-row-header");
    if (!header || header.classList.contains("non-expandable")) {
      return;
    }
    toggleToolRow(header.closest(".tool-row"));
  });

  $("detailBody").addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") {
      return;
    }
    const header = event.target.closest?.(".tool-row-header");
    if (!header || header.classList.contains("non-expandable")) {
      return;
    }
    event.preventDefault();
    toggleToolRow(header.closest(".tool-row"));
  });
}

function toggleToolRow(row) {
  if (!row) return;
  const expanded = row.classList.toggle("expanded");
  row.classList.toggle("collapsed", !expanded);
  const chevron = row.querySelector(".expand-chevron");
  if (chevron) {
    chevron.textContent = expanded ? "▾" : "▸";
  }
}

Object.assign(globalThis, {
  renderMemberConversation,
  refreshMemberConversation,
  renderMemberConversationContent,
  renderConversationSource,
  renderConversationTranscript,
  preprocessConversationItems,
  groupConversationItemsIntoTurns,
  renderConversationGroup,
  renderWorkTurn,
  renderConversationRenderItem,
  renderUserPromptItem,
  renderSessionSetupItem,
  renderTextItem,
  renderThinkingItem,
  renderSystemItem,
  renderToolCallItem,
  renderMarkdownText,
  preserveDetailScroll,
  nextFrame,
  fileName,
  truncate,
  refreshMemberDetail,
  bindConversationEvents,
  toggleToolRow,
});
