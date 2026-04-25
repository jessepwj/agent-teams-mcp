import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const appJs = fs.readFileSync(
  path.join(import.meta.dirname, "app.js"),
  "utf8",
);

const FIXED_ELEMENT_IDS = [
  "banner",
  "brandKicker",
  "brandTitle",
  "teamLabel",
  "teamSelect",
  "searchLabel",
  "searchInput",
  "languageToggleButton",
  "liveStatus",
  "reloadButton",
  "roomsTitle",
  "roomList",
  "membersTitle",
  "memberList",
  "filtersTitle",
  "clearFiltersButton",
  "focusLeadButton",
  "filterSummary",
  "workspace",
  "leftSplitter",
  "timelineTitle",
  "timelineSubtitle",
  "timelineStats",
  "messageList",
  "rightSplitter",
  "detailPane",
  "detailPaneTitle",
  "sessionTabButton",
  "detailTabButton",
  "diagnosticsTabButton",
  "detailTitle",
  "detailBody",
  "statusSummary",
  "countsSummary",
];

class FakeElement {
  constructor(id, ownerDocument) {
    this.id = id;
    this.ownerDocument = ownerDocument;
    this.listeners = new Map();
    this.children = [];
    this.hidden = false;
    this.className = "";
    this.textContent = "";
    this.value = "";
    this.title = "";
    this._innerHTML = "";
    this._firstDetailCard = null;
    this.attributes = new Map();
    this.style = {
      values: new Map(),
      setProperty: (name, value) => {
        this.style.values.set(name, value);
      },
    };
    this.classList = {
      add: (name) => {
        const names = new Set(this.className.split(/\s+/).filter(Boolean));
        names.add(name);
        this.className = Array.from(names).join(" ");
      },
      remove: (name) => {
        const names = new Set(this.className.split(/\s+/).filter(Boolean));
        names.delete(name);
        this.className = Array.from(names).join(" ");
      },
      toggle: (name, force) => {
        const shouldAdd = force ?? !this.className.split(/\s+/).includes(name);
        if (shouldAdd) {
          this.classList.add(name);
        } else {
          this.classList.remove(name);
        }
        return shouldAdd;
      },
    };
  }

  get innerHTML() {
    return this._innerHTML;
  }

  set innerHTML(value) {
    this._innerHTML = String(value);
    this.ownerDocument.registerIdsFromMarkup(this._innerHTML);
    if (this._innerHTML.includes('class="detail-card"')) {
      this._firstDetailCard = new FakeElement(null, this.ownerDocument);
    } else {
      this._firstDetailCard = null;
    }
  }

  addEventListener(type, handler) {
    this.listeners.set(type, handler);
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  async dispatch(type, event = {}) {
    const handler = this.listeners.get(type);
    if (!handler) {
      return;
    }
    await handler({
      stopPropagation() {},
      preventDefault() {},
      target: this,
      ...event,
    });
  }

  querySelector(selector) {
    if (selector === ".detail-card") {
      return this._firstDetailCard;
    }
    return null;
  }

  appendChild(node) {
    this.children.push(node);
  }
}

class FakeDocument {
  constructor() {
    this.elements = new Map();
    this.documentElement = { lang: "zh-CN" };
    for (const id of FIXED_ELEMENT_IDS) {
      this.ensureElement(id);
    }
  }

  ensureElement(id) {
    if (!this.elements.has(id)) {
      this.elements.set(id, new FakeElement(id, this));
    }
    return this.elements.get(id);
  }

  getElementById(id) {
    return this.ensureElement(id);
  }

  querySelectorAll() {
    return [];
  }

  createElement() {
    return new FakeElement(null, this);
  }

  registerIdsFromMarkup(markup) {
    const idPattern = /id="([^"]+)"/g;
    let match = null;
    while ((match = idPattern.exec(markup)) !== null) {
      this.ensureElement(match[1]);
    }
  }
}

function okJson(body) {
  return {
    ok: true,
    status: 200,
    statusText: "OK",
    async json() {
      return body;
    },
  };
}

function failedJson(status = 500, statusText = "Internal Server Error") {
  return {
    ok: false,
    status,
    statusText,
    async json() {
      throw new Error("json() should not be called for failed responses");
    },
  };
}

