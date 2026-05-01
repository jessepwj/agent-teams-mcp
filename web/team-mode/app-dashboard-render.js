const DASHBOARD_TEXT = {
  zh: {
    chat: "群聊",
    dashboard: "仪表盘",
    title: "团队仪表盘",
    subtitle: "通过实时事件流读取团队事件；不支持实时事件时降级为轮询。",
    workers: "成员状态",
    tasks: "任务进度",
    connection: "连接",
    pollingReady: "轮询就绪",
    pollingConnecting: "轮询连接中",
    pollingConnected: "轮询已连接",
    pollingDisconnected: "轮询已断开",
    pollingFallback: "轮询备用",
    pollingFallbackAfterSseFailure: "轮询备用（SSE 失败）",
    sseConnecting: "实时连接中",
    sseConnected: "实时已连接",
    sseReconnecting: "实时重连中",
    sseStandby: "实时待接入",
    sseUnavailable: "实时不可用",
    sseSource: "实时事件流",
    pollingSource: "轮询事件",
    mockSource: "模拟数据",
    mockReady: "数据已就绪",
    loading: "正在加载仪表盘...",
    empty: "暂无仪表盘数据。",
    error: "仪表盘加载失败",
    adapter: "适配器",
    session: "会话",
    progress: "进度",
    done: "完成",
    active: "活跃",
    blocked: "阻塞",
    pending: "待办",
    spawn: "已启动",
    alive: "在线",
    online: "在线",
    idle: "空闲",
    dead: "离线",
    revived: "已恢复",
    running: "运行中",
  },
  en: {
    chat: "Chat",
    dashboard: "Dashboard",
    title: "Team Dashboard",
    subtitle: "Team events stream over SSE; polling remains the fallback.",
    workers: "Worker Status",
    tasks: "Task Progress",
    connection: "Connection",
    pollingReady: "polling ready",
    pollingConnecting: "polling connecting",
    pollingConnected: "polling connected",
    pollingDisconnected: "polling disconnected",
    pollingFallback: "polling fallback",
    pollingFallbackAfterSseFailure: "polling fallback after sse failure",
    sseConnecting: "sse connecting",
    sseConnected: "sse connected",
    sseReconnecting: "sse reconnecting",
    sseStandby: "SSE standby",
    sseUnavailable: "SSE unavailable",
    sseSource: "events SSE",
    pollingSource: "events polling",
    mockSource: "mock fixture",
    mockReady: "ready",
    loading: "Loading dashboard...",
    empty: "No dashboard data.",
    error: "Dashboard failed to load",
    adapter: "Adapter",
    session: "Session",
    progress: "Progress",
    done: "Done",
    active: "active",
    blocked: "Blocked",
    pending: "Pending",
    spawn: "spawn",
    alive: "alive",
    online: "online",
    idle: "idle",
    dead: "dead",
    revived: "revived",
    running: "running",
  },
};

const DASHBOARD_MOCK_FIXTURE = {
  generatedAt: "2026-04-27T11:30:00Z",
  workers: [
    { name: "backend-dev", status: "alive", adapter: "codex", sessionId: "mock-backend-t4", role: "Rust backend" },
    { name: "frontend-dev", status: "spawn", adapter: "codex", sessionId: "mock-frontend-t5", role: "Web UI" },
    { name: "researcher", status: "revived", adapter: "codex", sessionId: "mock-research-t2", role: "Research" },
    { name: "e2e-tester", status: "dead", adapter: "codex", sessionId: "pending", role: "E2E" },
  ],
  agents: [
    {
      name: "researcher",
      tasks: [
        { id: "T2", label: "viz recommendation", state: "done" },
        { id: "api-doc", label: "api contracts refresh", state: "done" },
      ],
    },
    {
      name: "backend-dev",
      tasks: [
        { id: "T4", label: "polling endpoint v1", state: "active" },
        { id: "events", label: "event projection", state: "pending" },
      ],
    },
    {
      name: "frontend-dev",
      tasks: [
        { id: "baseline", label: "clean baseline", state: "done" },
        { id: "T5", label: "dashboard skeleton", state: "active" },
        { id: "api-cutover", label: "real API cutover", state: "blocked" },
      ],
    },
    {
      name: "reviewer",
      tasks: [{ id: "review-t5", label: "dashboard review", state: "pending" }],
    },
  ],
};

