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

test("keeps the timeline pinned to bottom when new messages arrive", async () => {
  const payloads = basePayloads();
  const roomPayload = await payloads["/api/teams/demo/rooms/main?limit=200"].json();
  payloads["/api/teams/demo/rooms/main?limit=200"] = okJson(roomPayload);
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  await flushTimers();

  const messageList = harness.document.getElementById("messageList");
  messageList.clientHeight = 100;
  messageList.scrollHeight = 1000;
  messageList.scrollTop = 900;
  roomPayload.messages.push({
    id: "m3",
    sender: "lead",
    senderKind: "lead",
    kind: "dispatch",
    body: "New bottom message",
    bodyPreview: "New bottom message",
    createdAt: "2026-04-24T00:11:00Z",
    mentions: ["alice"],
    effectiveRecipients: ["alice"],
    deliveryStatus: "delivered",
    readCount: 0,
    ackedCount: 0,
    replyTo: null,
    threadId: "t2",
    threadReplyCount: 0,
  });

  await harness.context.refreshCurrentTeam();
  messageList.scrollHeight = 1300;
  await flushTimers();

  assert.equal(messageList.scrollTop, 1300);
});

test("preserves timeline scroll when reading older messages during refresh", async () => {
  const payloads = basePayloads();
  const roomPayload = await payloads["/api/teams/demo/rooms/main?limit=200"].json();
  payloads["/api/teams/demo/rooms/main?limit=200"] = okJson(roomPayload);
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  await flushTimers();

  const messageList = harness.document.getElementById("messageList");
  messageList.clientHeight = 100;
  messageList.scrollHeight = 1000;
  messageList.scrollTop = 240;
  roomPayload.messages.push({
    id: "m3",
    sender: "lead",
    senderKind: "lead",
    kind: "dispatch",
    body: "New message while reading history",
    bodyPreview: "New message while reading history",
    createdAt: "2026-04-24T00:11:00Z",
    mentions: ["alice"],
    effectiveRecipients: ["alice"],
    deliveryStatus: "delivered",
    readCount: 0,
    ackedCount: 0,
    replyTo: null,
    threadId: "t2",
    threadReplyCount: 0,
  });

  await harness.context.refreshCurrentTeam();
  messageList.scrollHeight = 1300;
  await flushTimers();

  assert.equal(messageList.scrollTop, 240);
});


test("shows a clear state for a missing message deep link", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    hash: "#message=missing",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  assert.equal(harness.document.getElementById("detailTitle").textContent, "找不到消息");
  assert.match(harness.document.getElementById("detailBody").innerHTML, /missing/);
});

test("restores deep link to a specific team on load", async () => {
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
        lastMessageAt: "2026-04-24T00:10:00Z",
      },
      {
        id: "beta",
        name: "Beta Team",
        cwd: "E:/aigc/beta",
        status: "active",
        leadMemberId: "lead",
        memberCount: 1,
        activeWorkerCount: 0,
        lastMessageAt: null,
      },
    ],
  });
  payloads["/api/teams/beta"] = okJson({
    team: {
      id: "beta",
      name: "Beta Team",
      cwd: "E:/aigc/beta",
      status: "active",
      leadMemberId: "lead",
      createdAt: "2026-04-24T00:00:00Z",
      updatedAt: "2026-04-24T00:00:00Z",
    },
    counts: {
      memberCount: 1,
      activeWorkerCount: 0,
      messageCount: 0,
      threadCount: 0,
      unreadForLead: 0,
      lastMessageAt: null,
    },
  });
  payloads["/api/teams/beta/rooms/main?limit=200"] = okJson({
    room: { id: "main", teamId: "beta", status: "active" },
    messages: [],
    page: { hasMoreBefore: false, hasMoreAfter: false, nextCursor: null },
  });
  payloads["/api/teams/beta/members"] = okJson({
    members: [
      {
        name: "lead",
        kind: "lead",
        roleLabel: "lead",
        status: "active",
        sessionState: "coordinator",
      },
    ],
  });
  payloads["/api/teams/beta/members/lead"] = payloads["/api/teams/demo/members/lead"];
  payloads["/api/teams/beta/members/lead/activity"] = payloads["/api/teams/demo/members/lead/activity"];
  payloads["/api/teams/beta/members/lead/conversation"] = payloads["/api/teams/demo/members/lead/conversation"];
  payloads["/api/teams/beta/diagnostics"] = payloads["/api/teams/demo/diagnostics"];

  const harness = createHarness({
    hash: "#team=beta&member=lead",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  assert.equal(harness.document.getElementById("teamSelect").value, "beta");
  assert.match(harness.document.getElementById("timelineSubtitle").textContent, /Beta Team/);
  assert.equal(harness.document.getElementById("detailTitle").textContent, "进程会话 · lead");
});

test("renders diagnostics sections with source previews and lead session summary", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    hash: "#member=lead",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  await harness.document.getElementById("diagnosticsTabButton").dispatch("click");
  await flushPromises();

  const detailBody = harness.document.getElementById("detailBody").innerHTML;
  assert.equal(harness.document.getElementById("detailTitle").textContent, "团队诊断");
  assert.match(detailBody, /团队诊断/);
  assert.match(detailBody, /诊断来源/);
  assert.match(detailBody, /负责人会话诊断/);
  assert.match(detailBody, /MCP 日志/);
  assert.match(detailBody, /128 字节/);
  assert.match(detailBody, /session-1/);
  assert.match(detailBody, /Read/);
  assert.match(
    detailBody,
    /文件\/会话级观察结果/,
  );
});