function basePayloads() {
  return {
    "/api/teams": okJson({
      teams: [
        {
          id: "demo",
          name: "Diagnostics Team",
          cwd: "E:/aigc/agent-teams-rs-team-mode",
          status: "active",
          leadMemberId: "lead",
          memberCount: 2,
          activeWorkerCount: 1,
          lastMessageAt: "2026-04-24T00:10:00Z",
        },
      ],
    }),
    "/api/teams/demo": okJson({
      team: {
        id: "demo",
        name: "Diagnostics Team",
        cwd: "E:/aigc/agent-teams-rs-team-mode",
        status: "active",
        leadMemberId: "lead",
        createdAt: "2026-04-23T09:00:00Z",
        updatedAt: "2026-04-24T00:10:00Z",
      },
      counts: {
        memberCount: 2,
        activeWorkerCount: 1,
        messageCount: 2,
        threadCount: 1,
        unreadForLead: 1,
        lastMessageAt: "2026-04-24T00:10:00Z",
      },
    }),
    "/api/teams/demo/rooms/main?limit=200": okJson({
      room: { id: "main", teamId: "demo", status: "active" },
      messages: [
        {
          id: "m1",
          sender: "lead",
          senderKind: "lead",
          kind: "dispatch",
          body: "Please review @alice",
          bodyPreview: "Please review @alice",
          createdAt: "2026-04-24T00:01:00Z",
          mentions: ["alice"],
          effectiveRecipients: ["alice"],
          deliveryStatus: "delivered",
          readCount: 1,
          ackedCount: 0,
          replyTo: null,
          threadId: "t1",
          threadReplyCount: 1,
        },
        {
          id: "m2",
          sender: "alice",
          senderKind: "member",
          kind: "reply",
          body: "Done",
          bodyPreview: "Done",
          createdAt: "2026-04-24T00:10:00Z",
          mentions: ["lead"],
          effectiveRecipients: ["lead"],
          deliveryStatus: "delivered",
          readCount: 0,
          ackedCount: 0,
          replyTo: "m1",
          threadId: "t1",
          threadReplyCount: 0,
        },
      ],
      page: {
        hasMoreBefore: false,
        hasMoreAfter: false,
        nextCursor: null,
      },
    }),
    "/api/teams/demo/members": okJson({
      members: [
        {
          name: "lead",
          kind: "lead",
          roleLabel: "lead",
          status: "active",
          sessionState: "coordinator",
        },
        {
          name: "alice",
          kind: "member",
          roleLabel: "worker",
          status: "active",
          sessionState: "running",
        },
      ],
    }),
    "/api/teams/demo/members/lead": okJson({
      profile: {
        name: "lead",
        kind: "lead",
        roleLabel: "lead",
        status: "active",
        joinedAt: "2026-04-23T09:00:00Z",
      },
      execution: {
        executionMode: "unknown",
        sessionState: "coordinator",
        adapter: null,
        model: null,
        cwd: "E:/aigc/agent-teams-rs-team-mode",
        hasSystemPrompt: false,
        envKeys: [],
        redactedEnv: {},
      },
      activity: {
        sentCount: 1,
        receivedCount: 1,
        mentionedCount: 1,
      },
    }),
    "/api/teams/demo/members/lead/activity": okJson({
      member: "lead",
      source: "derived-from-messages",
      items: [
        {
          itemType: "sent_message",
          messageId: "m1",
          summary: "lead sent a message",
          createdAt: "2026-04-24T00:01:00Z",
        },
      ],
      limitations: ["No stdout/stderr or tool-call events are available yet."],
    }),
    "/api/teams/demo/members/lead/conversation": okJson({
      member: "lead",
      source: {
        provider: "claude-code",
        confidence: "cwd_latest",
        sessionId: "session-1",
        path: "C:/Users/msi/.claude/projects/demo/session-1.jsonl",
        updatedAt: "2026-04-24T00:10:00Z",
        cwd: "E:/aigc/agent-teams-rs-team-mode",
      },
      items: [
        {
          id: "0:0",
          role: "user",
          kind: "text",
          text: "请检查团队状态",
          timestamp: "2026-04-24T00:01:00Z",
        },
        {
          id: "1:0",
          role: "assistant",
          kind: "text",
          text: "团队状态正常。",
          timestamp: "2026-04-24T00:02:00Z",
        },
        {
          id: "2:0",
          role: "tool",
          kind: "tool_use",
          title: "Read",
          toolUseId: "tool-1",
          toolName: "Read",
          input: { file_path: "src/team_mode_web/read_model.rs" },
          text: "{\"path\":\"src/team_mode_web/read_model.rs\"}",
          timestamp: "2026-04-24T00:03:00Z",
        },
        {
          id: "3:0",
          role: "tool",
          kind: "tool_result",
          title: "Tool result tool-1",
          toolUseId: "tool-1",
          result: "pub fn read_member_conversation() {}",
          text: "pub fn read_member_conversation() {}",
          timestamp: "2026-04-24T00:03:01Z",
        },
      ],
      limitations: [
        "The session is matched by cwd and latest modified Claude Code JSONL file.",
      ],
    }),
    "/api/teams/demo/diagnostics": okJson({
      teamId: "demo",
      teamName: "Diagnostics Team",
      cwd: "E:/aigc/agent-teams-rs-team-mode",
      generatedAt: "2026-04-24T00:12:00Z",
      limitations: [
        "These diagnostics are file/session-level observations, not per-member stdout/stderr.",
      ],
      sources: [
        {
          id: "mcp_log",
          label: "MCP Log",
          kind: "file",
          path: "E:/aigc/agent-teams-rs-team-mode/.agent-teams/mcp.log",
          exists: true,
          sizeBytes: 128,
          updatedAt: "2026-04-24T00:11:00Z",
          preview: "15:51:17 INFO team_mode_mcp: Team Mode MCP server starting",
        },
      ],
      leadSession: {
        discovered: true,
        sessionCount: 1,
        latestSessionId: "session-1",
        latestModifiedAt: "2026-04-24T00:10:00Z",
        sourcePath: "C:/Users/msi/.claude/projects/demo/session-1.jsonl",
        recentToolCalls: [
          {
            toolName: "Read",
            inputSummary: "{\"path\":\"src/team_mode_web/read_model.rs\"}",
            timestamp: "2026-04-24T00:09:00Z",
          },
        ],
        tokenUsage: {
          inputTokens: 111,
          outputTokens: 222,
          cacheReadTokens: 33,
          cacheWriteTokens: 44,
          totalTokens: 410,
        },
        limitations: [
          "Lead session diagnostics sample Claude session files only; they do not expose per-member stdout/stderr.",
        ],
      },
    }),
  };
}