function dashboardText(key) {
  return DASHBOARD_TEXT[state.language]?.[key] || DASHBOARD_TEXT.zh[key] || key;
}

function cloneDashboardFixture(fixture = DASHBOARD_MOCK_FIXTURE) {
  return {
    ...fixture,
    workers: (fixture.workers || []).map((worker) => ({ ...worker })),
    agents: (fixture.agents || []).map((agent) => ({
      ...agent,
      tasks: (agent.tasks || []).map((task) => ({ ...task })),
    })),
  };
}

function renderDashboardShell() {
  ensureDashboardState();
  renderWorkspaceTabs();
  renderDashboard();
}

function renderWorkspaceTabs() {
  const view = state.workspaceView || "chat";
  const chatButton = $("chatViewButton");
  const dashboardButton = $("dashboardViewButton");
  const chatPanel = $("workspace");
  const dashboardPanel = $("dashboardWorkspace");
  if (!chatButton || !dashboardButton || !chatPanel || !dashboardPanel) {
    return;
  }
  chatButton.textContent = dashboardText("chat");
  dashboardButton.textContent = dashboardText("dashboard");
  chatButton.classList.toggle("active", view === "chat");
  dashboardButton.classList.toggle("active", view === "dashboard");
  chatButton.setAttribute("aria-selected", view === "chat" ? "true" : "false");
  dashboardButton.setAttribute("aria-selected", view === "dashboard" ? "true" : "false");
  chatPanel.hidden = view !== "chat";
  dashboardPanel.hidden = view !== "dashboard";
}

function renderDashboard() {
  const root = $("dashboardRoot");
  if (!root) {
    return;
  }
  const dashboard = ensureDashboardState();
  if (dashboard.phase === "loading") {
    root.innerHTML = renderDashboardFrame(`<div class="dashboard-state loading">${dashboardText("loading")}</div>`);
    return;
  }
  if (dashboard.phase === "error") {
    const errorText = dashboard.error || dashboardText("error");
    root.innerHTML = renderDashboardFrame(`<div class="dashboard-state error">${escapeHtml(errorText)}</div>`);
    return;
  }
  const data = dashboard.data || {};
  const workers = data.workers || [];
  const agents = data.agents || [];
  if (!workers.length && !agents.length) {
    root.innerHTML = renderDashboardFrame(`<div class="dashboard-state empty">${dashboardText("empty")}</div>`);
    return;
  }
  root.innerHTML = renderDashboardFrame(`
    ${dashboard.error ? `<div class="dashboard-state error">${escapeHtml(dashboard.error)}</div>` : ""}
    <div class="dashboard-grid">
      ${renderWorkerPanel(workers)}
      ${renderTaskPanel(agents)}
    </div>
  `);
}

function renderDashboardFrame(content) {
  return `
    <div class="dashboard-header">
      <div>
        <div class="section-title">${dashboardText("dashboard")}</div>
        <h2>${dashboardText("title")}</h2>
        <div class="subtle">${dashboardText("subtitle")}</div>
      </div>
      <div class="connection-stack" aria-label="${escapeAttr(dashboardText("connection"))}">
        <span class="connection-badge">${escapeHtml(dashboardTransportSummary())}</span>
        <span class="connection-source">${escapeHtml(dashboardSourceLabel())}</span>
      </div>
    </div>
    ${content}
  `;
}

function renderWorkerPanel(workers) {
  return `
    <section class="dashboard-panel dashboard-worker-panel" aria-labelledby="dashboardWorkersTitle">
      <div class="dashboard-panel-head">
        <h3 id="dashboardWorkersTitle">${dashboardText("workers")}</h3>
        <span class="metric-pill">${workers.length}</span>
      </div>
      <div class="worker-table" role="list" aria-label="${escapeAttr(dashboardText("workers"))}">
        ${workers.map(renderWorkerRow).join("")}
      </div>
    </section>
  `;
}

