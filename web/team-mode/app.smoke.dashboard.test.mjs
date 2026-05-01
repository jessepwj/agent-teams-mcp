import {
  test,
  assert,
  okJson,
  failedJson,
  deferredOkJson,
  basePayloads,
  workerEvent,
  messageEvent,
  fileEvent,
  createFakeEventSourceHarness,
  createHarness,
  flushPromises,
  flushTimers,
} from "./app.smoke.shared.mjs";

test("dashboard loads initial polling events and renders API data", async () => {
  const payloads = basePayloads();
  const requests = [];
  const harness = createHarness({
    fetchImpl: async (url) => {
      requests.push(url);
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });

  await harness.start();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");
  await flushPromises();

  assert.ok(requests.includes("/api/teams/demo/events?cursor=&limit=100"));
  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  assert.match(dashboardHtml, /backend-dev/);
  assert.match(dashboardHtml, /Implement dashboard polling/);
  assert.match(harness.document.getElementById("liveStatus").textContent, /轮询备用/);
});


test("dashboard advances event cursor and merges incremental polling updates", async () => {
  const payloads = basePayloads();
  const requests = [];
  payloads["/api/teams/demo/events?cursor=c-initial&limit=100"] = okJson({
    teamId: "demo",
    generatedAt: "2026-04-27T12:00:02Z",
    events: [workerEvent("demo", "backend-dev", "dead", "c-worker-2")],
    page: { hasMoreAfter: false, nextCursor: "c-second" },
    limitations: [],
  });
  const harness = createHarness({
    fetchImpl: async (url) => {
      requests.push(url);
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });

  await harness.start();
  await harness.context.pollDashboardEvents({ force: true });
  await flushPromises();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  assert.ok(requests.includes("/api/teams/demo/events?cursor=&limit=100"));
  assert.ok(requests.includes("/api/teams/demo/events?cursor=c-initial&limit=100"));
  assert.equal(harness.context.state.dashboard.connection.cursor, "c-second");
  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  assert.match(dashboardHtml, /dash-status-dead/);
  assert.match(dashboardHtml, /Implement dashboard polling/);
});

test("dashboard marks polling disconnected, backs off, and recovers", async () => {
  const payloads = basePayloads();
  let failEvents = true;
  const harness = createHarness({
    fetchImpl: async (url) => {
      if (url.startsWith("/api/teams/demo/events")) {
        return failEvents ? failedJson(503, "Service Unavailable") : payloads[url];
      }
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });

  await harness.start();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");
  await flushPromises();

  assert.match(harness.document.getElementById("liveStatus").textContent, /轮询已断开/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /503 Service Unavailable/);
  assert.equal(harness.context.state.dashboard.connection.failures, 1);
  assert.equal(harness.context.state.dashboard.connection.retryDelayMs, 2000);

  failEvents = false;
  await harness.context.pollDashboardEvents({ force: true });
  await flushPromises();

  assert.match(harness.document.getElementById("liveStatus").textContent, /轮询备用/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /backend-dev/);
  assert.equal(harness.context.state.dashboard.connection.failures, 0);
});


test("dashboard uses mock fixture only when mock mode is requested", async () => {
  const payloads = basePayloads();
  const requests = [];
  const harness = createHarness({
    search: "?mock=1",
    fetchImpl: async (url) => {
      requests.push(url);
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });

  await harness.start();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");
  await flushPromises();

  assert.equal(requests.some((url) => url.includes("/events?")), false);
  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  assert.match(dashboardHtml, /mock-frontend-t5/);
  assert.match(dashboardHtml, /模拟数据/);
  assert.match(dashboardHtml, /数据已就绪/);
  const mockSourceCount = (dashboardHtml.match(/模拟数据/g) || []).length;
  assert.equal(mockSourceCount, 1, "mockSource label should appear in source chip only, not duplicated in transport chip");
});

test("dashboard ignores stale polling responses after switching teams", async () => {
  const payloads = basePayloads();
  payloads["/api/teams"] = okJson({
    teams: [
      {
        id: "demo",
        name: "Diagnostics Team",
        cwd: "E:/aigc/demo",
        status: "active",
        leadMemberId: "lead",
        memberCount: 2,
        activeWorkerCount: 1,
      },
      {
        id: "beta",
        name: "Beta Team",
        cwd: "E:/aigc/beta",
        status: "active",
        leadMemberId: "lead",
        memberCount: 1,
        activeWorkerCount: 1,
      },
    ],
  });
  payloads["/api/teams/beta"] = okJson({
    team: { id: "beta", name: "Beta Team", status: "active", leadMemberId: "lead" },
    counts: { memberCount: 1, activeWorkerCount: 1, messageCount: 0 },
  });
  payloads["/api/teams/beta/rooms/main?limit=200"] = okJson({
    room: { id: "main", teamId: "beta", status: "active" },
    messages: [],
    page: { hasMoreBefore: false, hasMoreAfter: false, nextCursor: null },
  });
  payloads["/api/teams/beta/members"] = okJson({
    members: [
      { name: "lead", kind: "lead", roleLabel: "lead", status: "active", sessionState: "coordinator" },
      { name: "beta-worker", kind: "member", roleLabel: "worker", status: "active", sessionState: "running" },
    ],
  });
  payloads["/api/teams/beta/diagnostics"] = payloads["/api/teams/demo/diagnostics"];
  payloads["/api/teams/beta/members/lead"] = payloads["/api/teams/demo/members/lead"];
  payloads["/api/teams/beta/members/lead/activity"] = payloads["/api/teams/demo/members/lead/activity"];
  payloads["/api/teams/beta/members/lead/conversation"] = payloads["/api/teams/demo/members/lead/conversation"];
  payloads["/api/teams/beta/events?cursor=&limit=100"] = okJson({
    teamId: "beta",
    generatedAt: "2026-04-28T09:00:00Z",
    events: [workerEvent("beta", "beta-worker", "alive", "c-beta-worker")],
    page: { hasMoreAfter: false, nextCursor: "c-beta" },
    limitations: [],
  });
  const staleDemo = deferredOkJson({
    teamId: "demo",
    generatedAt: "2026-04-28T09:00:01Z",
    events: [workerEvent("demo", "stale-demo-worker", "dead", "c-stale-worker")],
    page: { hasMoreAfter: false, nextCursor: "c-stale-demo" },
    limitations: [],
  });

  const harness = createHarness({
    fetchImpl: async (url) => {
      if (url === "/api/teams/demo/events?cursor=c-initial&limit=100") {
        return staleDemo.responsePromise;
      }
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });

  await harness.start();
  const stalePoll = harness.context.pollDashboardEvents({ force: true });
  await flushPromises();

  await harness.context.loadTeam("beta");
  await flushPromises();

  assert.equal(harness.context.state.dashboard.connection.teamId, "beta");
  assert.equal(harness.context.state.dashboard.connection.cursor, "c-beta");
  staleDemo.resolveResponse();
  await stalePoll;
  await flushPromises();

  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  assert.equal(harness.context.state.dashboard.connection.teamId, "beta");
  assert.equal(harness.context.state.dashboard.connection.cursor, "c-beta");
  assert.match(dashboardHtml, /beta-worker/);
  assert.doesNotMatch(dashboardHtml, /stale-demo-worker/);
  assert.doesNotMatch(harness.document.getElementById("liveStatus").textContent, /轮询已断开/);
});