test("renders diagnostics even when lead session data is absent", async () => {
  const payloads = basePayloads();
  payloads["/api/teams/demo/diagnostics"] = okJson({
    teamId: "demo",
    teamName: "Diagnostics Team",
    cwd: "E:/aigc/agent-teams-rs-team-mode",
    generatedAt: "2026-04-24T00:12:00Z",
    limitations: [],
    sources: [],
  });
  const harness = createHarness({
    hash: "#member=lead",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  await harness.document.getElementById("diagnosticsTabButton").dispatch("click");
  await flushPromises();

  const detailBody = harness.document.getElementById("detailBody").innerHTML;
  assert.match(detailBody, /负责人会话诊断/);
  assert.match(detailBody, /未解析到最近工具调用/);
});

test("message detail keeps the full thread when timeline filters narrow the visible list", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    hash: "#message=m2",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  const searchInput = harness.document.getElementById("searchInput");
  searchInput.value = "Done";
  await searchInput.dispatch("input");
  await flushPromises();

  const detailBody = harness.document.getElementById("detailBody");
  assert.match(detailBody.innerHTML, /Please review @alice/);
  assert.match(detailBody.innerHTML, /Done/);

  const rootSummary = detailBody.querySelector(".detail-card");
  assert.ok(rootSummary, "message detail should keep a root summary card");
  const rootFooter = rootSummary.children.at(-1);
  assert.equal(rootFooter?.textContent, "线程根消息：m1");
});

test("surfaces refresh failure and allows retry to recover team data", async () => {
  const payloads = basePayloads();
  let failTeamLoad = false;

  const harness = createHarness({
    fetchImpl: async (url) => {
      if (
        failTeamLoad &&
        [
          "/api/teams/demo",
          "/api/teams/demo/rooms/main?limit=200",
          "/api/teams/demo/members",
        ].includes(url)
      ) {
        return failedJson(500, "Server Error");
      }
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });

  await harness.start();
  failTeamLoad = true;

  await harness.document.getElementById("reloadButton").dispatch("click");
  await flushPromises();

  assert.match(
    harness.document.getElementById("banner").textContent,
    /加载团队数据失败/,
  );
  assert.match(
    harness.document.getElementById("countsSummary").textContent,
    /刷新失败/,
  );

  failTeamLoad = false;
  await harness.document.getElementById("retryDetailButton").dispatch("click");
  await flushPromises();

  assert.equal(harness.document.getElementById("banner").hidden, true);
  assert.equal(harness.document.getElementById("detailTitle").textContent, "进程会话 · lead");
  assert.doesNotMatch(
    harness.document.getElementById("countsSummary").textContent,
    /刷新失败/,
  );
});

test("focusing lead opens the process conversation", async () => {
  const payloads = basePayloads();

  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  await harness.document.getElementById("focusLeadButton").dispatch("click");
  await flushPromises();
  assert.equal(harness.document.getElementById("detailTitle").textContent, "进程会话 · lead");
  assert.match(harness.document.getElementById("detailBody").innerHTML, /团队状态正常/);
});

test("member conversation opens scrolled to the bottom by default", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    hash: "#member=lead",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });
  const detailPane = harness.document.getElementById("detailPane");
  detailPane.clientHeight = 100;
  detailPane.scrollHeight = 1000;
  detailPane.scrollTop = 0;

  await harness.start();
  detailPane.scrollHeight = 1400;
  await flushTimers();

  assert.equal(detailPane.scrollTop, 1400);
});

test("process conversation pairs tool calls into collapsible rows", async () => {
  const payloads = basePayloads();

  const harness = createHarness({
    hash: "#member=lead",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  const html = harness.document.getElementById("detailBody").innerHTML;
  assert.match(html, /conversation-work-turn/);
  assert.match(html, /assistant-turn/);
  assert.match(html, /message-user-prompt/);
  assert.match(html, /最终回复/);
  assert.match(html, /final-reply-block/);
  assert.match(html, /tool-row/);
  assert.match(html, /Read/);
  assert.match(html, /read_model\.rs/);
  assert.match(html, /pub fn read_member_conversation/);
});

test("keeps cached member detail when background member refresh fails", async () => {
  const payloads = basePayloads();
  let failLeadDetail = false;

  const harness = createHarness({
    hash: "#member=lead",
    fetchImpl: async (url) => {
      if (
        failLeadDetail &&
        [
          "/api/teams/demo/members/lead",
          "/api/teams/demo/members/lead/activity",
        ].includes(url)
      ) {
        return failedJson(500, "Server Error");
      }
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });

  await harness.start();
  await harness.document.getElementById("detailTabButton").dispatch("click");
  await flushPromises();
  assert.equal(harness.document.getElementById("detailTitle").textContent, "负责人活动");

  failLeadDetail = true;
  await harness.context.refreshMemberDetail("lead", { force: true });
  await flushPromises();

  assert.equal(harness.document.getElementById("detailTitle").textContent, "负责人活动");
  assert.doesNotMatch(
    harness.document.getElementById("detailBody").innerHTML,
    /加载成员详情失败/,
  );
  assert.match(
    harness.document.getElementById("countsSummary").textContent,
    /刷新失败/,
  );
});
