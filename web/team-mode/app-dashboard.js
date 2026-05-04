const DASHBOARD_VIEWS = ["chat", "dashboard"];

function dashboardSupportsSse() {
  return typeof window !== "undefined" && "EventSource" in window;
}

function dashboardTransportMode() {
  if (dashboardMockEnabled() || !state.teamId) {
    return "mock";
  }
  return dashboardSupportsSse() ? "sse" : "polling";
}

function dashboardMockEnabled() {
  const search = new URLSearchParams(window.location.search || "");
  const hash = new URLSearchParams((window.location.hash || "").replace(/^#/, ""));
  return search.get("mock") === "1" || hash.get("mock") === "1";
}

function ensureDashboardState() {
  if (!DASHBOARD_VIEWS.includes(state.workspaceView)) {
    state.workspaceView = "chat";
  }
  if (!state.dashboard) {
    const mockMode = dashboardMockEnabled() || !state.teamId;
    state.dashboard = {
      phase: "ready",
      error: "",
      data: mockMode ? cloneDashboardFixture() : { workers: [], agents: [] },
      connection: {
        teamId: null,
        cursor: "",
        failures: 0,
        retryDelayMs: DASHBOARD_POLL_INTERVAL_MS,
        backoffUntil: 0,
      },
      transport: {
        polling: mockMode ? "ready" : "idle",
        sse: dashboardSupportsSse() ? "connecting" : "unavailable",
        source: mockMode ? "mock" : "events",
        mode: dashboardTransportMode(),
      },
    };
  }
  return state.dashboard;
}

function dashboardTransportSummary() {
  const dashboard = ensureDashboardState();
  if (dashboard.transport.source === "mock") {
    return dashboardText("mockReady");
  }
  if (dashboard.transport.mode === "sse") {
    const sseKey = {
      connected: "sseConnected",
      reconnecting: "sseReconnecting",
      connecting: "sseConnecting",
      standby: "sseStandby",
    }[dashboard.transport.sse] || "sseUnavailable";
    return dashboardText(sseKey);
  }
  const pollingKey = {
    connected: "pollingConnected",
    connecting: "pollingConnecting",
    disconnected: "pollingDisconnected",
    ready: "pollingReady",
  }[dashboard.transport.polling] || "pollingReady";
  if (dashboard.transport.source === "fallback-sse-failure") {
    return dashboardText("pollingFallbackAfterSseFailure");
  }
  return dashboard.transport.source === "fallback"
    ? `${dashboardText(pollingFallbackKey(pollingKey))}`
    : dashboardText(pollingKey);
}

function dashboardSourceLabel() {
  const dashboard = ensureDashboardState();
  if (dashboard.transport.source === "mock") {
    return dashboardText("mockSource");
  }
  if (dashboard.transport.mode === "sse") {
    return dashboardText("sseSource");
  }
  if (dashboard.transport.source === "fallback-sse-failure") {
    return dashboardText("pollingFallbackAfterSseFailure");
  }
  return dashboard.transport.source === "fallback" ? dashboardText("pollingFallback") : dashboardText("pollingSource");
}

function pollingFallbackKey(pollingKey) {
  return pollingKey === "pollingConnected" ? "pollingFallback" : pollingKey;
}

function switchWorkspaceView(view) {
  if (!DASHBOARD_VIEWS.includes(view)) {
    return;
  }
  state.workspaceView = view;
  renderShell();
}

function bindDashboardEvents() {
  $("chatViewButton")?.addEventListener("click", () => switchWorkspaceView("chat"));
  $("dashboardViewButton")?.addEventListener("click", () => switchWorkspaceView("dashboard"));
}

const DASHBOARD_POLL_INTERVAL_MS = 2000;
const DASHBOARD_MAX_BACKOFF_MS = 10000;
const DASHBOARD_EVENT_LIMIT = 100;
const DASHBOARD_SSE_RUNTIME_ERROR_LIMIT = 3;
let dashboardPollTimer = null;
let dashboardEventSource = null;
let dashboardEventSourceMeta = null;
let dashboardConnectionGeneration = 0;

function dashboardHasData(data) {
  return Boolean((data?.workers || []).length || (data?.agents || []).length);
}

async function openTeamEvents(teamId) {
  const dashboard = ensureDashboardState();
  if (!teamId || dashboardMockEnabled()) {
    closeTeamEvents();
    state.dashboard = {
      ...dashboard,
      phase: "ready",
      error: "",
      data: cloneDashboardFixture(),
      transport: { ...dashboard.transport, polling: "ready", source: "mock" },
    };
    renderShell();
    return;
  }
  closeTeamEvents();
  const mode = dashboardTransportMode();
  state.dashboard = {
    ...dashboard,
    phase: dashboardHasData(dashboardSnapshotFromState(teamId)) ? "ready" : "loading",
    error: "",
    data: dashboardSnapshotFromState(teamId),
    connection: {
      teamId,
      cursor: "",
      failures: 0,
      retryDelayMs: DASHBOARD_POLL_INTERVAL_MS,
      backoffUntil: 0,
      generation: dashboardConnectionGeneration,
    },
    transport: {
      polling: mode === "polling" ? "connecting" : "ready",
      sse: mode === "sse" ? "connecting" : "unavailable",
      source: mode === "polling" ? "fallback" : "events",
      mode,
    },
  };
  renderShell();
  if (mode === "sse" && openDashboardSse(teamId)) {
    return;
  }
  if (mode === "sse") {
    activateDashboardPollingFallback();
  }
  startDashboardPolling();
  await pollDashboardEvents({ force: true });
}

function closeTeamEvents() {
  closeDashboardEventSource();
  if (dashboardPollTimer) {
    const clearFn = window.clearInterval || globalThis.clearInterval;
    clearFn?.(dashboardPollTimer);
    dashboardPollTimer = null;
  }
}

function closeDashboardEventSource() {
  dashboardConnectionGeneration += 1;
  if (dashboardEventSource) {
    dashboardEventSource.close?.();
    dashboardEventSource = null;
  }
  dashboardEventSourceMeta = null;
}

function openDashboardSse(teamId) {
  if (!dashboardSupportsSse()) {
    return false;
  }
  const EventSourceCtor = window.EventSource;
  const generation = dashboardConnectionGeneration;
  const cursorParam = encodeURIComponent(state.dashboard?.connection?.cursor || "");
  // BUG-7: SSE bypasses api()/apiPost(), so propagate ?project= manually.
  const baseUrl = `/api/teams/${encodeURIComponent(teamId)}/events/stream?cursor=${cursorParam}`;
  const url = typeof globalThis.withProjectQuery === "function"
    ? globalThis.withProjectQuery(baseUrl)
    : baseUrl;
  let source = null;
  try {
    source = new EventSourceCtor(url);
  } catch (error) {
    return false;
  }
  dashboardEventSource = source;
  dashboardEventSourceMeta = { teamId, generation, runtimeErrors: 0, hasConnected: false };
  source.onopen = () => {
    applyDashboardSseStatus(teamId, generation, "connected");
  };
  source.onerror = () => {
    handleDashboardSseRuntimeError(teamId, generation);
  };
  for (const eventType of ["messageCreated", "fileChanged", "workerStatusChanged"]) {
    source.addEventListener?.(eventType, (event) => applyDashboardSseFrame(teamId, generation, event));
  }
  source.addEventListener?.("heartbeat", (event) => applyDashboardSseHeartbeat(teamId, generation, event));
  return true;
}

async function handleDashboardSseRuntimeError(teamId, generation) {
  if (!dashboardSseRequestIsCurrent(teamId, generation)) {
    return false;
  }
  const errors = (dashboardEventSourceMeta?.runtimeErrors || 0) + 1;
  dashboardEventSourceMeta = { ...dashboardEventSourceMeta, runtimeErrors: errors };
  if (errors < DASHBOARD_SSE_RUNTIME_ERROR_LIMIT) {
    return applyDashboardSseStatus(teamId, generation, "reconnecting");
  }
  closeDashboardEventSource();
  activateDashboardPollingFallback("fallback-sse-failure");
  startDashboardPolling();
  await resnapshotDashboardMembers(teamId);
  await pollDashboardEvents({ force: true });
  return true;
}

function activateDashboardPollingFallback(source = "fallback") {
  const dashboard = ensureDashboardState();
  state.dashboard = {
    ...dashboard,
    transport: {
      ...dashboard.transport,
      polling: "connecting",
      sse: "unavailable",
      source,
      mode: "polling",
    },
  };
}

function startDashboardPolling() {
  if (dashboardPollTimer) {
    return;
  }
  const intervalFn = window.setInterval || globalThis.setInterval;
  dashboardPollTimer = intervalFn?.(() => pollDashboardEvents(), DASHBOARD_POLL_INTERVAL_MS) || null;
}

async function pollDashboardEvents(options = {}) {
  const dashboard = ensureDashboardState();
  const connection = dashboard.connection || {};
  if (!connection.teamId) {
    return;
  }
  const now = Date.now();
  if (!options.force && connection.backoffUntil && now < connection.backoffUntil) {
    return;
  }
  const cursorParam = encodeURIComponent(connection.cursor || "");
  const requestedTeamId = connection.teamId;
  const teamParam = encodeURIComponent(requestedTeamId);
  try {
    const response = await api(`/api/teams/${teamParam}/events?cursor=${cursorParam}&limit=${DASHBOARD_EVENT_LIMIT}`);
    if (!applyDashboardEventsResponse(response, requestedTeamId)) {
      return;
    }
  } catch (error) {
    if (!applyDashboardEventsError(error, requestedTeamId)) {
      return;
    }
  }
  renderShell();
}

function dashboardRequestIsCurrent(requestedTeamId, response) {
  const currentTeamId = state.teamId || "";
  const dashboardTeamId = state.dashboard?.connection?.teamId || "";
  if (!requestedTeamId || currentTeamId !== requestedTeamId || dashboardTeamId !== requestedTeamId) {
    return false;
  }
  return !response?.teamId || response.teamId === requestedTeamId;
}

function dashboardSseRequestIsCurrent(requestedTeamId, generation, response) {
  const meta = dashboardEventSourceMeta || {};
  if (meta.teamId !== requestedTeamId || meta.generation !== generation) {
    return false;
  }
  return dashboardRequestIsCurrent(requestedTeamId, response);
}

function applyDashboardSseStatus(requestedTeamId, generation, sseStatus) {
  if (!dashboardSseRequestIsCurrent(requestedTeamId, generation)) {
    return false;
  }
  const dashboard = ensureDashboardState();
  const shouldResnapshot =
    sseStatus === "connected" &&
    dashboard.transport?.sse !== "connected" &&
    Boolean(dashboardEventSourceMeta?.hasConnected);
  if (sseStatus === "connected" && dashboardEventSourceMeta) {
    dashboardEventSourceMeta = { ...dashboardEventSourceMeta, runtimeErrors: 0, hasConnected: true };
  }
  state.dashboard = {
    ...dashboard,
    phase: dashboardHasData(dashboard.data) ? "ready" : dashboard.phase === "loading" ? "ready" : dashboard.phase,
    error: sseStatus === "connected" ? "" : dashboard.error,
    connection: {
      ...dashboard.connection,
      failures: sseStatus === "connected" ? 0 : dashboard.connection?.failures || 0,
    },
    transport: { ...dashboard.transport, sse: sseStatus, source: "events", mode: "sse" },
  };
  renderShell();
  if (shouldResnapshot) {
    resnapshotDashboardMembers(requestedTeamId, { generation });
  }
  return true;
}

function applyDashboardSseFrame(requestedTeamId, generation, frame) {
  let event = null;
  try {
    event = JSON.parse(frame?.data || "{}");
  } catch (error) {
    return applyDashboardSseStatus(requestedTeamId, generation, "reconnecting");
  }
  if (!dashboardSseRequestIsCurrent(requestedTeamId, generation, event)) {
    return false;
  }
  if (dashboardEventSourceMeta) {
    dashboardEventSourceMeta = { ...dashboardEventSourceMeta, runtimeErrors: 0 };
  }
  const cursor = event.cursor || frame?.lastEventId || "";
  return applyDashboardEventsResponse(
    {
      teamId: event.teamId || requestedTeamId,
      generatedAt: event.occurredAt,
      events: [event],
      page: { nextCursor: cursor },
    },
    requestedTeamId,
  ) && applyDashboardSseStatus(requestedTeamId, generation, "connected");
}

function applyDashboardSseHeartbeat(requestedTeamId, generation, frame) {
  if (!dashboardSseRequestIsCurrent(requestedTeamId, generation)) {
    return false;
  }
  if (dashboardEventSourceMeta) {
    dashboardEventSourceMeta = { ...dashboardEventSourceMeta, runtimeErrors: 0 };
  }
  const dashboard = ensureDashboardState();
  state.dashboard = {
    ...dashboard,
    phase: dashboard.phase === "loading" ? "ready" : dashboard.phase,
    connection: {
      ...dashboard.connection,
      cursor: frame?.lastEventId || dashboard.connection?.cursor || "",
    },
    transport: { ...dashboard.transport, sse: "connected", source: "events", mode: "sse" },
  };
  renderShell();
  return true;
}

function applyDashboardEventsResponse(response, requestedTeamId = state.dashboard?.connection?.teamId) {
  if (!dashboardRequestIsCurrent(requestedTeamId, response)) {
    return false;
  }
  const dashboard = ensureDashboardState();
  const nextData = mergeDashboardEvents(dashboard.data, response.events || [], response.generatedAt);
  const transport = dashboard.transport?.mode === "sse"
    ? { ...dashboard.transport, source: "events", mode: "sse" }
    : {
        ...dashboard.transport,
        polling: "connected",
        source: dashboard.transport?.source || "events",
        mode: dashboard.transport?.mode || "polling",
      };
  state.dashboard = {
    ...dashboard,
    phase: "ready",
    error: "",
    data: nextData,
    connection: {
      ...dashboard.connection,
      cursor: response.page?.nextCursor || dashboard.connection?.cursor || "",
      failures: 0,
      retryDelayMs: DASHBOARD_POLL_INTERVAL_MS,
      backoffUntil: 0,
    },
    transport,
  };
  return true;
}

function applyDashboardEventsError(error, requestedTeamId = state.dashboard?.connection?.teamId) {
  if (!dashboardRequestIsCurrent(requestedTeamId)) {
    return false;
  }
  const dashboard = ensureDashboardState();
  const failures = (dashboard.connection?.failures || 0) + 1;
  const retryDelayMs = Math.min(
    DASHBOARD_POLL_INTERVAL_MS * 2 ** Math.max(failures - 1, 0),
    DASHBOARD_MAX_BACKOFF_MS,
  );
  const transport = dashboard.transport?.mode === "sse"
    ? { ...dashboard.transport, sse: "reconnecting", source: "events", mode: "sse" }
    : {
        ...dashboard.transport,
        polling: "disconnected",
        source: dashboard.transport?.source || "events",
        mode: dashboard.transport?.mode || "polling",
      };
  state.dashboard = {
    ...dashboard,
    phase: dashboardHasData(dashboard.data) ? "ready" : "error",
    error: error?.message || dashboardText("error"),
    connection: {
      ...dashboard.connection,
      failures,
      retryDelayMs,
      backoffUntil: Date.now() + retryDelayMs,
    },
    transport,
  };
  return true;
}

function mergeDashboardEvents(currentData, events, generatedAt) {
  return (events || []).reduce(
    (data, event) => applyDashboardEvent(data, event),
    {
      generatedAt,
      workers: (currentData?.workers || []).map((worker) => ({ ...worker })),
      agents: (currentData?.agents || []).map((agent) => ({
        ...agent,
        tasks: (agent.tasks || []).map((task) => ({ ...task })),
      })),
    },
  );
}

function dashboardSnapshotFromState(teamId = state.teamId, members = state.members) {
  const workers = (members || [])
    .map((member) => ({
      name: member.name,
      status: deriveWorkerStatusMeta(member).kind,
      adapter: member.adapter || "",
      sessionId: member.sessionId || member.latestSessionId || "",
      role: member.roleLabel || member.kind || "",
      lastActivityAt: member.lastActivityAt || "",
    }))
    .sort((a, b) => {
      const aLead = a.name === "lead";
      const bLead = b.name === "lead";
      if (aLead !== bLead) return aLead ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
  const messages = (state.room?.messages || []).slice(-40);
  const data = {
    generatedAt: new Date().toISOString(),
    teamId,
    workers,
    agents: [],
  };
  return messages.reduce((nextData, message) => {
    const event = {
      id: message.id,
      eventType: "messageCreated",
      payload: { message },
    };
    return applyMessageEvent(nextData, event);
  }, data);
}

function reconcileDashboardWorkersFromMembers(teamId = state.teamId, members = state.members) {
  const dashboard = state.dashboard;
  if (!dashboard || dashboard.transport?.source === "mock") {
    return false;
  }
  const dashboardTeamId = dashboard.connection?.teamId || teamId;
  if (!teamId || dashboardTeamId !== teamId) {
    return false;
  }
  const snapshot = dashboardSnapshotFromState(teamId, members);
  const currentData = dashboard.data || {};
  const nextData = {
    ...currentData,
    generatedAt: snapshot.generatedAt,
    teamId,
    workers: snapshot.workers,
    agents: mergeDashboardAgents(snapshot.agents, currentData.agents || []),
  };
  state.dashboard = {
    ...dashboard,
    phase: dashboardHasData(nextData) ? "ready" : dashboard.phase,
    data: nextData,
  };
  return true;
}

function mergeDashboardAgents(snapshotAgents = [], projectedAgents = []) {
  const byName = new Map();
  for (const agent of snapshotAgents) {
    byName.set(agent.name, {
      ...agent,
      tasks: (agent.tasks || []).map((task) => ({ ...task })),
    });
  }
  for (const agent of projectedAgents) {
    const existing = byName.get(agent.name);
    if (!existing) {
      byName.set(agent.name, {
        ...agent,
        tasks: (agent.tasks || []).map((task) => ({ ...task })),
      });
      continue;
    }
    const tasks = new Map((existing.tasks || []).map((task) => [task.id, { ...task }]));
    for (const task of agent.tasks || []) {
      tasks.set(task.id, { ...task });
    }
    byName.set(agent.name, { ...existing, tasks: Array.from(tasks.values()) });
  }
  return Array.from(byName.values());
}

async function resnapshotDashboardMembers(teamId = state.teamId, options = {}) {
  if (!teamId || dashboardMockEnabled()) {
    return false;
  }
  const generation = options.generation ?? null;
  if (generation !== null && !dashboardSseRequestIsCurrent(teamId, generation)) {
    return false;
  }
  try {
    const members = await api(`/api/teams/${encodeURIComponent(teamId)}/members`);
    if (state.teamId !== teamId) {
      return false;
    }
    if (generation !== null && !dashboardSseRequestIsCurrent(teamId, generation)) {
      return false;
    }
    state.members = members.members || [];
    reconcileDashboardWorkersFromMembers(teamId, state.members);
    renderShell();
    return true;
  } catch (error) {
    return false;
  }
}

function applyDashboardEvent(data, event) {
  if (event.eventType === "workerStatusChanged") {
    return applyWorkerEvent(data, event);
  }
  if (event.eventType === "messageCreated") {
    return applyMessageEvent(data, event);
  }
  if (event.eventType === "fileChanged") {
    return upsertAgentTask(data, "system", {
      id: event.id,
      label: `${event.payload?.path || "file"} ${event.payload?.changeKind || "changed"}`,
      state: "active",
    });
  }
  return data;
}

function applyWorkerEvent(data, event) {
  const payload = event.payload || {};
  const name = payload.workerName || "unknown";
  const lifecycle = payload.lifecycleEvent || payload.sessionState || "alive";
  const statusMeta = deriveWorkerStatusMeta(
    {
      name,
      kind: "member",
      sessionState: lifecycle,
      lastActivityAt: payload.lastActivityAt || event.occurredAt || event.generatedAt,
    },
    {},
    { sessionState: lifecycle },
  );
  const workers = [
    ...data.workers.filter((worker) => worker.name !== name),
    {
      name,
      status: statusMeta.kind,
      adapter: payload.adapter || "",
      sessionId: payload.sessionId || "pending",
      role: payload.model || payload.cwd || "worker",
      lastActivityAt: payload.lastActivityAt || event.occurredAt || "",
    },
  ].sort((a, b) => a.name.localeCompare(b.name));
  return upsertAgentTask(
    { ...data, workers },
    name,
    {
      id: `worker-${name}`,
      label: `${name} ${lifecycle}`,
      state: lifecycle === "dead" ? "blocked" : "active",
    },
  );
}

function applyMessageEvent(data, event) {
  const message = event.payload?.message || {};
  const agentNames = dashboardMessageAttribution(message);
  return agentNames.reduce(
    (nextData, agentName) =>
      upsertAgentTask(nextData, agentName, {
        id: event.id || message.id,
        label: message.bodyPreview || message.body || event.eventType,
        state: message.kind === "reply" ? "done" : "active",
      }),
    data,
  );
}

function dashboardMessageAttribution(message) {
  if (message.kind === "reply" && message.sender) {
    return [message.sender];
  }
  const targets = [
    ...(message.mentions || []),
    ...(message.effectiveRecipients || []),
  ].filter(Boolean);
  return targets.length ? Array.from(new Set(targets)) : [message.sender || "team"];
}

function upsertAgentTask(data, agentName, task) {
  const agents = data.agents || [];
  const existing = agents.find((agent) => agent.name === agentName);
  const nextTask = { ...task };
  if (!existing) {
    return { ...data, agents: [...agents, { name: agentName, tasks: [nextTask] }] };
  }
  return {
    ...data,
    agents: agents.map((agent) =>
      agent.name === agentName
        ? { ...agent, tasks: [...agent.tasks.filter((item) => item.id !== nextTask.id), nextTask] }
        : agent,
    ),
  };
}

function setDashboardMockState(phase, overrides = {}) {
  const current = ensureDashboardState();
  state.dashboard = {
    ...current,
    ...overrides,
    phase,
    data: overrides.data === undefined ? current.data : overrides.data,
    transport: { ...current.transport, ...(overrides.transport || {}) },
  };
}

function dashboardTaskStats(agent) {
  const tasks = agent.tasks || [];
  const total = tasks.length;
  const done = tasks.filter((task) => task.state === "done").length;
  const active = tasks.filter((task) => task.state === "active").length;
  const blocked = tasks.filter((task) => task.state === "blocked").length;
  const pending = Math.max(total - done - active - blocked, 0);
  return { total, done, active, blocked, pending };
}

Object.assign(globalThis, {
  dashboardText,
  cloneDashboardFixture,
  ensureDashboardState,
  dashboardTransportSummary,
  dashboardSourceLabel,
  dashboardTransportMode,
  switchWorkspaceView,
  bindDashboardEvents,
  openTeamEvents,
  closeTeamEvents,
  closeDashboardEventSource,
  openDashboardSse,
  pollDashboardEvents,
  dashboardRequestIsCurrent,
  mergeDashboardEvents,
  reconcileDashboardWorkersFromMembers,
  resnapshotDashboardMembers,
  setDashboardMockState,
  dashboardTaskStats,
});
