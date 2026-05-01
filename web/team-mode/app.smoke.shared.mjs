import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";

const APP_SCRIPT_ORDER = [
  "app-state.js",
  "app-api.js",
  "app-utils.js",
  "app-diagnostics.js",
  "app-render.js",
  "app-conversation.js",
  "app-dashboard-render.js",
  "app-dashboard.js",
  "app.js",
];

function readAppScript(fileName) {
  const source = fs.readFileSync(path.join(import.meta.dirname, fileName), "utf8");
  return fileName === "app.js" ? source.replace(/^import\s+["'][^"']+["'];\n/gm, "") : source;
}

const appJs = APP_SCRIPT_ORDER.map(readAppScript).join("\n");

const FIXED_ELEMENT_IDS = [
  "banner",
  "brandKicker",
  "brandTitle",
  "teamLabel",
  "teamSelect",
  "searchLabel",
  "searchInput",
  "chatViewButton",
  "dashboardViewButton",
  "languageToggleButton",
  "liveStatus",
  "reloadButton",
  "mainStage",
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
  "composerForm",
  "composerMentionLabel",
  "composerMentionPrefix",
  "composerMention",
  "composerInput",
  "composerSend",
  "composerStatus",
  "rightSplitter",
  "detailPane",
  "dashboardWorkspace",
  "dashboardRoot",
  "detailPaneTitle",
  "sessionTabButton",
  "detailTabButton",
  "diagnosticsTabButton",
  "detailTitle",
  "detailBody",
  "statusSummary",
  "countsSummary",
  "bundleRevisionSummary",
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
    this.dataset = {};
    this.disabled = false;
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

  getAttribute(name) {
    return this.attributes.get(name) || null;
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

  querySelector(selector) {
    if (selector === 'meta[name="bundle-revision"]') {
      const meta = this.ensureElement("__bundleRevisionMeta");
      meta.setAttribute("content", "test-bundle-rev");
      return meta;
    }
    return null;
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

function deferredOkJson(body) {
  let resolveResponse = null;
  const responsePromise = new Promise((resolve) => {
    resolveResponse = () => resolve(okJson(body));
  });
  return { responsePromise, resolveResponse };
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
    "/api/teams/demo/events?cursor=&limit=100": okJson({
      teamId: "demo",
      generatedAt: "2026-04-27T12:00:00Z",
      events: [
        workerEvent("demo", "backend-dev", "alive", "c-worker-1"),
        messageEvent("demo", "m-task-1", "Implement dashboard polling", ["frontend-dev"], "c-message-1"),
      ],
      page: { hasMoreAfter: false, nextCursor: "c-initial" },
      limitations: [],
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
        {
          id: "4:0",
          role: "assistant",
          kind: "text",
          text: "读取完成，团队状态正常。",
          timestamp: "2026-04-24T00:04:00Z",
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

function workerEvent(teamId, workerName, lifecycleEvent, cursor) {
  return {
    id: `${teamId}:worker:${workerName}:${cursor}`,
    teamId,
    eventType: "workerStatusChanged",
    occurredAt: "2026-04-27T12:00:00Z",
    source: "runtimeWorkers",
    cursor,
    payload: {
      workerName,
      lifecycleEvent,
      sessionState: lifecycleEvent,
      previousSessionState: null,
      adapter: "codex",
      model: null,
      cwd: null,
      note: "",
    },
  };
}

function messageEvent(teamId, messageId, body, mentions, cursor, overrides = {}) {
  const sender = overrides.sender || "lead";
  const senderKind = overrides.senderKind || "lead";
  const kind = overrides.kind || "dispatch";
  return {
    id: `${teamId}:message:${messageId}`,
    teamId,
    eventType: "messageCreated",
    occurredAt: "2026-04-27T12:00:01Z",
    source: "messages",
    cursor,
    payload: {
      message: {
        id: messageId,
        sender,
        senderKind,
        kind,
        body,
        bodyPreview: body,
        createdAt: "2026-04-27T12:00:01Z",
        mentions,
        effectiveRecipients: mentions,
        replyTo: overrides.replyTo || null,
        threadId: overrides.threadId || null,
        ...overrides.message,
      },
    },
  };
}

function fileEvent(teamId, pathName, changeKind, cursor) {
  return {
    id: `${teamId}:file:${cursor}`,
    teamId,
    eventType: "fileChanged",
    occurredAt: "2026-04-27T12:00:02Z",
    source: "filesystem",
    cursor,
    payload: {
      path: pathName,
      changeKind,
    },
  };
}

function createFakeEventSourceHarness() {
  const instances = [];
  class FakeEventSource {
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      this.closed = false;
      this.listeners = new Map();
      instances.push(this);
    }

    addEventListener(type, handler) {
      const handlers = this.listeners.get(type) || [];
      handlers.push(handler);
      this.listeners.set(type, handlers);
    }

    dispatchOpen() {
      this.readyState = 1;
      this.onopen?.({ type: "open" });
    }

    dispatchError() {
      this.readyState = 0;
      this.onerror?.({ type: "error" });
    }

    dispatchEventFrame(event) {
      const frame = {
        type: event.eventType,
        data: JSON.stringify(event),
        lastEventId: event.cursor || event.id || "",
      };
      for (const handler of this.listeners.get(event.eventType) || []) {
        handler(frame);
      }
    }

    dispatchHeartbeat(cursor = "") {
      const frame = { type: "heartbeat", data: "{}", lastEventId: cursor };
      for (const handler of this.listeners.get("heartbeat") || []) {
        handler(frame);
      }
    }

    close() {
      this.closed = true;
      this.readyState = 2;
    }
  }
  return { FakeEventSource, instances };
}

function createHarness({ hash = "", search = "", fetchImpl } = {}) {
  const document = new FakeDocument();
  const windowListeners = new Map();
  const location = { hash, search };

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

async function flushPromises(times = 8) {
  for (let index = 0; index < times; index += 1) {
    await Promise.resolve();
  }
}

async function flushTimers() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

export {
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
};