function createHarness({ hash = "", fetchImpl } = {}) {
  const document = new FakeDocument();
  const windowListeners = new Map();
  const location = { hash };

  const context = vm.createContext({
    console,
    document,
    window: {
      location,
      addEventListener(type, handler) {
        windowListeners.set(type, handler);
      },
      setInterval() {
        return 1;
      },
      clearInterval() {},
    },
    location,
    fetch: async (url, options) => fetchImpl(url, options),
    URLSearchParams,
    setTimeout,
    clearTimeout,
    setInterval() {
      return 1;
    },
    clearInterval() {},
  });

  vm.runInContext(appJs, context, { filename: "web/team-mode/app.js" });

  return {
    document,
    location,
    context,
    async start() {
      const handler = windowListeners.get("DOMContentLoaded");
      assert.ok(handler, "DOMContentLoaded handler should be registered");
      await handler();
      await flushPromises();
    },
  };
}

async function flushPromises(times = 4) {
  for (let index = 0; index < times; index += 1) {
    await Promise.resolve();
  }
}

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

test("defaults to Chinese and can switch to English", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    hash: "#member=lead",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  assert.equal(harness.document.documentElement.lang, "zh-CN");
  assert.equal(harness.document.getElementById("brandTitle").textContent, "团队模式");
  assert.equal(harness.document.getElementById("detailTitle").textContent, "进程会话 · lead");

  await harness.document.getElementById("languageToggleButton").dispatch("click");
  await flushPromises();

  assert.equal(harness.document.documentElement.lang, "en");
  assert.equal(harness.document.getElementById("brandTitle").textContent, "Team Mode");
  assert.equal(harness.document.getElementById("detailTitle").textContent, "Process Session · lead");
  assert.match(harness.document.getElementById("detailBody").innerHTML, /团队状态正常/);
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

test("balances the chat and detail panes by default", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });
  harness.document.getElementById("workspace").clientWidth = 1260;

  await harness.start();

  const workspaceStyle = harness.document.getElementById("workspace").style.values;
  assert.equal(workspaceStyle.get("--left-pane-width"), "260px");
  assert.equal(workspaceStyle.get("--right-pane-width"), "494px");
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

test("process conversation pairs tool calls into collapsible rows", async () => {
  const payloads = basePayloads();

  const harness = createHarness({
    hash: "#member=lead",
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();

  const html = harness.document.getElementById("detailBody").innerHTML;
  assert.match(html, /assistant-turn/);
  assert.match(html, /message-user-prompt/);
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
