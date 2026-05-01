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

test("dashboard derives stale worker status from last activity", async () => {
  const payloads = basePayloads();
  const membersPayload = await payloads["/api/teams/demo/members"].json();
  membersPayload.members[1].lastActivityAt = "2000-01-01T00:00:00Z";
  payloads["/api/teams/demo/members"] = okJson(membersPayload);
  payloads["/api/teams/demo/events?cursor=&limit=100"] = okJson({
    teamId: "demo",
    generatedAt: "2026-04-27T12:00:00Z",
    events: [],
    page: { hasMoreAfter: false, nextCursor: "c-empty" },
    limitations: [],
  });
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");
  await flushPromises();

  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  assert.match(dashboardHtml, /dash-status-idle/);
  assert.match(dashboardHtml, /空闲/);
});

test("dashboard shows baseline member data even when event feed is empty", async () => {
  const payloads = basePayloads();
  payloads["/api/teams/demo/events?cursor=&limit=100"] = okJson({
    teamId: "demo",
    generatedAt: "2026-04-27T12:00:00Z",
    events: [],
    page: { hasMoreAfter: false, nextCursor: "c-empty" },
    limitations: [],
  });
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");
  await flushPromises();

  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  assert.match(dashboardHtml, /dashboard-worker-panel/);
  assert.match(dashboardHtml, /lead/);
  assert.match(dashboardHtml, /alice/);
  assert.doesNotMatch(dashboardHtml, /暂无仪表盘数据/);
});

test("dashboard reconciles worker rows from members snapshot during refresh", async () => {
  const payloads = basePayloads();
  const initialMembers = await payloads["/api/teams/demo/members"].json();
  const refreshedMembers = JSON.parse(JSON.stringify(initialMembers));
  refreshedMembers.members[1] = {
    ...refreshedMembers.members[1],
    sessionState: "dead",
    lastActivityAt: "2026-04-27T12:03:00Z",
  };
  payloads["/api/teams/demo/events?cursor=&limit=100"] = okJson({
    teamId: "demo",
    generatedAt: "2026-04-27T12:00:00Z",
    events: [workerEvent("demo", "alice", "alive", "c-stale-worker")],
    page: { hasMoreAfter: false, nextCursor: "c-stale" },
    limitations: [],
  });
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");
  await flushPromises();
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /alice/);

  payloads["/api/teams/demo/members"] = okJson(refreshedMembers);
  await harness.context.refreshCurrentTeam();
  await flushPromises();

  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  const memberListHtml = harness.document.getElementById("memberList").innerHTML;
  const composerHtml = harness.document.getElementById("composerMention").innerHTML;
  assert.match(dashboardHtml, /dash-status-dead/);
  assert.match(memberListHtml, /离线/);
  assert.match(composerHtml, /alice · 已离线/);

  harness.context.renderMemberDetailContent(
    "alice",
    {
      profile: {
        name: "alice",
        kind: "member",
        roleLabel: "worker",
        status: "active",
        joinedAt: "2026-04-23T09:00:00Z",
      },
      execution: {
        executionMode: "codex",
        sessionState: "running",
        adapter: "codex",
        model: "default",
        cwd: "E:/aigc/demo",
        hasSystemPrompt: false,
        envKeys: [],
        redactedEnv: {},
      },
      activity: { lastActivityAt: "2026-04-27T12:00:00Z" },
    },
    { items: [] },
  );
  assert.match(harness.document.getElementById("detailBody").innerHTML, /离线/);
});


test("dashboard re-snapshots worker status when SSE reconnects", async () => {
  const payloads = basePayloads();
  const requests = [];
  const fakeSse = createFakeEventSourceHarness();
  const deadMembers = await payloads["/api/teams/demo/members"].json();
  deadMembers.members[1] = {
    ...deadMembers.members[1],
    sessionState: "dead",
    lastActivityAt: "2026-04-28T10:00:00Z",
  };
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
  await flushPromises();
  source.dispatchError();
  await flushPromises();

  payloads["/api/teams/demo/members"] = okJson(deadMembers);
  source.dispatchOpen();
  await flushPromises();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  assert.ok(requests.filter((url) => url === "/api/teams/demo/members").length >= 2);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /dash-status-dead/);
  assert.match(harness.document.getElementById("memberList").innerHTML, /离线/);
});


test("dashboard falls back to polling after SSE runtime persistent failure", async () => {
  const payloads = basePayloads();
  const requests = [];
  const fakeSse = createFakeEventSourceHarness();
  const deadMembers = await payloads["/api/teams/demo/members"].json();
  deadMembers.members[1] = {
    ...deadMembers.members[1],
    sessionState: "dead",
    lastActivityAt: "2026-04-28T10:00:00Z",
  };
  payloads["/api/teams/demo/events?cursor=c-before-fallback&limit=100"] = okJson({
    teamId: "demo",
    generatedAt: "2026-04-28T10:00:00Z",
    events: [messageEvent("demo", "m-polling-fallback", "Recovered through polling", ["frontend-dev"], "c-after-fallback")],
    page: { hasMoreAfter: false, nextCursor: "c-after-fallback" },
    limitations: [],
  });
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
  source.dispatchEventFrame(messageEvent("demo", "m-sse-before-fallback", "Before fallback", ["frontend-dev"], "c-before-fallback"));
  payloads["/api/teams/demo/members"] = okJson(deadMembers);
  source.dispatchError();
  source.dispatchError();
  source.dispatchError();
  await flushPromises(24);
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  assert.equal(source.closed, true);
  assert.equal(harness.context.state.dashboard.transport.mode, "polling");
  assert.equal(harness.context.state.dashboard.transport.source, "fallback-sse-failure");
  assert.ok(requests.filter((url) => url === "/api/teams/demo/members").length >= 2);
  assert.ok(requests.includes("/api/teams/demo/events?cursor=c-before-fallback&limit=100"));
  assert.equal(harness.context.state.dashboard.connection.cursor, "c-after-fallback");
  assert.match(harness.document.getElementById("liveStatus").textContent, /轮询备用（SSE 失败）/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /dash-status-dead/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /Recovered through polling/);
});