function renderWorkerRow(worker) {
  return `
    <article class="worker-row" role="listitem">
      <div>
        <div class="worker-name">${escapeHtml(worker.name)}</div>
        <div class="subtle">${escapeHtml(worker.role || "")}</div>
      </div>
      ${renderDashboardStatusPill(worker.status)}
      <div class="worker-meta">
        <span>${dashboardText("adapter")}: ${escapeHtml(worker.adapter || na())}</span>
        <span>${dashboardText("session")}: ${escapeHtml(worker.sessionId || na())}</span>
      </div>
    </article>
  `;
}

function renderDashboardStatusPill(status) {
  const meta = deriveWorkerStatusMeta({ sessionState: status || "pending" });
  return `<span class="dash-status dash-status-${escapeAttr(meta.kind)}">${escapeHtml(meta.label)}</span>`;
}

function renderTaskPanel(agents) {
  return `
    <section class="dashboard-panel dashboard-task-panel" aria-labelledby="dashboardTasksTitle">
      <div class="dashboard-panel-head">
        <h3 id="dashboardTasksTitle">${dashboardText("tasks")}</h3>
        <span class="metric-pill">${dashboardText("progress")}</span>
      </div>
      ${renderTaskProgressSvg(agents)}
      <div class="task-agent-list" role="list" aria-label="${escapeAttr(dashboardText("tasks"))}">
        ${agents.map(renderTaskAgent).join("")}
      </div>
    </section>
  `;
}

function renderTaskProgressSvg(agents) {
  const width = 520;
  const height = 180;
  const gap = 22;
  const barWidth = 48;
  const startX = 34;
  const baseY = 130;
  const maxBar = 92;
  const bars = agents.map((agent, index) => {
    const stats = dashboardTaskStats(agent);
    const ratio = stats.total ? stats.done / stats.total : 0;
    const barHeight = Math.max(8, Math.round(ratio * maxBar));
    const x = startX + index * (barWidth + gap);
    const y = baseY - barHeight;
    return { agent, stats, x, y, barHeight };
  });
  const label = `${dashboardText("tasks")} ${dashboardText("progress")}`;
  return `
    <svg class="task-progress-svg" viewBox="0 0 ${width} ${height}" role="img" aria-label="${escapeAttr(label)}">
      <line x1="20" y1="${baseY}" x2="500" y2="${baseY}" class="svg-axis"></line>
      <polyline points="${bars.map((bar) => `${bar.x + barWidth / 2},${bar.y}`).join(" ")}" class="svg-trend"></polyline>
      ${bars
        .map(
          (bar) => `
            <g>
              <rect x="${bar.x}" y="${bar.y}" width="${barWidth}" height="${bar.barHeight}" rx="5" class="svg-bar"></rect>
              <text x="${bar.x + barWidth / 2}" y="154" text-anchor="middle">${escapeHtml(bar.agent.name)}</text>
              <text x="${bar.x + barWidth / 2}" y="${bar.y - 8}" text-anchor="middle">${bar.stats.done}/${bar.stats.total}</text>
            </g>
          `,
        )
        .join("")}
    </svg>
  `;
}

function renderTaskAgent(agent) {
  const stats = dashboardTaskStats(agent);
  return `
    <article class="task-agent" role="listitem">
      <div class="task-agent-head">
        <strong>${escapeHtml(agent.name)}</strong>
        <span>${stats.done}/${stats.total}</span>
      </div>
      <div class="task-pills">
        ${agent.tasks.map((task) => `<span class="task-pill task-${escapeAttr(task.state)}">${escapeHtml(task.id)} · ${escapeHtml(task.label)}</span>`).join("")}
      </div>
    </article>
  `;
}

Object.assign(globalThis, {
  renderDashboardShell,
  renderDashboard,
  renderWorkspaceTabs,
  renderTaskProgressSvg,
});
