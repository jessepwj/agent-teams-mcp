function renderDiagnosticsSections() {
  if (!state.teamId) {
    return "";
  }
  if (state.diagnosticsLoading && !state.diagnostics && !state.diagnosticsError) {
    return `
      <div class="detail-card">
        <div class="section-title">${t("diagnostics")}</div>
        <div class="empty">${t("loadingDiagnostics")}</div>
      </div>
    `;
  }
  if (state.diagnosticsError) {
    return `
      <div class="detail-card">
        <div class="section-title">${t("diagnostics")}</div>
        <div class="empty">${escapeHtml(state.diagnosticsError)}</div>
        <button class="chip retry-button" type="button" id="retryDiagnosticsButton">${t("retryDiagnostics")}</button>
      </div>
    `;
  }
  if (!state.diagnostics) {
    return "";
  }

  const diagnostics = state.diagnostics;
  const sourceCards = (diagnostics.sources || [])
    .map((source) => {
      const statusClass = source.exists ? "lead" : "fail";
      const statusLabel = source.exists ? t("available") : t("missing");
      const sizeLabel =
        typeof source.sizeBytes === "number" ? `${source.sizeBytes} ${t("bytes")}` : na();
      return `
        <div class="diagnostic-source">
          <div class="detail-head">
            <span class="badge ${statusClass}">${escapeHtml(sourceLabel(source.label))}</span>
            <span class="pill">${escapeHtml(statusLabel)}</span>
            <span class="pill">${escapeHtml(label(source.kind))}</span>
          </div>
          <div class="subtle">${escapeHtml(source.path)}</div>
          <div class="detail-meta">
            <span class="pill">${escapeHtml(sizeLabel)}</span>
            <span class="pill">${fmtTime(source.updatedAt)}</span>
          </div>
          <pre class="diagnostic-preview">${escapeHtml(localText(source.preview || ""))}</pre>
        </div>
      `;
    })
    .join("");

  const leadSession = diagnostics.leadSession || {};
  const toolCalls = (leadSession.recentToolCalls || [])
    .map(
      (call) => `
        <div class="detail-item">
          <div class="detail-head">
            <span class="badge worker">${escapeHtml(call.toolName)}</span>
            <span class="pill">${fmtTime(call.timestamp)}</span>
          </div>
          <div class="subtle">${escapeHtml(call.inputSummary || t("noInputSummary"))}</div>
        </div>
      `,
    )
    .join("");

  const tokenUsage = leadSession.tokenUsage;
  const tokenUsageMarkup = tokenUsage
    ? `
      <div class="detail-grid">
        <div><span class="muted">${t("inputTokens")}</span><div>${tokenUsage.inputTokens}</div></div>
        <div><span class="muted">${t("outputTokens")}</span><div>${tokenUsage.outputTokens}</div></div>
        <div><span class="muted">${t("cacheRead")}</span><div>${tokenUsage.cacheReadTokens ?? na()}</div></div>
        <div><span class="muted">${t("cacheWrite")}</span><div>${tokenUsage.cacheWriteTokens ?? na()}</div></div>
        <div><span class="muted">${t("totalTokens")}</span><div>${tokenUsage.totalTokens}</div></div>
      </div>
    `
    : `<div class="empty-inline">${t("noTokenUsage")}</div>`;

  return `
    <div class="detail-card">
      <div class="section-title">${t("teamDiagnostics")}</div>
      <div class="detail-grid">
        <div><span class="muted">${t("teamId")}</span><div>${escapeHtml(diagnostics.teamId)}</div></div>
        <div><span class="muted">${t("generatedAt")}</span><div>${fmtTime(diagnostics.generatedAt)}</div></div>
        <div><span class="muted">${t("teamName")}</span><div>${escapeHtml(diagnostics.teamName || na())}</div></div>
        <div><span class="muted">${t("cwd")}</span><div>${escapeHtml(diagnostics.cwd || na())}</div></div>
      </div>
      <div class="detail-list-block">
        <div class="muted">${t("limitations")}</div>
        <div class="detail-pills">${
          (diagnostics.limitations || []).length
            ? (diagnostics.limitations || []).map((item) => `<span class="pill">${escapeHtml(localText(item))}</span>`).join("")
            : `<span class="empty-inline">${t("none")}</span>`
        }</div>
      </div>
    </div>
    <div class="detail-card">
      <div class="section-title">${t("diagnosticsSources")}</div>
      <div class="diagnostic-source-list">${sourceCards || `<div class="empty-inline">${t("noDiagnosticSources")}</div>`}</div>
    </div>
    <div class="detail-card">
      <div class="section-title">${t("leadSessionDiagnostics")}</div>
      <div class="detail-grid">
        <div><span class="muted">${t("discovered")}</span><div>${leadSession.discovered ? t("yes") : t("no")}</div></div>
        <div><span class="muted">${t("sessionCount")}</span><div>${leadSession.sessionCount ?? 0}</div></div>
        <div><span class="muted">${t("latestSession")}</span><div>${escapeHtml(leadSession.latestSessionId || na())}</div></div>
        <div><span class="muted">${t("latestModified")}</span><div>${fmtTime(leadSession.latestModifiedAt)}</div></div>
      </div>
      <div class="detail-list-block">
        <div class="muted">${t("sourcePath")}</div>
        <div class="subtle">${escapeHtml(leadSession.sourcePath || na())}</div>
      </div>
      <div class="detail-list-block">
        <div class="muted">${t("recentToolCalls")}</div>
        ${
          toolCalls
            ? `<div class="diagnostic-source-list">${toolCalls}</div>`
            : `<div class="empty-inline">${t("noRecentToolCalls")}</div>`
        }
      </div>
      <div class="detail-list-block">
        <div class="muted">${t("tokenUsage")}</div>
        ${tokenUsageMarkup}
      </div>
      <div class="detail-list-block">
        <div class="muted">${t("limitations")}</div>
        <div class="detail-pills">${
          (leadSession.limitations || []).length
            ? (leadSession.limitations || [])
                .map((item) => `<span class="pill">${escapeHtml(localText(item))}</span>`)
                .join("")
            : `<span class="empty-inline">${t("none")}</span>`
        }</div>
      </div>
    </div>
  `;
}

function bindDiagnosticsRetryButton() {
  const retryDiagnosticsButton = $("retryDiagnosticsButton");
  if (!retryDiagnosticsButton || !state.teamId) {
    return;
  }
  retryDiagnosticsButton.onclick = async () => {
    state.diagnosticsError = "";
    state.diagnosticsLoading = true;
    renderShell();
    await loadDiagnostics(state.teamId);
  };
}

Object.assign(globalThis, {
  renderDiagnosticsSections,
  bindDiagnosticsRetryButton,
});
