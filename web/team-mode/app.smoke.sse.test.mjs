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

test("dashboard connects SSE and renders messageCreated events", async () => {
  const payloads = basePayloads();
  const requests = [];
  const fakeSse = createFakeEventSourceHarness();
  const harness = createHarness({
    fetchImpl: async (url) => {
      requests.push(url);
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });
  harness.context.window.EventSource = fakeSse.FakeEventSource;

  await harness.start();
  assert.equal(fakeSse.instances.length, 1);
  assert.equal(fakeSse.instances[0].url, "/api/teams/demo/events/stream?cursor=");
  assert.equal(requests.some((url) => url.includes("/events?")), false);

  fakeSse.instances[0].dispatchOpen();
  fakeSse.instances[0].dispatchEventFrame(
    messageEvent("demo", "m-sse-1", "Implement dashboard SSE", ["frontend-dev"], "c-sse-message"),
  );
  await flushPromises();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  assert.equal(harness.context.dashboardTransportMode(), "sse");
  assert.equal(harness.context.state.dashboard.connection.cursor, "c-sse-message");
  assert.match(harness.document.getElementById("liveStatus").textContent, /实时已连接/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /Implement dashboard SSE/);
});


test("dashboard merges SSE worker file and heartbeat frames", async () => {
  const payloads = basePayloads();
  const fakeSse = createFakeEventSourceHarness();
  const membersPayload = await payloads["/api/teams/demo/members"].json();
  membersPayload.members.push({
    name: "sse-worker",
    kind: "member",
    roleLabel: "worker",
    status: "active",
    sessionState: "running",
  });
  payloads["/api/teams/demo/members"] = okJson(membersPayload);
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });
  harness.context.window.EventSource = fakeSse.FakeEventSource;

  await harness.start();
  const source = fakeSse.instances[0];
  source.dispatchOpen();
  source.dispatchEventFrame(workerEvent("demo", "sse-worker", "revived", "c-sse-worker"));
  source.dispatchEventFrame(fileEvent("demo", ".plans/agent-teams-v2/frontend-dev/task_plan.md", "modified", "c-sse-file"));
  source.dispatchHeartbeat("c-sse-file");
  await flushPromises();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  assert.equal(harness.context.state.dashboard.connection.cursor, "c-sse-file");
  assert.match(dashboardHtml, /sse-worker/);
  assert.match(dashboardHtml, /dash-status-revived/);
  assert.match(dashboardHtml, /task_plan\.md modified/);
  assert.equal((dashboardHtml.match(/heartbeat/g) || []).length, 0);
});

test("dashboard reconnects SSE without losing cursor", async () => {
  const payloads = basePayloads();
  const fakeSse = createFakeEventSourceHarness();
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });
  harness.context.window.EventSource = fakeSse.FakeEventSource;

  await harness.start();
  const source = fakeSse.instances[0];
  source.dispatchOpen();
  source.dispatchEventFrame(messageEvent("demo", "m-sse-before", "Before reconnect", ["frontend-dev"], "c-before"));
  source.dispatchError();
  await flushPromises();

  assert.equal(harness.context.state.dashboard.connection.cursor, "c-before");
  assert.match(harness.document.getElementById("liveStatus").textContent, /实时重连中/);

  source.dispatchOpen();
  source.dispatchEventFrame(messageEvent("demo", "m-sse-after", "After reconnect", ["frontend-dev"], "c-after"));
  await flushPromises();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  assert.equal(harness.context.state.dashboard.connection.cursor, "c-after");
  assert.match(harness.document.getElementById("liveStatus").textContent, /实时已连接/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /Before reconnect/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /After reconnect/);
});


test("dashboard SSE single transient error keeps reconnecting without fallback", async () => {
  const payloads = basePayloads();
  const requests = [];
  const fakeSse = createFakeEventSourceHarness();
  const harness = createHarness({
    fetchImpl: async (url) => {
      requests.push(url);
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });
  harness.context.window.EventSource = fakeSse.FakeEventSource;

  await harness.start();
  const source = fakeSse.instances[0];
  source.dispatchOpen();
  source.dispatchEventFrame(messageEvent("demo", "m-sse-transient", "Transient before error", ["frontend-dev"], "c-transient"));
  source.dispatchError();
  await flushPromises();

  assert.equal(source.closed, false);
  assert.equal(harness.context.state.dashboard.transport.mode, "sse");
  assert.equal(harness.context.state.dashboard.connection.cursor, "c-transient");
  assert.match(harness.document.getElementById("liveStatus").textContent, /实时重连中/);
  assert.equal(requests.some((url) => url.includes("/events?cursor=c-transient")), false);
});


test("dashboard closes stale SSE connection when switching teams", async () => {
  const payloads = basePayloads();
  const fakeSse = createFakeEventSourceHarness();
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

  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });
  harness.context.window.EventSource = fakeSse.FakeEventSource;

  await harness.start();
  const demoSource = fakeSse.instances[0];
  demoSource.dispatchOpen();
  demoSource.dispatchEventFrame(workerEvent("demo", "demo-worker", "alive", "c-demo"));
  await flushPromises();

  await harness.context.loadTeam("beta");
  await flushPromises();

  assert.equal(demoSource.closed, true);
  assert.equal(fakeSse.instances.length, 2);
  assert.equal(fakeSse.instances[1].url, "/api/teams/beta/events/stream?cursor=");
  fakeSse.instances[1].dispatchOpen();
  fakeSse.instances[1].dispatchEventFrame(workerEvent("beta", "beta-worker", "alive", "c-beta"));
  demoSource.dispatchEventFrame(workerEvent("demo", "stale-demo-worker", "dead", "c-stale"));
  await flushPromises();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  assert.equal(harness.context.state.dashboard.connection.teamId, "beta");
  assert.equal(harness.context.state.dashboard.connection.cursor, "c-beta");
  assert.match(dashboardHtml, /beta-worker/);
  assert.doesNotMatch(dashboardHtml, /stale-demo-worker/);
  assert.equal(harness.context.state.dashboard.transport.sse, "connected");
});
