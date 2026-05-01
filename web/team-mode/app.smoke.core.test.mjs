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

test("shows explicit empty states when no teams are available", async () => {
  const harness = createHarness({
    fetchImpl: async (url) => {
      if (url === "/api/teams") {
        return okJson({ teams: [] });
      }
      throw new Error(`unexpected request: ${url}`);
    },
  });

  await harness.start();

  assert.equal(harness.document.getElementById("banner").hidden, false);
  assert.equal(harness.document.getElementById("banner").textContent, "没有团队");
  assert.match(harness.document.getElementById("roomList").innerHTML, /没有团队/);
  assert.match(harness.document.getElementById("memberList").innerHTML, /没有成员/);
  assert.match(harness.document.getElementById("messageList").innerHTML, /没有消息/);
  assert.match(
    harness.document.getElementById("teamSelect").innerHTML,
    /没有团队/,
  );
});

test("restores deep link to a message detail on load", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    hash: "#message=m1",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  assert.equal(harness.document.getElementById("detailTitle").textContent, "消息 m1");
  assert.match(harness.document.getElementById("detailBody").innerHTML, /路由/);
});

test("restores deep link to lead activity on load", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    hash: "#member=lead",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  assert.equal(harness.document.getElementById("detailTitle").textContent, "进程会话 · lead");
  assert.match(
    harness.document.getElementById("detailBody").innerHTML,
    /团队状态正常/,
  );
  assert.doesNotMatch(
    harness.document.getElementById("detailBody").innerHTML,
    /团队诊断/,
  );
});


test("supports resizing panes with the separator controls", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  await harness.document.getElementById("leftSplitter").dispatch("keydown", { key: "ArrowRight" });
  await harness.document.getElementById("rightSplitter").dispatch("keydown", { key: "ArrowLeft" });

  const workspaceStyle = harness.document.getElementById("workspace").style.values;
  assert.equal(workspaceStyle.get("--left-pane-width"), "276px");
  assert.equal(workspaceStyle.get("--right-pane-width"), "376px");
});

test("uses fluid CSS column widths by default", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });
  harness.document.getElementById("workspace").clientWidth = 1260;

  await harness.start();

  const workspaceStyle = harness.document.getElementById("workspace").style.values;
  assert.equal(workspaceStyle.get("--left-pane-width"), undefined);
  assert.equal(workspaceStyle.get("--right-pane-width"), undefined);
});

test("renders the timeline as a simple group chat", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  const messageList = harness.document.getElementById("messageList").innerHTML;
  assert.match(messageList, /chat-message/);
  assert.match(messageList, /chat-avatar/);
  assert.match(messageList, /chat-bubble/);
  assert.match(messageList, /Please review/);
  assert.match(messageList, /@alice/);
  assert.match(messageList, /Done/);
  assert.doesNotMatch(messageList, /已投递|delivered|派发|dispatch|回复|reply|1 线程/);

  assert.match(harness.document.getElementById("liveStatus").textContent, /轮询备用/);
  assert.equal(harness.document.getElementById("dashboardWorkspace").hidden, true);
  await harness.document.getElementById("dashboardViewButton").dispatch("click");
  await flushPromises();

  const dashboardRoot = harness.document.getElementById("dashboardRoot");
  assert.equal(harness.document.getElementById("workspace").hidden, true);
  assert.equal(harness.document.getElementById("dashboardWorkspace").hidden, false);
  assert.match(dashboardRoot.innerHTML, /dashboard-worker-panel/);
  assert.match(dashboardRoot.innerHTML, /backend-dev/);
  assert.match(dashboardRoot.innerHTML, /Implement dashboard polling/);
  assert.match(dashboardRoot.innerHTML, /task-progress-svg/);
  assert.match(dashboardRoot.innerHTML, /轮询备用/);

  harness.context.setDashboardMockState("loading");
  harness.context.renderDashboardShell();
  assert.match(dashboardRoot.innerHTML, /正在加载仪表盘/);
  harness.context.setDashboardMockState("error", { error: "mock dashboard failure" });
  harness.context.renderDashboardShell();
  assert.match(dashboardRoot.innerHTML, /mock dashboard failure/);
  harness.context.setDashboardMockState("ready", { data: { workers: [], agents: [] } });
  harness.context.renderDashboardShell();
  assert.match(dashboardRoot.innerHTML, /暂无仪表盘数据/);
});

test("timeline can expand truncated group messages", async () => {
  const payloads = basePayloads();
  const roomPayload = await payloads["/api/teams/demo/rooms/main?limit=200"].json();
  roomPayload.messages.push({
    id: "m-long",
    sender: "alice",
    senderKind: "member",
    kind: "reply",
    body: "This is the complete worker response with details that must remain available in the group chat.",
    bodyPreview: "This is the complete worker response...",
    createdAt: "2026-04-24T00:11:00Z",
    mentions: ["lead"],
    effectiveRecipients: ["lead"],
    deliveryStatus: "delivered",
    readCount: 0,
    ackedCount: 0,
    replyTo: "m1",
    threadId: "t1",
    threadReplyCount: 0,
  });
  payloads["/api/teams/demo/rooms/main?limit=200"] = okJson(roomPayload);
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  let messageList = harness.document.getElementById("messageList").innerHTML;
  assert.match(messageList, /展开全文/);
  assert.match(messageList, /This is the complete worker response\.\.\./);
  assert.doesNotMatch(messageList, /details that must remain available/);

  harness.context.state.timelineExpandedMessages.add("m-long");
  harness.context.renderTimeline();

  messageList = harness.document.getElementById("messageList").innerHTML;
  assert.match(messageList, /收起/);
  assert.match(messageList, /details that must remain available/);
});

test("member list prioritizes lead and marks stale running workers idle", async () => {
  const payloads = basePayloads();
  const membersPayload = await payloads["/api/teams/demo/members"].json();
  membersPayload.members = [
    {
      name: "alice",
      kind: "member",
      roleLabel: "worker",
      status: "active",
      sessionState: "running",
      lastActivityAt: "2000-01-01T00:00:00Z",
    },
    {
      name: "lead",
      kind: "lead",
      roleLabel: "lead",
      status: "active",
      sessionState: "coordinator",
      lastActivityAt: "2026-04-24T00:10:00Z",
    },
  ];
  payloads["/api/teams/demo/members"] = okJson(membersPayload);
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  const memberList = harness.document.getElementById("memberList").innerHTML;
  assert.ok(memberList.indexOf("lead") < memberList.indexOf("alice"));
  assert.match(memberList, /空闲/);
});

test("session tab defaults to lead when no member is selected", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    hash: "#message=m1",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  await harness.document.getElementById("sessionTabButton").dispatch("click");
  await flushPromises();

  assert.equal(harness.context.state.selectedMemberName, "lead");
  assert.equal(harness.document.getElementById("detailTitle").textContent, "进程会话 · lead");
});
