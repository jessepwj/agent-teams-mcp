const REFRESH_INTERVAL_MS = 2000;
const COLUMN_WIDTHS_STORAGE_KEY = "team-mode-web-column-widths";
const DEFAULT_COLUMN_WIDTHS = { left: 260, right: null };
const FALLBACK_RIGHT_PANE_WIDTH = 360;
const TIMELINE_BOTTOM_THRESHOLD_PX = 120;
const COLUMN_LIMITS = {
  left: { min: 180, max: 520 },
  right: { min: 260, max: 680 },
};

const state = {
  teams: [],
  teamId: null,
  teamDetail: null,
  room: null,
  members: [],
  selectedMessageId: null,
  selectedMemberName: null,
  search: "",
  senderFilter: "",
  mentionFilter: "",
  loadingTeams: false,
  loadingTeam: false,
  teamError: "",
  detailError: "",
  refreshError: "",
  diagnostics: null,
  diagnosticsError: "",
  diagnosticsLoading: false,
  deepLink: null,
  failedTeamId: null,
  memberDetailKey: "",
  memberDetailLoadingKey: "",
  memberDetail: null,
  memberActivity: null,
  memberConversationKey: "",
  memberConversationLoadingKey: "",
  memberConversationScrollKey: "",
  memberConversation: null,
  detailRequestSeq: 0,
  language: "zh",
  composerMention: "",
  composerSending: false,
  timelineForceScrollBottom: false,
  columnWidths: loadColumnWidths(),
  detailTab: "session",
};

const $ = (id) => document.getElementById(id);
let refreshTimer = null;

const STRINGS = {
  zh: {
    brandKicker: "团队模式 Web",
    brandTitle: "团队模式",
    team: "团队",
    search: "搜索",
    searchPlaceholder: "筛选消息、成员或提及",
    switchLanguage: "English",
    languageToggleTitle: "切换语言",
    reload: "刷新",
    rooms: "房间",
    members: "成员",
    filters: "筛选",
    resetFilters: "重置筛选",
    leadActivity: "负责人活动",
    noFilters: "未启用筛选。",
    timeline: "群聊",
    selectTeam: "选择团队以加载消息。",
    detailPane: "详情面板",
    sessionTab: "会话",
    detailTab: "详情",
    diagnosticsTab: "诊断",
    processSession: "进程会话",
    loadingConversation: "正在加载进程会话...",
    noConversation: "没有找到可展示的进程会话。",
    conversationSource: "会话来源",
    sessionFile: "会话文件",
    matchedBy: "匹配方式",
    sessionSetup: "会话初始化",
    workTurn: "工作轮次",
    receivedInput: "收到输入",
    hookInput: "Hook 输入",
    executionSteps: "执行步骤",
    finalReply: "最终回复",
    noFinalReply: "暂无最终回复",
    thinking: "思考",
    running: "运行中",
    complete: "完成",
    failed: "失败",
    interrupted: "已中断",
    showDetails: "展开详情",
    hideDetails: "收起详情",
    copy: "复制",
    copied: "已复制",
    noSelection: "未选择",
    pickMessageOrMember: "请选择消息或成员。",
    readOnly: "只读",
    noTeams: "没有团队",
    noMembers: "没有成员",
    noMessages: "没有消息",
    loadingTeams: "正在加载团队...",
    loadingTeamData: "正在加载团队数据...",
    loadingMessages: "正在加载消息...",
    loadingDiagnostics: "正在加载诊断信息...",
    loadingMemberDetail: "正在加载成员详情...",
    loadingSelectedMessage: "正在加载选中的消息...",
    noTeamsAvailable: "没有可用团队。",
    noMessagesMatch: "没有消息匹配当前筛选条件。",
    noDiagnosticSources: "没有可用诊断来源。",
    noDiagnosticsLoaded: "暂无诊断信息。",
    noRecentToolCalls: "未解析到最近工具调用。",
    noTokenUsage: "未从最新会话解析到 token 用量。",
    none: "无",
    na: "无",
    unknown: "未知",
    available: "可用",
    missing: "缺失",
    ready: "就绪",
    loading: "加载中",
    composerSend: "发送",
    composerSending: "发送中...",
    composerPlaceholder: "输入消息，回车发送，Shift+回车换行",
    composerEmpty: "请输入要发送的消息。",
    composerNoTeam: "请先选择团队再发送。",
    composerSentTo: "已发送给",
    composerSendFailed: "发送失败：",
    error: "错误",
    refreshFailedFlag: "刷新失败",
    livePrefix: "状态",
    sender: "发送者",
    mentioned: "提及",
    searchFilter: "搜索",
    visible: "可见",
    total: "总计",
    messagesVisible: "条消息可见",
    teamLoadFailed: "团队加载失败",
    teamDiagnostics: "团队诊断",
    diagnostics: "诊断信息",
    diagnosticsSources: "诊断来源",
    leadSessionDiagnostics: "负责人会话诊断",
    limitations: "限制说明",
    teamId: "团队 ID",
    generatedAt: "生成时间",
    teamName: "团队名称",
    cwd: "工作目录",
    discovered: "已发现",
    sessionCount: "会话数量",
    latestSession: "最新会话",
    latestModified: "最近修改",
    sourcePath: "来源路径",
    recentToolCalls: "最近工具调用",
    tokenUsage: "Token 用量",
    inputTokens: "输入 Token",
    outputTokens: "输出 Token",
    cacheRead: "缓存读取",
    cacheWrite: "缓存写入",
    totalTokens: "总 Token",
    yes: "是",
    no: "否",
    retryDiagnostics: "重试诊断",
    retry: "重试",
    loadError: "加载错误",
    messageNotFound: "找不到消息",
    message: "消息",
    messageUnit: "条消息",
    messageMissing: "当前时间线中没有该消息。",
    teamLabel: "团队",
    routing: "路由",
    kind: "类型",
    delivery: "投递",
    replyTo: "回复至",
    threadId: "线程 ID",
    mentions: "提及",
    effectiveRecipients: "有效接收者",
    threadMessages: "线程消息",
    rawJson: "原始 JSON",
    threadRoot: "线程根消息",
    read: "已读",
    acked: "已确认",
    thread: "线程",
    member: "成员",
    leadCoordination: "负责人协作",
    profile: "档案",
    executionSnapshot: "执行快照",
    executionMode: "执行模式",
    sessionState: "会话状态",
    adapter: "适配器",
    model: "模型",
    systemPrompt: "系统提示词",
    folded: "已折叠",
    environment: "环境变量",
    recentActivity: "最近活动",
    sent: "已发送",
    received: "已接收",
    role: "角色",
    status: "状态",
    name: "名称",
    foldedValue: "已折叠",
    noInputSummary: "无输入摘要",
    diagnosticsUnavailable: "诊断不可用",
    noTeamForDiagnostics: "请选择团队以查看诊断信息。",
    failedLoadTeams: "加载团队失败",
    failedLoadTeamData: "加载团队数据失败",
    failedLoadTeamDetail: "加载团队详情失败",
    failedLoadMemberDetail: "加载成员详情失败",
    refreshFailed: "刷新失败",
    bytes: "字节",
    resizeLeftPane: "拖动调整左侧栏宽度",
    resizeRightPane: "拖动调整详情栏宽度",
  },
  en: {
    brandKicker: "Team Mode Web",
    brandTitle: "Team Mode",
    team: "Team",
    search: "Search",
    searchPlaceholder: "Filter messages, members, or mentions",
    switchLanguage: "中文",
    languageToggleTitle: "Switch language",
    reload: "Reload",
    rooms: "Rooms",
    members: "Members",
    filters: "Filters",
    resetFilters: "Reset filters",
    leadActivity: "Lead Activity",
    noFilters: "No filters active.",
    timeline: "Group Chat",
    selectTeam: "Select a team to load messages.",
    detailPane: "Detail Pane",
    sessionTab: "Session",
    detailTab: "Details",
    diagnosticsTab: "Diagnostics",
    processSession: "Process Session",
    loadingConversation: "Loading process session...",
    noConversation: "No process conversation was found.",
    conversationSource: "Session Source",
    sessionFile: "Session File",
    matchedBy: "Matched By",
    sessionSetup: "Session setup",
    workTurn: "Work turn",
    receivedInput: "Received input",
    hookInput: "Hook input",
    executionSteps: "Execution steps",
    finalReply: "Final reply",
    noFinalReply: "No final reply",
    thinking: "Thinking",
    running: "Running",
    complete: "Complete",
    failed: "Failed",
    interrupted: "Interrupted",
    showDetails: "Show details",
    hideDetails: "Hide details",
    copy: "Copy",
    copied: "Copied",
    noSelection: "No selection",
    pickMessageOrMember: "Pick a message or member.",
    readOnly: "Read only",
    noTeams: "no teams",
    noMembers: "no members",
    noMessages: "no messages",
    loadingTeams: "Loading teams...",
    loadingTeamData: "Loading team data...",
    loadingMessages: "Loading messages...",
    loadingDiagnostics: "Loading diagnostics...",
    loadingMemberDetail: "Loading member detail...",
    loadingSelectedMessage: "Loading selected message...",
    noTeamsAvailable: "No teams available.",
    noMessagesMatch: "No messages match the current filters.",
    noDiagnosticSources: "No diagnostic sources available.",
    noDiagnosticsLoaded: "No diagnostics loaded.",
    noRecentToolCalls: "No recent tool calls parsed.",
    noTokenUsage: "No token usage parsed from the latest session.",
    none: "none",
    na: "n/a",
    unknown: "unknown",
    available: "available",
    missing: "missing",
    ready: "ready",
    composerSend: "Send",
    composerSending: "Sending...",
    composerPlaceholder: "Type a message — Enter to send, Shift+Enter for newline",
    composerEmpty: "Type something before sending.",
    composerNoTeam: "Pick a team first.",
    composerSentTo: "Sent to",
    composerSendFailed: "Send failed: ",
    loading: "loading",
    error: "error",
    refreshFailedFlag: "refresh failed",
    livePrefix: "Live",
    sender: "sender",
    mentioned: "mentioned",
    searchFilter: "search",
    visible: "visible",
    total: "total",
    messagesVisible: "messages visible",
    teamLoadFailed: "team load failed",
    teamDiagnostics: "Team Diagnostics",
    diagnostics: "Diagnostics",
    diagnosticsSources: "Diagnostics Sources",
    leadSessionDiagnostics: "Lead Session Diagnostics",
    limitations: "Limitations",
    teamId: "Team Id",
    generatedAt: "Generated At",
    teamName: "Team Name",
    cwd: "CWD",
    discovered: "Discovered",
    sessionCount: "Session Count",
    latestSession: "Latest Session",
    latestModified: "Latest Modified",
    sourcePath: "Source Path",
    recentToolCalls: "Recent Tool Calls",
    tokenUsage: "Token Usage",
    inputTokens: "Input Tokens",
    outputTokens: "Output Tokens",
    cacheRead: "Cache Read",
    cacheWrite: "Cache Write",
    totalTokens: "Total Tokens",
    yes: "yes",
    no: "no",
    retryDiagnostics: "Retry diagnostics",
    retry: "Retry",
    loadError: "Load error",
    messageNotFound: "Message not found",
    message: "Message",
    messageUnit: "messages",
    messageMissing: "is not present in the current timeline.",
    teamLabel: "Team",
    routing: "Routing",
    kind: "Kind",
    delivery: "Delivery",
    replyTo: "Reply To",
    threadId: "Thread Id",
    mentions: "Mentions",
    effectiveRecipients: "Effective Recipients",
    threadMessages: "Thread Messages",
    rawJson: "Raw JSON",
    threadRoot: "Thread root",
    read: "read",
    acked: "acked",
    thread: "thread",
    member: "Member",
    leadCoordination: "Lead Coordination",
    profile: "Profile",
    executionSnapshot: "Execution Snapshot",
    executionMode: "Execution Mode",
    sessionState: "Session State",
    adapter: "Adapter",
    model: "Model",
    systemPrompt: "System Prompt",
    folded: "folded",
    environment: "Environment",
    recentActivity: "Recent Activity",
    sent: "sent",
    received: "received",
    role: "Role",
    status: "Status",
    name: "Name",
    foldedValue: "folded",
    noInputSummary: "no input summary",
    diagnosticsUnavailable: "Diagnostics unavailable",
    noTeamForDiagnostics: "Select a team to view diagnostics.",
    failedLoadTeams: "Failed to load teams",
    failedLoadTeamData: "Failed to load team data",
    failedLoadTeamDetail: "Failed to load team detail",
    failedLoadMemberDetail: "Failed to load member detail",
    refreshFailed: "Refresh failed",
    bytes: "bytes",
    resizeLeftPane: "Drag to resize the left pane",
    resizeRightPane: "Drag to resize the detail pane",
  },
};

function t(key) {
  return STRINGS[state.language]?.[key] || STRINGS.zh[key] || key;
}

function localizedError(key, error) {
  return state.language === "zh" ? `${t(key)}：${error.message}` : `${t(key)}: ${error.message}`;
}

function messageCountLabel(count) {
  return `${count} ${t("messageUnit")}`;
}

function na() {
  return t("na");
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

function loadColumnWidths() {
  const fallback = { ...DEFAULT_COLUMN_WIDTHS };
  try {
    if (typeof localStorage === "undefined") {
      return fallback;
    }
    const stored = JSON.parse(localStorage.getItem(COLUMN_WIDTHS_STORAGE_KEY) || "{}");
    const storedRight = Number(stored.right);
    return {
      left: clamp(Number(stored.left) || fallback.left, COLUMN_LIMITS.left.min, COLUMN_LIMITS.left.max),
      right: Number.isFinite(storedRight) && storedRight > 0
        ? clamp(storedRight, COLUMN_LIMITS.right.min, COLUMN_LIMITS.right.max)
        : null,
    };
  } catch {
    return fallback;
  }
}

function saveColumnWidths() {
  try {
    if (typeof localStorage === "undefined") {
      return;
    }
    localStorage.setItem(COLUMN_WIDTHS_STORAGE_KEY, JSON.stringify(state.columnWidths));
  } catch {
    // Persistence is optional; resizing still works for the current page.
  }
}

function applyColumnWidths() {
  const workspace = $("workspace");
  if (!workspace?.style?.setProperty) {
    return;
  }
  workspace.style.setProperty("--left-pane-width", `${state.columnWidths.left}px`);
  workspace.style.setProperty("--right-pane-width", `${resolvedRightPaneWidth(workspace)}px`);
}

function resolvedRightPaneWidth(workspace = $("workspace")) {
  const explicitRight = Number(state.columnWidths.right);
  if (Number.isFinite(explicitRight) && explicitRight > 0) {
    return clamp(explicitRight, COLUMN_LIMITS.right.min, COLUMN_LIMITS.right.max);
  }

  const workspaceWidth = Number(workspace?.clientWidth);
  if (!Number.isFinite(workspaceWidth) || workspaceWidth <= 0) {
    return FALLBACK_RIGHT_PANE_WIDTH;
  }

  const splitterWidth = 12;
  const remaining = workspaceWidth - state.columnWidths.left - splitterWidth;
  return clamp(
    Math.round(remaining / 2),
    COLUMN_LIMITS.right.min,
    COLUMN_LIMITS.right.max,
  );
}

function resolvedPaneWidth(side) {
  return side === "right" ? resolvedRightPaneWidth() : state.columnWidths.left;
}

function rightPaneUsesBalancedWidth() {
  const explicitRight = Number(state.columnWidths.right);
  return !(Number.isFinite(explicitRight) && explicitRight > 0);
}

const VALUE_LABELS = {
  zh: {
    lead: "负责人",
    member: "成员",
    worker: "工作者",
    coordinator: "协调者",
    running: "运行中",
    starting: "启动中",
    failed: "失败",
    active: "活跃",
    removed: "已移除",
    unknown: "未知",
    delivered: "已投递",
    partial: "部分投递",
    pending: "待投递",
    expired: "已过期",
    dispatch: "派发",
    discussion: "讨论",
    reply: "回复",
    system: "系统",
    notice: "通知",
    status: "状态",
    file: "文件",
    empty: "空",
    "not found": "未找到",
    "sent_message": "已发送消息",
    "received_message": "已接收消息",
    "sent_reply": "已发送回复",
    "mentioned": "被提及",
    "n/a": "无",
    user: "用户",
    assistant: "助手",
    tool: "工具",
    error: "错误",
    text: "文本",
    thinking: "思考",
    result: "结果",
    tool_use: "工具调用",
    tool_result: "工具结果",
    cwd_latest: "按工作目录最新会话",
    no_cwd: "缺少工作目录",
    no_session_file: "未找到会话文件",
    unsupported_provider: "暂不支持的提供方",
  },
  en: {},
};

const SOURCE_LABELS = {
  zh: {
    "Lead Pending Queue (project root)": "负责人待处理队列（项目根目录）",
    "Lead Pending Queue (base dir)": "负责人待处理队列（数据目录）",
    "MCP Log": "MCP 日志",
    "Lead Pending Wake Log": "负责人唤醒日志",
  },
  en: {},
};

const KNOWN_TEXT_TRANSLATIONS = {
  zh: [
    [
      "These diagnostics are file/session-level observations, not per-member stdout/stderr.",
      "这些诊断是文件/会话级观察结果，不是每个成员的 stdout/stderr。",
    ],
    [
      "Lead pending queue sources are probed in the project root and the web data base_dir; the real source may live in either place.",
      "负责人待处理队列会同时探测项目根目录和 Web 数据目录；真实来源可能位于任一位置。",
    ],
    [
      "Lead session diagnostics sample Claude session files only; they do not expose per-member stdout/stderr.",
      "负责人会话诊断只采样 Claude 会话文件，不暴露每个成员的 stdout/stderr。",
    ],
    [
      "Recent tool calls are truncated and derived from the latest discovered Claude session.",
      "最近工具调用已截断，并从最新发现的 Claude 会话中派生。",
    ],
    [
      "No stdout/stderr or tool-call events are available yet.",
      "目前还没有 stdout/stderr 或工具调用事件。",
    ],
    [
      "No cwd is available for this member or team.",
      "该成员和团队都没有可用工作目录。",
    ],
    [
      "Conversation rendering currently supports Claude Code JSONL sessions only.",
      "进程会话渲染目前只支持 Claude Code JSONL 会话。",
    ],
    [
      "No Claude Code session JSONL file was found for this member cwd.",
      "没有在该成员工作目录下找到 Claude Code 会话 JSONL 文件。",
    ],
    [
      "The lookup is scoped to the member cwd first, then team cwd.",
      "查找范围优先使用成员工作目录，其次使用团队工作目录。",
    ],
    [
      "The session is matched by cwd and latest modified Claude Code JSONL file.",
      "当前会话按工作目录和最近修改的 Claude Code JSONL 文件匹配。",
    ],
    [
      "Per-member exact session ids are not persisted yet, so concurrent members sharing one cwd can be ambiguous.",
      "目前尚未持久化每个成员的精确会话 ID；多个成员共用同一工作目录时可能存在歧义。",
    ],
    ["lead sent a message", "负责人发送了一条消息"],
  ],
  en: [],
};

function label(value) {
  const normalized = value == null || value === "" ? "n/a" : String(value);
  return VALUE_LABELS[state.language]?.[normalized] || normalized;
}

function sourceLabel(value) {
  return SOURCE_LABELS[state.language]?.[value] || value;
}

function localText(value) {
  if (value == null || value === "") {
    return na();
  }
  let text = String(value);
  for (const [from, to] of KNOWN_TEXT_TRANSLATIONS[state.language] || []) {
    text = text.replaceAll(from, to);
  }
  return label(text);
}

async function api(path) {
  const response = await fetch(path, { headers: { Accept: "application/json" } });
  if (!response.ok) {
    throw new Error(`${response.status} ${response.statusText}`);
  }
  return response.json();
}

async function apiPost(path, payload) {
  const response = await fetch(path, {
    method: "POST",
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      Accept: "application/json",
    },
    body: JSON.stringify(payload),
  });
  // Always try to parse JSON — both success (Created) and structured error
  // bodies come back as JSON. If parsing fails fall back to status text.
  let parsed = null;
  try {
    parsed = await response.json();
  } catch (_) {
    parsed = null;
  }
  if (!response.ok) {
    const detail = parsed && parsed.error ? parsed.error : `${response.status} ${response.statusText}`;
    throw new Error(detail);
  }
  return parsed;
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/// Stable HSL hue (0..360) derived from a sender name.
///
/// We use a simple djb2-style hash so the same name always lands on the
/// same hue across sessions. Special-casing keeps the two highest-signal
/// senders visually distinct regardless of name:
///   * `lead`  -> 188 (cyan, matches the brand accent so the eye still
///                 reads it as "the coordinator")
///   * `user`  -> 28  (warm orange — the human input shouldn't blend into
///                 the wash of AI workers)
/// All other names hash freely. The pre-skipped slice of hue space (the
/// orange/cyan bands) stays large enough that random workers still cover
/// blue/green/purple/red without clashing with the reserved colors.
function senderHue(name) {
  if (!name) return 200;
  if (name === "lead") return 188;
  if (name === "user") return 28;
  let hash = 5381;
  for (let i = 0; i < name.length; i += 1) {
    hash = ((hash << 5) + hash + name.charCodeAt(i)) & 0xffffffff;
  }
  // Skip the 14..50 (orange) and 175..205 (cyan) bands so workers don't
  // accidentally collide with `user` / `lead`. We have a comfortable
  // ~290° of remaining hue real estate, which is plenty for normal team sizes.
  const usable = 360 - 36 - 30; // 294 degrees
  let bucket = Math.abs(hash) % usable;
  if (bucket >= 14) bucket += 36; // jump past 14..50
  if (bucket >= 175) bucket += 30; // jump past 175..205
  return bucket;
}

/// Render a senderName as the colored pill used everywhere a member's
/// name appears (timeline rows, detail panels, member list). Caller is
/// responsible for HTML-escaping `senderName` if it ever flows from
/// untrusted input — names are validated to a strict slug, so within
/// this codebase they're safe to interpolate.
function renderSenderBadge(senderName, senderKind, extraClass = "") {
  const hue = senderHue(senderName);
  const kindClass =
    senderName === "user"
      ? "is-user"
      : senderKind === "lead" || senderName === "lead"
        ? "is-lead"
        : "";
  const cls = ["sender-badge", kindClass, extraClass].filter(Boolean).join(" ");
  return `<span class="${cls}" style="--sender-hue: ${hue}">${escapeHtml(senderName)}</span>`;
}

function escapeAttr(value) {
  return escapeHtml(value);
}

function fmtTime(value) {
  if (!value) return na();
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function parseDeepLink() {
  const hash = window.location.hash.replace(/^#/, "");
  const params = new URLSearchParams(hash);
  return {
    team: params.get("team") || "",
    message: params.get("message") || "",
    member: params.get("member") || "",
  };
}

function setDeepLink(type, value) {
  const params = new URLSearchParams();
  if (state.teamId) {
    params.set("team", state.teamId);
  }
  if (!type || !value) {
    const nextHash = params.toString();
    window.location.hash = nextHash ? nextHash : "";
    state.deepLink = parseDeepLink();
    return;
  }

  params.set(type, value);
  state.deepLink = {
    team: state.teamId || "",
    message: type === "message" ? value : "",
    member: type === "member" ? value : "",
  };
  window.location.hash = params.toString();
}

function clearDeepLink() {
  if (!state.teamId) {
    window.location.hash = "";
    state.deepLink = null;
    return;
  }
  const params = new URLSearchParams();
  params.set("team", state.teamId);
  window.location.hash = params.toString();
  state.deepLink = parseDeepLink();
}

function activeTeam() {
  return (
    state.teams.find((team) => team.id === state.teamId) ||
    (state.teamDetail?.team?.id === state.teamId ? state.teamDetail.team : null)
  );
}

function leadMemberName() {
  const team = activeTeam();
  if (!team) return null;
  const lead = state.members.find((member) => member.kind === "lead");
  return lead?.name || team.leadMemberId || null;
}

async function loadTeams() {
  state.loadingTeams = true;
  state.teamError = "";
  state.refreshError = "";
  state.diagnostics = null;
  state.diagnosticsError = "";
  state.diagnosticsLoading = false;
  renderShell();
  try {
    const data = await api("/api/teams");
    state.teams = data.teams || [];
    const deepLinkTeam = (state.deepLink || parseDeepLink()).team;
    const nextTeamId = resolveSelectedTeamId(state.teams, state.teamId || deepLinkTeam);
    if (nextTeamId !== state.teamId) {
      state.selectedMessageId = null;
      state.selectedMemberName = null;
      clearMemberDetailCache();
      state.detailError = "";
      state.refreshError = "";
    }
    state.teamId = nextTeamId;
    renderTeamSelect();
    if (state.teamId) {
      await loadTeam(state.teamId);
    } else {
      state.room = null;
      state.members = [];
      state.teamDetail = null;
      state.failedTeamId = null;
      state.selectedMessageId = null;
      state.selectedMemberName = null;
      clearMemberDetailCache();
      state.refreshError = "";
      state.diagnostics = null;
      state.diagnosticsError = "";
      state.diagnosticsLoading = false;
      renderShell();
    }
  } catch (error) {
    state.teamError = localizedError("failedLoadTeams", error);
    state.teams = [];
    state.teamId = null;
    state.room = null;
    state.members = [];
    state.teamDetail = null;
    state.failedTeamId = null;
    state.selectedMessageId = null;
    state.selectedMemberName = null;
    clearMemberDetailCache();
    state.refreshError = "";
    state.diagnostics = null;
    state.diagnosticsError = "";
    state.diagnosticsLoading = false;
    renderShell();
  } finally {
    state.loadingTeams = false;
    renderShell();
  }
}

function resolveSelectedTeamId(teams, requestedTeamId) {
  if (requestedTeamId && teams.some((team) => team.id === requestedTeamId)) {
    return requestedTeamId;
  }
  return teams[0]?.id || null;
}

function applyTeamDeepLink(teamId) {
  state.teamId = teamId || null;
  clearDeepLink();
}

function renderTeamSelect() {
  const select = $("teamSelect");
  select.innerHTML = state.teams.length
    ? state.teams
        .map((team) => `<option value="${escapeHtml(team.id)}">${escapeHtml(team.name)}</option>`)
        .join("")
    : `<option value="">${t("noTeams")}</option>`;
  select.value = state.teamId || "";
}

async function loadTeam(teamId) {
  state.loadingTeam = true;
  state.teamError = "";
  state.detailError = "";
  state.refreshError = "";
  state.diagnostics = null;
  state.diagnosticsError = "";
  state.diagnosticsLoading = true;
  renderShell();
  try {
    const [teamDetail, room, members] = await Promise.all([
      api(`/api/teams/${encodeURIComponent(teamId)}`),
      api(`/api/teams/${encodeURIComponent(teamId)}/rooms/main?limit=200`),
      api(`/api/teams/${encodeURIComponent(teamId)}/members`),
    ]);
    state.teamId = teamId;
    state.teamDetail = teamDetail;
    state.room = room;
    state.members = members.members || [];
    state.failedTeamId = null;
    applyDeepLink();
    if (!state.selectedMessageId && !state.selectedMemberName) {
      state.selectedMemberName = leadMemberName() || state.members[0]?.name || null;
      state.detailTab = "session";
    }
    renderShell();
    await loadDiagnostics(teamId);
  } catch (error) {
    state.teamError = localizedError("failedLoadTeamData", error);
    state.refreshError = localizedError("refreshFailed", error);
    state.room = null;
    state.members = [];
    state.teamDetail = null;
    state.failedTeamId = teamId;
    state.detailError = localizedError("failedLoadTeamDetail", error);
    state.selectedMessageId = null;
    state.selectedMemberName = null;
    state.diagnosticsLoading = false;
    renderShell();
  } finally {
    state.loadingTeam = false;
    renderShell();
  }
}

async function loadDiagnostics(teamId) {
  try {
    const data = await api(`/api/teams/${encodeURIComponent(teamId)}/diagnostics`);
    if (state.teamId !== teamId) {
      return;
    }
    state.diagnostics = data;
    state.diagnosticsError = "";
  } catch (error) {
    if (state.teamId !== teamId) {
      return;
    }
    state.diagnostics = null;
    state.diagnosticsError = localizedError("diagnosticsUnavailable", error);
  } finally {
    if (state.teamId === teamId) {
      state.diagnosticsLoading = false;
      renderShell();
    }
  }
}

function applyDeepLink() {
  const deepLink = state.deepLink || parseDeepLink();
  if (deepLink.message) {
    state.selectedMessageId = deepLink.message;
    state.selectedMemberName = null;
    state.detailTab = "detail";
    clearMemberDetailCache();
  } else if (deepLink.member) {
    state.selectedMemberName = deepLink.member;
    state.selectedMessageId = null;
    state.detailTab = "session";
  }
}

function clearMemberDetailCache() {
  state.memberDetailKey = "";
  state.memberDetailLoadingKey = "";
  state.memberDetail = null;
  state.memberActivity = null;
  state.memberConversationKey = "";
  state.memberConversationLoadingKey = "";
  state.memberConversationScrollKey = "";
  state.memberConversation = null;
}

function memberDetailKey(teamId, name) {
  return `${teamId || ""}:${name || ""}`;
}

async function refreshCurrentTeam() {
  if (state.loadingTeams || state.loadingTeam || !state.teamId) {
    return;
  }
  const teamId = state.teamId;
  try {
    const [teamDetail, room, members] = await Promise.all([
      api(`/api/teams/${encodeURIComponent(teamId)}`),
      api(`/api/teams/${encodeURIComponent(teamId)}/rooms/main?limit=200`),
      api(`/api/teams/${encodeURIComponent(teamId)}/members`),
    ]);
    if (state.teamId !== teamId) {
      return;
    }
    state.teamDetail = teamDetail;
    state.room = room;
    state.members = members.members || [];
    state.refreshError = "";
    if (state.selectedMemberName && !state.members.some((member) => member.name === state.selectedMemberName)) {
      state.selectedMemberName = leadMemberName() || state.members[0]?.name || null;
      state.selectedMessageId = null;
      clearMemberDetailCache();
      setDeepLink("member", state.selectedMemberName);
    }
    renderShell();
    if (state.selectedMemberName) {
      if (state.detailTab === "session") {
        refreshMemberConversation(state.selectedMemberName, { force: true });
      } else {
        refreshMemberDetail(state.selectedMemberName, { force: true });
      }
    }
  } catch (error) {
    if (state.teamId !== teamId) {
      return;
    }
    state.refreshError = localizedError("refreshFailed", error);
    renderFooter();
    renderHeader();
  }
}

function startAutoRefresh() {
  if (refreshTimer) {
    return;
  }
  const intervalFn = window.setInterval || globalThis.setInterval;
  if (typeof intervalFn !== "function") {
    return;
  }
  refreshTimer = intervalFn(refreshCurrentTeam, REFRESH_INTERVAL_MS);
}

function allMessages() {
  return state.room?.messages || [];
}

function filteredMessages() {
  const search = state.search.trim().toLowerCase();
  return allMessages().filter((message) => {
    if (state.senderFilter && message.sender !== state.senderFilter) return false;
    if (state.mentionFilter) {
      const mentions = message.mentions || [];
      const recipients = message.effectiveRecipients || [];
      if (!mentions.includes(state.mentionFilter) && !recipients.includes(state.mentionFilter)) {
        return false;
      }
    }
    if (!search) return true;
    return [
      message.sender,
      message.kind,
      message.body,
      ...(message.mentions || []),
      ...(message.effectiveRecipients || []),
    ]
      .join(" ")
      .toLowerCase()
      .includes(search);
  });
}

function renderShell() {
  renderStaticText();
  renderEmptyOrErrorBanner();
  renderHeader();
  renderLeftPane();
  renderTimeline();
  renderComposer();
  renderDetail();
  bindDiagnosticsRetryButton();
  renderFooter();
}

/// Recipient candidates for the composer's @ dropdown.
///
/// Order: lead first (so it's the natural default), then alphabetized
/// active workers. Synthetic `user` is excluded — sending @user to yourself
/// is rejected by the server's self-mention guard, so showing it is
/// guaranteed-fail UX.
function composerRecipientCandidates() {
  const all = state.members || [];
  const lead = all.find((m) => m.kind === "lead");
  const workers = all
    .filter((m) => m.kind !== "lead")
    .filter((m) => m.name !== "user")
    .filter((m) => (m.status || "active") === "active")
    .sort((a, b) => a.name.localeCompare(b.name));
  return lead ? [lead, ...workers] : workers;
}

function renderComposer() {
  const form = document.getElementById("composerForm");
  if (!form) return;
  const select = document.getElementById("composerMention");
  const input = document.getElementById("composerInput");
  const button = document.getElementById("composerSend");
  if (!select || !input || !button) return;

  // Populate placeholder + button text on every render so language toggle
  // takes effect immediately.
  input.placeholder = t("composerPlaceholder");
  if (!state.composerSending) {
    button.textContent = t("composerSend");
    button.disabled = false;
  } else {
    button.textContent = t("composerSending");
    button.disabled = true;
  }

  const candidates = composerRecipientCandidates();
  const previous = state.composerMention || (candidates[0] && candidates[0].name) || "";
  const stillExists = candidates.some((m) => m.name === previous);
  const chosen = stillExists ? previous : candidates[0] && candidates[0].name;

  // Repopulate the <select> only when the membership list changed; otherwise
  // re-renders during typing would reset the focused option to the first
  // entry and surprise the user.
  const desired = candidates.map((m) => m.name).join(",");
  if (select.dataset.populatedFor !== desired) {
    select.innerHTML = candidates
      .map(
        (m) =>
          `<option value="${escapeAttr(m.name)}">${escapeHtml(m.name)}${
            m.kind === "lead" ? ` (${escapeHtml(label("lead"))})` : ""
          }</option>`,
      )
      .join("");
    select.dataset.populatedFor = desired;
  }
  if (chosen) {
    select.value = chosen;
    state.composerMention = chosen;
  }

  form.style.display = state.teamId && candidates.length ? "" : "none";
}

async function submitComposer() {
  const input = document.getElementById("composerInput");
  const select = document.getElementById("composerMention");
  const status = document.getElementById("composerStatus");
  if (!input || !select || !status) return;

  if (!state.teamId) {
    setComposerStatus("error", t("composerNoTeam"));
    return;
  }
  const text = (input.value || "").trim();
  if (!text) {
    setComposerStatus("error", t("composerEmpty"));
    return;
  }
  const recipient = select.value;
  if (!recipient) {
    setComposerStatus("error", t("composerNoTeam"));
    return;
  }

  // The recipient is added explicitly so the server doesn't have to scan
  // the body for an @mention; user-typed @ in the body is also honored
  // (combined deduped server-side).
  const body = text.startsWith(`@${recipient}`) ? text : `@${recipient} ${text}`;

  state.composerSending = true;
  setComposerStatus("info", t("composerSending"));
  renderComposer();

  try {
    await apiPost(
      `/api/teams/${encodeURIComponent(state.teamId)}/rooms/main/messages`,
      { body, mentions: [recipient] },
    );
    input.value = "";
    setComposerStatus("success", `${t("composerSentTo")} @${recipient}`);
    state.timelineForceScrollBottom = true;
    // Refresh room messages so the just-sent message appears immediately.
    await loadTeam(state.teamId);
  } catch (err) {
    setComposerStatus("error", `${t("composerSendFailed")}${err.message || err}`);
  } finally {
    state.composerSending = false;
    renderComposer();
  }
}

function setComposerStatus(level, text) {
  const status = document.getElementById("composerStatus");
  if (!status) return;
  status.textContent = text;
  status.className = `composer-status ${level === "success" ? "success" : level === "error" ? "error" : "muted"}`;
}

function renderStaticText() {
  document.documentElement.lang = state.language === "zh" ? "zh-CN" : "en";
  document.title = t("brandKicker");
  applyColumnWidths();
  $("brandKicker").textContent = t("brandKicker");
  $("brandTitle").textContent = t("brandTitle");
  $("teamLabel").textContent = t("team");
  $("searchLabel").textContent = t("search");
  $("searchInput").placeholder = t("searchPlaceholder");
  $("languageToggleButton").textContent = t("switchLanguage");
  $("languageToggleButton").title = t("languageToggleTitle");
  $("reloadButton").title = t("reload");
  $("leftSplitter").title = t("resizeLeftPane");
  $("leftSplitter").setAttribute("aria-label", t("resizeLeftPane"));
  $("rightSplitter").title = t("resizeRightPane");
  $("rightSplitter").setAttribute("aria-label", t("resizeRightPane"));
  $("roomsTitle").textContent = t("rooms");
  $("membersTitle").textContent = t("members");
  $("filtersTitle").textContent = t("filters");
  $("clearFiltersButton").textContent = t("resetFilters");
  $("focusLeadButton").textContent = t("leadActivity");
  $("timelineTitle").textContent = t("timeline");
  $("detailPaneTitle").textContent = t("detailPane");
  $("sessionTabButton").textContent = t("sessionTab");
  $("detailTabButton").textContent = t("detailTab");
  $("diagnosticsTabButton").textContent = t("diagnosticsTab");
}

function renderHeader() {
  const team = activeTeam();
  $("teamSelect").value = state.teamId || "";
  $("timelineSubtitle").textContent = state.loadingTeam
    ? t("loadingTeamData")
    : team
      ? `${team.name} · ${state.members.length} ${t("members")} · ${messageCountLabel(allMessages().length)}`
      : state.loadingTeams
        ? t("loadingTeams")
        : t("noTeamsAvailable");

  const summary = [];
  if (state.senderFilter) summary.push(`${t("sender")}=${state.senderFilter}`);
  if (state.mentionFilter) summary.push(`${t("mentioned")}=${state.mentionFilter}`);
  if (state.search.trim()) summary.push(`${t("searchFilter")}=${state.search.trim()}`);
  $("filterSummary").textContent = summary.length ? summary.join(" · ") : t("noFilters");
  $("statusSummary").textContent = team ? `${t("readOnly")} · ${team.name}` : t("readOnly");

  const liveParts = [];
  if (state.loadingTeams || state.loadingTeam) liveParts.push(t("loading"));
  else liveParts.push(t("ready"));
  if (state.teamError) liveParts.push(t("error"));
  if (state.refreshError && !state.loadingTeam) liveParts.push(t("refreshFailedFlag"));
  $("liveStatus").textContent = `${t("livePrefix")}：${liveParts.join(" · ")}`;
}

function renderLeftPane() {
  const roomList = $("roomList");
  if (!state.teamId) {
    roomList.innerHTML = `<div class="empty">${t("noTeams")}</div>`;
  } else {
    roomList.innerHTML = `<button class="list-button active" type="button">main</button>`;
  }

  const memberList = $("memberList");
  if (!state.teamId) {
    memberList.innerHTML = `<div class="empty">${t("noMembers")}</div>`;
  } else if (state.members.length === 0) {
    memberList.innerHTML = `<div class="empty">${t("noMembers")}</div>`;
  } else {
    memberList.innerHTML = state.members
      .map((member) => {
        const active = member.name === state.selectedMemberName ? " active" : "";
        const badge = member.kind === "lead" ? "lead" : "worker";
        const hue = senderHue(member.name);
        return `
          <button class="list-button${active}" type="button" data-member="${escapeHtml(member.name)}" style="--sender-hue: ${hue}">
            <div class="message-head">
              ${renderSenderBadge(member.name, member.kind)}
              <span class="badge ${badge}">${escapeHtml(label(member.sessionState || "unknown"))}</span>
            </div>
            <div class="subtle">${escapeHtml(label(member.roleLabel || ""))}</div>
          </button>
        `;
      })
      .join("");
  }

  document.querySelectorAll("[data-member]").forEach((button) => {
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      state.selectedMemberName = button.getAttribute("data-member");
      state.selectedMessageId = null;
      state.detailTab = "session";
      clearMemberDetailCache();
      state.detailError = "";
      setDeepLink("member", state.selectedMemberName);
      renderShell();
    });
  });
}

function renderTimeline() {
  const scrollSnapshot = captureTimelineScroll();
  const messages = filteredMessages();
  const totalMessages = allMessages().length;
  const hasActiveFilters = Boolean(state.senderFilter || state.mentionFilter || state.search.trim());
  $("timelineStats").innerHTML = hasActiveFilters
    ? `<span class="chat-count">${messages.length}/${messageCountLabel(totalMessages)}</span>`
    : `<span class="chat-count">${messageCountLabel(totalMessages)}</span>`;

  if (!state.teamId) {
    $("messageList").innerHTML = `<div class="empty">${t("noMessages")}</div>`;
    restoreTimelineScroll(scrollSnapshot);
    return;
  }

  if (state.loadingTeam) {
    $("messageList").innerHTML = `<div class="empty">${t("loadingMessages")}</div>`;
    restoreTimelineScroll(scrollSnapshot);
    return;
  }

  if (allMessages().length === 0) {
    $("messageList").innerHTML = `<div class="empty">${t("noMessages")}</div>`;
    restoreTimelineScroll(scrollSnapshot);
    return;
  }

  if (messages.length === 0) {
    $("messageList").innerHTML = `<div class="empty">${t("noMessagesMatch")}</div>`;
    restoreTimelineScroll(scrollSnapshot);
    return;
  }

  $("messageList").innerHTML = messages
    .map((message) => {
      const active = message.id === state.selectedMessageId ? " active" : "";
      if (isSystemChatMessage(message)) {
        return `
          <article class="chat-system${active}" data-message="${escapeHtml(message.id)}">
            <div class="chat-system-time">${fmtTime(message.createdAt)}</div>
            <div class="chat-system-text">${renderChatText(message.bodyPreview || message.body || "")}</div>
          </article>
        `;
      }
      const status = renderChatDeliveryStatus(message);
      const senderHueValue = senderHue(message.sender);
      const avatarKind =
        message.sender === "user"
          ? "user"
          : message.senderKind === "lead" || message.sender === "lead"
            ? "lead"
            : "worker";
      return `
        <article class="chat-message${active}" data-message="${escapeHtml(message.id)}" style="--sender-hue: ${senderHueValue}">
          <button class="chat-avatar ${avatarKind} sender-tinted" type="button" data-sender="${escapeHtml(message.sender)}" aria-label="${escapeHtml(message.sender)}" style="--sender-hue: ${senderHueValue}">
            ${escapeHtml(avatarInitials(message.sender))}
          </button>
          <div class="chat-content">
            <div class="chat-meta">
              <button class="link-button chat-name" type="button" data-sender="${escapeHtml(message.sender)}" style="color: hsl(${senderHueValue}, 80%, 75%)">${escapeHtml(message.sender)}</button>
              <span class="chat-time">${fmtTime(message.createdAt)}</span>
              ${status}
            </div>
            <div class="chat-bubble">${renderChatText(message.bodyPreview || message.body || "")}</div>
          </div>
        </article>
      `;
    })
    .join("");

  document.querySelectorAll("[data-message]").forEach((row) => {
    row.addEventListener("click", () => {
      state.selectedMessageId = row.getAttribute("data-message");
      state.selectedMemberName = null;
      state.detailTab = "detail";
      state.detailError = "";
      setDeepLink("message", state.selectedMessageId);
      renderShell();
    });
  });

  document.querySelectorAll("[data-mention]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      state.mentionFilter = button.getAttribute("data-mention");
      renderShell();
    });
  });

  document.querySelectorAll("[data-sender]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      state.senderFilter = button.getAttribute("data-sender");
      state.mentionFilter = "";
      renderShell();
    });
  });

  restoreTimelineScroll(scrollSnapshot);
}

function captureTimelineScroll() {
  const list = $("messageList");
  if (!list) {
    return { list: null, nearBottom: true, top: 0 };
  }
  const distanceToBottom = list.scrollHeight - list.scrollTop - list.clientHeight;
  const nearBottom =
    !Number.isFinite(distanceToBottom) || distanceToBottom <= TIMELINE_BOTTOM_THRESHOLD_PX;
  return {
    list,
    nearBottom,
    top: list.scrollTop || 0,
  };
}

function restoreTimelineScroll(snapshot) {
  const list = snapshot?.list || $("messageList");
  if (!list) {
    return;
  }
  const shouldStickToBottom = Boolean(state.timelineForceScrollBottom || snapshot?.nearBottom);
  const shouldClearForce = !state.loadingTeam;
  nextFrame(() => {
    if (shouldStickToBottom) {
      list.scrollTop = list.scrollHeight;
    } else {
      list.scrollTop = snapshot?.top || 0;
    }
    if (shouldClearForce) {
      state.timelineForceScrollBottom = false;
    }
  });
}

function isSystemChatMessage(message) {
  return message.kind === "status" || String(message.body || "").startsWith("[SYSTEM]");
}

function avatarInitials(name) {
  const trimmed = String(name || "?").trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/[\s._-]+/).filter(Boolean);
  if (parts.length >= 2) {
    return `${parts[0][0] || ""}${parts[1][0] || ""}`.toUpperCase();
  }
  return trimmed.slice(0, 2).toUpperCase();
}

function renderChatDeliveryStatus(message) {
  const status = message.deliveryStatus;
  if (!["failed", "expired", "partial"].includes(status)) {
    return "";
  }
  const className = status === "partial" ? "warn" : "fail";
  return `<span class="chat-status ${className}">${escapeHtml(label(status))}</span>`;
}

function renderChatText(text) {
  const escaped = escapeHtml(text || "");
  return escaped.replace(
    /@([\p{L}\p{N}_.-]+)/gu,
    '<button class="link-button mention-token" type="button" data-mention="$1">@$1</button>',
  );
}

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

function renderDetail() {
  renderDetailTabButtons();
  if (state.detailError) {
    $("detailTitle").textContent = t("loadError");
    $("detailBody").innerHTML = `
      <div class="empty">${escapeHtml(state.detailError)}</div>
      <button class="chip retry-button" type="button" id="retryDetailButton">${t("retry")}</button>
    `;
    const retryButton = $("retryDetailButton");
    if (retryButton) {
      retryButton.addEventListener("click", async () => {
        state.detailError = "";
        await retryDetail();
      });
    }
    return;
  }
  if (state.detailTab === "session") {
    renderSessionPanel();
    return;
  }
  if (state.detailTab === "diagnostics") {
    renderDiagnosticsPanel();
    return;
  }

  const message = allMessages().find((item) => item.id === state.selectedMessageId);
  if (message) {
    renderMessageDetail(message);
    return;
  }
  if (state.selectedMessageId) {
    $("detailTitle").textContent = t("messageNotFound");
    $("detailBody").innerHTML = `
      <div class="empty">${
        state.loadingTeam
          ? t("loadingSelectedMessage")
          : `${t("message")} ${escapeHtml(state.selectedMessageId)} ${t("messageMissing")}`
      }</div>
    `;
    return;
  }
  if (state.selectedMemberName) {
    renderMemberDetail(state.selectedMemberName);
    return;
  }

  const team = activeTeam();
  if (team) {
    $("detailTitle").textContent = team.name;
    $("detailBody").innerHTML = `
      <div class="detail-card">
        <div class="detail-head">
          <span class="badge lead">${t("teamLabel")}</span>
          <span class="pill">${escapeHtml(label(team.status))}</span>
        </div>
        <div class="subtle">${escapeHtml(team.cwd || "")}</div>
      </div>
    `;
    return;
  }

  $("detailTitle").textContent = t("noSelection");
  $("detailBody").innerHTML = `<div class="empty">${state.teamError ? escapeHtml(state.teamError) : t("pickMessageOrMember")}</div>`;
}

function renderDetailTabButtons() {
  const sessionButton = $("sessionTabButton");
  const detailButton = $("detailTabButton");
  const diagnosticsButton = $("diagnosticsTabButton");
  if (!sessionButton || !detailButton || !diagnosticsButton) {
    return;
  }
  sessionButton.classList.toggle("active", state.detailTab === "session");
  detailButton.classList.toggle("active", state.detailTab === "detail");
  diagnosticsButton.classList.toggle("active", state.detailTab === "diagnostics");
}

function renderSessionPanel() {
  if (state.selectedMemberName) {
    renderMemberConversation(state.selectedMemberName);
    return;
  }
  if (state.selectedMessageId) {
    const message = allMessages().find((item) => item.id === state.selectedMessageId);
    if (message) {
      $("detailTitle").textContent = `${t("message")} ${message.id}`;
      $("detailBody").innerHTML = `<div class="empty">${t("pickMessageOrMember")}</div>`;
      return;
    }
  }
  const lead = leadMemberName() || state.members[0]?.name || null;
  if (lead) {
    state.selectedMemberName = lead;
    state.selectedMessageId = null;
    renderMemberConversation(lead);
    return;
  }
  $("detailTitle").textContent = t("processSession");
  $("detailBody").innerHTML = `<div class="empty">${t("pickMessageOrMember")}</div>`;
}

function renderDiagnosticsPanel() {
  const diagnosticsMarkup = renderDiagnosticsSections();
  $("detailTitle").textContent = t("teamDiagnostics");
  $("detailBody").innerHTML = state.teamId
    ? diagnosticsMarkup || `<div class="empty">${t("noDiagnosticsLoaded")}</div>`
    : `<div class="empty">${t("noTeamForDiagnostics")}</div>`;
  bindDiagnosticsRetryButton();
}

async function renderMessageDetail(message) {
  const thread = allMessages().filter((item) => item.threadId === message.threadId);
  const rootMessage = thread.find((item) => item.replyTo === null) || thread[0] || null;
  $("detailTitle").textContent = `${t("message")} ${message.id}`;
  $("detailBody").innerHTML = `
    <div class="detail-card">
      <div class="detail-head">
        ${renderSenderBadge(message.sender, message.senderKind)}
        <span class="pill">${escapeHtml(label(message.kind))}</span>
        <span class="pill">${escapeHtml(label(message.deliveryStatus))}</span>
      </div>
      <div class="detail-meta">
        <span class="pill">${fmtTime(message.createdAt)}</span>
        <span class="pill">${t("read")} ${message.readCount || 0}</span>
        <span class="pill">${t("acked")} ${message.ackedCount || 0}</span>
        <span class="pill">${t("thread")} ${message.threadReplyCount || 0}</span>
      </div>
      <div class="message-body">${escapeHtml(message.body || "")}</div>
    </div>
    <div class="detail-card">
      <div class="section-title">${t("routing")}</div>
      <div class="detail-grid">
        <div><span class="muted">${t("sender")}</span><div>${escapeHtml(message.sender)}</div></div>
        <div><span class="muted">${t("kind")}</span><div>${escapeHtml(label(message.kind))}</div></div>
        <div><span class="muted">${t("delivery")}</span><div>${escapeHtml(label(message.deliveryStatus))}</div></div>
        <div><span class="muted">${t("replyTo")}</span><div>${escapeHtml(message.replyTo || t("none"))}</div></div>
        <div><span class="muted">${t("threadId")}</span><div>${escapeHtml(message.threadId || t("none"))}</div></div>
      </div>
      <div class="detail-list-block">
        <div class="muted">${t("mentions")}</div>
        <div class="detail-pills">${
          (message.mentions || []).length
            ? (message.mentions || []).map((mention) => `<span class="pill">@${escapeHtml(mention)}</span>`).join("")
            : `<span class="empty-inline">${t("none")}</span>`
        }</div>
        <div class="muted">${t("effectiveRecipients")}</div>
        <div class="detail-pills">${
          (message.effectiveRecipients || []).length
            ? (message.effectiveRecipients || []).map((recipient) => `<span class="pill">${escapeHtml(recipient)}</span>`).join("")
            : `<span class="empty-inline">${t("none")}</span>`
        }</div>
      </div>
    </div>
    <div class="detail-card">
      <div class="section-title">${t("threadMessages")}</div>
      ${
        thread.length
          ? thread
              .map(
                (item) => `
                  <div class="detail-item">
                    <div class="detail-head">
                      ${renderSenderBadge(item.sender, item.senderKind)}
                      <span class="pill">${fmtTime(item.createdAt)}</span>
                    </div>
                    <div class="message-body">${escapeHtml(item.bodyPreview || item.body || "")}</div>
                  </div>
                `,
              )
              .join("")
          : `<div class="empty">${t("noMessages")}</div>`
      }
    </div>
    <details class="detail-card">
      <summary>${t("rawJson")}</summary>
      <pre class="code-block">${escapeHtml(JSON.stringify(message, null, 2))}</pre>
    </details>
  `;

  if (rootMessage) {
    const rootSummary = $("detailBody").querySelector(".detail-card");
    if (rootSummary) {
      const foot = document.createElement("div");
      foot.className = "subtle";
      foot.textContent = `${t("threadRoot")}：${rootMessage.id}`;
      rootSummary.appendChild(foot);
    }
  }
}

function renderMemberDetail(name) {
  const teamId = state.teamId;
  if (!teamId || !name) return;

  const key = memberDetailKey(teamId, name);
  if (state.memberDetailKey === key && state.memberDetail && state.memberActivity) {
    renderMemberDetailContent(name, state.memberDetail, state.memberActivity);
    return;
  }

  $("detailTitle").textContent = isLeadName(name) ? t("leadActivity") : `${t("member")} ${name}`;
  $("detailBody").innerHTML = `
    <div class="empty">${t("loadingMemberDetail")}</div>
  `;
  refreshMemberDetail(name);
}

function renderMemberConversation(name) {
  const teamId = state.teamId;
  if (!teamId || !name) return;

  const key = memberDetailKey(teamId, name);
  $("detailTitle").textContent = `${t("processSession")} · ${name}`;

  if (state.memberConversationKey === key && state.memberConversation) {
    renderMemberConversationContent(name, state.memberConversation, {
      forceBottom: state.memberConversationScrollKey !== key,
    });
    return;
  }

  $("detailBody").innerHTML = `<div class="empty">${t("loadingConversation")}</div>`;
  refreshMemberConversation(name);
}

async function refreshMemberConversation(name, { force = false } = {}) {
  const teamId = state.teamId;
  if (!teamId || !name) return;

  const key = memberDetailKey(teamId, name);
  if (state.memberConversationLoadingKey === key) {
    return;
  }
  if (!force && state.memberConversationKey === key && state.memberConversation) {
    return;
  }

  state.memberConversationLoadingKey = key;
  try {
    const data = await api(
      `/api/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(name)}/conversation`,
    );
    if (state.teamId !== teamId || state.selectedMemberName !== name) {
      return;
    }
    state.memberConversationKey = key;
    state.memberConversation = data;
    state.memberConversationLoadingKey = "";
    if (state.detailTab === "session") {
      renderMemberConversationContent(name, data, {
        forceBottom: state.memberConversationScrollKey !== key,
      });
    }
  } catch (error) {
    if (state.teamId !== teamId || state.selectedMemberName !== name) {
      return;
    }
    state.memberConversationLoadingKey = "";
    state.refreshError = localizedError("refreshFailed", error);
    if (state.detailTab === "session" && !(state.memberConversationKey === key && state.memberConversation)) {
      $("detailTitle").textContent = `${t("processSession")} · ${name}`;
      $("detailBody").innerHTML = `<div class="empty">${escapeHtml(localizedError("failedLoadMemberDetail", error))}</div>`;
    }
  }
}

function renderMemberConversationContent(name, data, { forceBottom = false } = {}) {
  const key = memberDetailKey(state.teamId, name);
  $("detailTitle").textContent = `${t("processSession")} · ${name}`;
  const source = data.source || {};
  const sourceMeta = renderConversationSource(source);

  const limitations = (data.limitations || []).length
    ? `<details class="conversation-disclosure"><summary>${t("limitations")}</summary><div class="detail-pills">${(data.limitations || [])
        .map((item) => `<span class="pill">${escapeHtml(localText(item))}</span>`)
        .join("")}</div></details>`
    : "";

  const items = Array.isArray(data.items) ? data.items : [];
  const conversation = items.length
    ? renderConversationTranscript(items)
    : `<div class="empty">${t("noConversation")}</div>`;

  preserveDetailScroll(() => {
    $("detailBody").innerHTML = `${sourceMeta}${conversation}${limitations}`;
  }, { forceBottom });
  if (forceBottom) {
    nextFrame(() => {
      state.memberConversationScrollKey = key;
    });
  } else {
    state.memberConversationScrollKey = key;
  }
}

function renderConversationSource(source) {
  return `
    <details class="conversation-source" open>
      <summary>
        <span>${t("conversationSource")}</span>
        <span class="conversation-source-summary">${escapeHtml(source.sessionId || na())}</span>
      </summary>
      <div class="conversation-source-grid">
        <div><span>${t("model")}</span><strong>${escapeHtml(label(source.provider || "n/a"))}</strong></div>
        <div><span>${t("matchedBy")}</span><strong>${escapeHtml(label(source.confidence || "unknown"))}</strong></div>
        <div><span>${t("latestModified")}</span><strong>${fmtTime(source.updatedAt)}</strong></div>
      </div>
      <div class="conversation-source-path">${escapeHtml(source.cwd || na())}</div>
      <div class="conversation-source-path">${escapeHtml(source.path || na())}</div>
    </details>
  `;
}

function renderConversationTranscript(items) {
  const renderItems = preprocessConversationItems(items);
  const groups = groupConversationItemsIntoTurns(renderItems);
  return `
    <div class="conversation-list">
      ${groups.map(renderConversationGroup).join("")}
    </div>
  `;
}

function preprocessConversationItems(items) {
  const renderItems = [];
  const pendingTools = new Map();
  const setupItems = [];

  const flushSetup = () => {
    if (!setupItems.length) return;
    if (setupItems.length > 1 || renderItems.length === 0) {
      renderItems.push({
        type: "session_setup",
        id: `setup-${setupItems[0].id}`,
        prompts: setupItems.splice(0),
      });
    } else {
      renderItems.push(setupItems.shift());
    }
    setupItems.length = 0;
  };

  for (const item of items) {
    const kind = item.kind || "text";
    const role = item.role || "unknown";
    const id = item.id || `${renderItems.length}`;
    const text = item.text || "";

    if (role === "user" && kind === "text") {
      const userItem = { type: "user_prompt", id, content: text, timestamp: item.timestamp };
      if (isSessionSetupText(text)) {
        setupItems.push(userItem);
      } else {
        flushSetup();
        renderItems.push(userItem);
      }
      continue;
    }

    flushSetup();

    if (kind === "thinking") {
      renderItems.push({
        type: "thinking",
        id,
        thinking: text,
        timestamp: item.timestamp,
      });
      continue;
    }

    if (kind === "tool_use") {
      const toolId = item.toolUseId || id;
      const toolName = item.toolName || item.title || "tool";
      const toolInput = item.input ?? parseMaybeJson(text);
      const toolItem = {
        type: "tool_call",
        id: toolId,
        sourceId: id,
        toolName,
        toolInput,
        toolResult: null,
        status: "pending",
        timestamp: item.timestamp,
      };
      pendingTools.set(toolId, toolItem);
      renderItems.push(toolItem);
      continue;
    }

    if (kind === "tool_result") {
      const toolId = item.toolUseId || "";
      const result = {
        content: text,
        structured: item.result,
        isError: item.isError || role === "error",
        timestamp: item.timestamp,
      };
      const toolItem = pendingTools.get(toolId);
      if (toolItem) {
        toolItem.toolResult = result;
        toolItem.status = result.isError ? "error" : "complete";
        pendingTools.delete(toolId);
      } else {
        renderItems.push({
          type: "tool_call",
          id: toolId || id,
          sourceId: id,
          toolName: item.toolName || item.title || "tool",
          toolInput: null,
          toolResult: result,
          status: result.isError ? "error" : "complete",
          timestamp: item.timestamp,
        });
      }
      continue;
    }

    if (role === "system" || role === "error" || kind === "error") {
      renderItems.push({
        type: "system",
        id,
        subtype: role === "error" || kind === "error" ? "error" : kind,
        content: text || item.title || label(kind),
        timestamp: item.timestamp,
      });
      continue;
    }

    if (text.trim()) {
      renderItems.push({
        type: "text",
        id,
        text,
        timestamp: item.timestamp,
      });
    }
  }

  flushSetup();
  return renderItems;
}

function groupConversationItemsIntoTurns(items) {
  const groups = [];
  let currentTurn = null;
  let turnIndex = 0;

  const flushTurn = () => {
    if (currentTurn && (currentTurn.prompt || currentTurn.items.length)) {
      groups.push(currentTurn);
    }
    currentTurn = null;
  };

  for (const item of items) {
    if (item.type === "session_setup") {
      flushTurn();
      groups.push({ type: "session_setup", items: [item] });
      continue;
    }

    if (item.type === "user_prompt") {
      flushTurn();
      turnIndex += 1;
      currentTurn = {
        type: "work_turn",
        index: turnIndex,
        prompt: item,
        items: [],
      };
    } else {
      if (!currentTurn) {
        turnIndex += 1;
        currentTurn = {
          type: "work_turn",
          index: turnIndex,
          prompt: null,
          items: [],
        };
      }
      currentTurn.items.push(item);
    }
  }
  flushTurn();
  return groups;
}

function renderConversationGroup(group) {
  if (group.type === "session_setup") {
    return group.items.map(renderConversationRenderItem).join("");
  }
  if (group.type === "work_turn") {
    return renderWorkTurn(group);
  }
  const first = group.items?.[0] || {};
  return `
    <div class="assistant-turn" data-turn-id="${escapeHtml(first.id || "")}">
      ${group.items.map(renderConversationRenderItem).join("")}
    </div>
  `;
}

function renderWorkTurn(group) {
  const finalText = findFinalTextItem(group.items);
  const stepCount = group.items.filter((item) => item.type !== "text").length;
  const promptKind = classifyConversationPrompt(group.prompt);
  return `
    <section class="conversation-work-turn" data-turn-id="${escapeHtml(group.prompt?.id || group.items[0]?.id || "")}">
      <div class="work-turn-header">
        <div>
          <div class="work-turn-title">${t("workTurn")} ${group.index}</div>
          <div class="work-turn-subtitle">
            <span>${promptKind.label}</span>
            <span>${stepCount} ${t("executionSteps")}</span>
            <span>${finalText ? t("finalReply") : t("noFinalReply")}</span>
          </div>
        </div>
        ${group.prompt?.timestamp ? `<span class="conversation-timestamp">${fmtTime(group.prompt.timestamp)}</span>` : ""}
      </div>
      ${
        group.prompt
          ? renderUserPromptItem(group.prompt, {
              label: promptKind.label,
              variant: promptKind.variant,
            })
          : ""
      }
      <div class="work-turn-steps assistant-turn">
        ${
          group.items.length
            ? group.items
                .map((item) =>
                  renderConversationRenderItem(item, {
                    isFinalReply: Boolean(finalText && item.id === finalText.id),
                  }),
                )
                .join("")
            : `<div class="empty-inline">${t("noFinalReply")}</div>`
        }
      </div>
    </section>
  `;
}

function findFinalTextItem(items) {
  let hasToolAfter = false;
  for (const item of [...items].reverse()) {
    if (item.type === "tool_call") {
      hasToolAfter = true;
      continue;
    }
    if (item.type === "text" && String(item.text || "").trim() && !hasToolAfter) {
      return item;
    }
  }
  return null;
}

function classifyConversationPrompt(item) {
  const text = String(item?.content || "");
  const isHook =
    /\bhook\b/i.test(text) ||
    /<system-reminder/i.test(text) ||
    /lead[_ -]?pending/i.test(text) ||
    /\binjected\b/i.test(text) ||
    /\[SYSTEM\]/i.test(text);
  return {
    label: isHook ? t("hookInput") : t("receivedInput"),
    variant: isHook ? "hook" : "received",
  };
}

function renderConversationRenderItem(item, options = {}) {
  switch (item.type) {
    case "user_prompt":
      return renderUserPromptItem(item, options);
    case "session_setup":
      return renderSessionSetupItem(item);
    case "thinking":
      return renderThinkingItem(item);
    case "tool_call":
      return renderToolCallItem(item);
    case "system":
      return renderSystemItem(item);
    case "text":
    default:
      return renderTextItem(item, options);
  }
}

function renderUserPromptItem(item, options = {}) {
  const label = options.label || t("receivedInput");
  const variant = options.variant || "received";
  return `
    <div class="conversation-user-prompt ${variant === "hook" ? "conversation-user-prompt-hook" : ""}" data-render-id="${escapeHtml(item.id)}">
      <div class="conversation-step-label">${escapeHtml(label)}</div>
      <div class="message-user-prompt">${renderMarkdownText(item.content || "")}</div>
    </div>
  `;
}

function renderSessionSetupItem(item) {
  return `
    <details class="session-setup-block">
      <summary>${t("sessionSetup")} · ${item.prompts.length}</summary>
      <div class="session-setup-content">
        ${item.prompts
          .map((prompt) => `<div class="message-user-prompt">${renderMarkdownText(prompt.content || "")}</div>`)
          .join("")}
      </div>
    </details>
  `;
}

function renderTextItem(item, options = {}) {
  const isFinalReply = Boolean(options.isFinalReply);
  return `
    <div class="text-block timeline-item ${isFinalReply ? "final-reply-block" : ""}" data-render-id="${escapeHtml(item.id)}">
      ${isFinalReply ? `<div class="conversation-step-label final-reply-label">${t("finalReply")}</div>` : ""}
      <button type="button" class="text-block-copy" data-copy-text="${escapeAttr(item.text || "")}" title="${t("copy")}" aria-label="${t("copy")}">⧉</button>
      ${renderMarkdownText(item.text || "")}
      ${item.timestamp ? `<div class="conversation-timestamp">${fmtTime(item.timestamp)}</div>` : ""}
    </div>
  `;
}

function renderThinkingItem(item) {
  return `
    <details class="thinking-block">
      <summary><span class="thinking-icon">▸</span>${t("thinking")}</summary>
      <div class="thinking-content">${renderMarkdownText(item.thinking || "")}</div>
    </details>
  `;
}

function renderSystemItem(item) {
  return `
    <div class="system-message ${item.subtype === "error" ? "system-message-error" : ""}">
      <span class="system-message-icon">${item.subtype === "error" ? "!" : "⟳"}</span>
      <span class="system-message-text">${escapeHtml(item.content || "")}</span>
    </div>
  `;
}

function renderToolCallItem(item) {
  const expandable = !toolHasCollapsedPreview(item);
  const expandedByDefault = expandable && ["Edit", "TodoWrite"].includes(item.toolName);
  const summary = getToolSummary(item);
  const statusLabel = toolStatusLabel(item.status);
  const preview = toolHasCollapsedPreview(item) ? renderToolCollapsedPreview(item) : "";
  return `
    <div class="tool-row timeline-item ${expandedByDefault ? "expanded" : "collapsed"} status-${escapeHtml(item.status)} ${expandable ? "" : "interactive"}" data-tool-row>
      <div class="tool-row-header ${expandable ? "" : "non-expandable"}" ${expandable ? `role="button" tabindex="0" title="${t("showDetails")}"` : ""}>
        ${item.status === "pending" ? `<span class="tool-spinner" aria-label="${t("running")}"></span>` : ""}
        ${item.status === "aborted" ? `<span class="tool-aborted-icon">×</span>` : ""}
        <span class="tool-name">${escapeHtml(toolDisplayName(item.toolName))}</span>
        <span class="tool-summary">${escapeHtml(summary)}${item.status === "aborted" ? ` <span class="tool-aborted-label">(${t("interrupted")})</span>` : ""}</span>
        <span class="tool-status">${escapeHtml(statusLabel)}</span>
        ${expandable ? `<span class="expand-chevron" aria-hidden="true">${expandedByDefault ? "▾" : "▸"}</span>` : ""}
      </div>
      ${preview}
      ${
        expandable
          ? `<div class="tool-row-content">${renderToolExpandedContent(item)}</div>`
          : ""
      }
    </div>
  `;
}

function renderToolExpandedContent(item) {
  if (item.status === "pending" || item.status === "aborted") {
    return renderToolUseContent(item.toolName, item.toolInput);
  }
  return renderToolResultContent(item.toolName, item.toolResult);
}

function renderToolUseContent(toolName, input) {
  if (toolName === "Bash" && input && typeof input === "object") {
    const command = input.command || input.cmd || "";
    if (command) {
      return `<pre class="code-block"><code>${escapeHtml(command)}</code></pre>`;
    }
  }
  return `<pre class="code-block"><code>${escapeHtml(prettyValue(input))}</code></pre>`;
}

function renderToolResultContent(toolName, result) {
  if (!result) {
    return `<div class="tool-no-result">${t("none")}</div>`;
  }
  const content = result.structured ?? result.content ?? "";
  const className = result.isError ? "code-block code-block-error" : "code-block";
  return `<pre class="${className}"><code>${escapeHtml(prettyValue(content))}</code></pre>`;
}

function renderToolCollapsedPreview(item) {
  const resultText = item.toolResult ? prettyValue(item.toolResult.structured ?? item.toolResult.content ?? "") : "";
  const preview = resultText
    .split("\n")
    .filter(Boolean)
    .slice(0, 6)
    .join("\n");
  if (!preview) return "";
  return `<pre class="tool-row-collapsed-preview"><code>${escapeHtml(preview)}</code></pre>`;
}

function toolHasCollapsedPreview(item) {
  return item.status !== "pending" && Boolean(item.toolResult?.content) && ["Bash", "Read", "Grep", "Glob"].includes(item.toolName);
}

function getToolSummary(item) {
  const inputSummary = getToolInputSummary(item.toolName, item.toolInput);
  if (item.status === "pending" || item.status === "aborted") return inputSummary;
  if (item.status === "error") return `${inputSummary} → ${t("failed")}`;
  const lineCount = String(item.toolResult?.content || "").split("\n").filter(Boolean).length;
  if (["Read", "Bash"].includes(item.toolName) && lineCount) return `${inputSummary} → ${lineCount} lines`;
  if (["Glob"].includes(item.toolName) && lineCount) return `${inputSummary} → ${lineCount} files`;
  if (["Grep"].includes(item.toolName) && lineCount) return `${inputSummary} → ${lineCount} matches`;
  return `${inputSummary} → ${t("complete")}`;
}

function getToolInputSummary(toolName, input) {
  const obj = input && typeof input === "object" ? input : {};
  if (["Read", "Write", "Edit"].includes(toolName) && obj.file_path) return fileName(obj.file_path);
  if (toolName === "Bash" && (obj.command || obj.cmd)) return String(obj.command || obj.cmd);
  if (["Glob", "Grep"].includes(toolName) && obj.pattern) return String(obj.pattern);
  if (["Task", "Agent"].includes(toolName) && obj.description) return truncate(String(obj.description), 48);
  if (toolName === "WebSearch" && obj.query) return truncate(String(obj.query), 48);
  if (toolName === "WebFetch" && obj.url) return truncate(String(obj.url), 60);
  const first = Object.values(obj).find((value) => typeof value === "string" && value.trim());
  return first ? truncate(String(first), 60) : "...";
}

function toolStatusLabel(status) {
  if (status === "pending") return t("running");
  if (status === "error") return t("failed");
  if (status === "aborted") return t("interrupted");
  return t("complete");
}

function toolDisplayName(name) {
  return name || "Tool";
}

function isSessionSetupText(text) {
  const trimmed = String(text || "").trimStart();
  return trimmed.startsWith("# AGENTS.md instructions") || trimmed.startsWith("<environment_context>");
}

function parseMaybeJson(text) {
  if (!text || typeof text !== "string") return null;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function prettyValue(value) {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function renderMarkdownText(text) {
  const raw = String(text || "");
  if (!raw.trim()) return `<p>${t("none")}</p>`;
  const parts = [];
  const regex = /```([A-Za-z0-9_-]*)\n?([\s\S]*?)```/g;
  let lastIndex = 0;
  let match;
  while ((match = regex.exec(raw))) {
    if (match.index > lastIndex) {
      parts.push(renderMarkdownParagraphs(raw.slice(lastIndex, match.index)));
    }
    parts.push(`<pre class="code-block"><code>${escapeHtml(match[2] || "")}</code></pre>`);
    lastIndex = regex.lastIndex;
  }
  if (lastIndex < raw.length) {
    parts.push(renderMarkdownParagraphs(raw.slice(lastIndex)));
  }
  return parts.join("");
}

function renderMarkdownParagraphs(text) {
  return String(text)
    .split(/\n{2,}/)
    .map((paragraph) => paragraph.trim())
    .filter(Boolean)
    .map((paragraph) => `<p>${renderInlineMarkdown(paragraph).replace(/\n/g, "<br>")}</p>`)
    .join("");
}

function renderInlineMarkdown(text) {
  return String(text)
    .split(/(`[^`]+`)/g)
    .map((part) => {
      if (part.startsWith("`") && part.endsWith("`") && part.length > 1) {
        return `<code>${escapeHtml(part.slice(1, -1))}</code>`;
      }
      return escapeHtml(part);
    })
    .join("");
}

function preserveDetailScroll(renderFn, { forceBottom = false } = {}) {
  const pane = $("detailPane");
  const previousTop = pane?.scrollTop || 0;
  const previousHeight = pane?.scrollHeight || 0;
  const nearBottom = pane ? pane.scrollHeight - pane.scrollTop - pane.clientHeight < 120 : true;
  renderFn();
  if (!pane) return;
  nextFrame(() => {
    if (forceBottom || nearBottom) {
      pane.scrollTop = pane.scrollHeight;
    } else {
      pane.scrollTop = previousTop + Math.max(0, pane.scrollHeight - previousHeight);
    }
  });
}

function nextFrame(callback) {
  const raf = window.requestAnimationFrame || globalThis.requestAnimationFrame;
  if (typeof raf === "function") {
    raf(callback);
    return;
  }
  const timeout = window.setTimeout || globalThis.setTimeout;
  timeout(callback, 0);
}

function fileName(path) {
  return String(path || "").split(/[\\/]/).pop() || String(path || "");
}

function truncate(text, max) {
  const value = String(text || "");
  return value.length <= max ? value : `${value.slice(0, Math.max(0, max - 3))}...`;
}

async function refreshMemberDetail(name, { force = false } = {}) {
  const teamId = state.teamId;
  if (!teamId || !name) return;

  const key = memberDetailKey(teamId, name);
  if (state.memberDetailLoadingKey === key) {
    return;
  }
  if (!force && state.memberDetailKey === key && state.memberDetail && state.memberActivity) {
    return;
  }

  const requestSeq = (state.detailRequestSeq += 1);
  state.memberDetailLoadingKey = key;

  try {
    const [data, activity] = await Promise.all([
      api(`/api/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(name)}`),
      api(`/api/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(name)}/activity`),
    ]);
    if (
      requestSeq !== state.detailRequestSeq ||
      state.teamId !== teamId ||
      state.selectedMemberName !== name
    ) {
      return;
    }
    state.memberDetailKey = key;
    state.memberDetail = data;
    state.memberActivity = activity;
    state.memberDetailLoadingKey = "";
    state.detailError = "";
    renderMemberDetailContent(name, data, activity);
  } catch (error) {
    if (state.teamId !== teamId || state.selectedMemberName !== name) {
      return;
    }
    state.memberDetailLoadingKey = "";
    if (force && state.memberDetailKey === key && state.memberDetail && state.memberActivity) {
      state.refreshError = localizedError("refreshFailed", error);
      renderHeader();
      renderFooter();
      return;
    }
    state.detailError = localizedError("failedLoadMemberDetail", error);
    state.refreshError = localizedError("refreshFailed", error);
    $("detailTitle").textContent = isLeadName(name) ? t("leadActivity") : `${t("member")} ${name}`;
    $("detailBody").innerHTML = `
      <div class="empty">${escapeHtml(state.detailError)}</div>
      <button class="chip retry-button" type="button" id="retryDetailButton">${t("retry")}</button>
    `;
    const retryButton = $("retryDetailButton");
    if (retryButton) {
      retryButton.addEventListener("click", async () => {
        state.detailError = "";
        await retryDetail();
      });
    }
  }
}

function renderMemberDetailContent(name, data, activity) {
  const profile = data.profile || {};
  const execution = data.execution || {};
  const summary = data.activity || {};
  const activityItems = Array.isArray(activity.items) ? activity.items : [];
  const envKeys = Array.isArray(execution.envKeys) ? execution.envKeys : [];
  const redactedEnv = execution.redactedEnv || {};
  const isLead = profile.kind === "lead" || isLeadName(name);
  $("detailTitle").textContent = isLead ? t("leadActivity") : `${t("member")} ${name}`;
  $("detailBody").innerHTML = `
      <div class="detail-card">
        <div class="detail-head">
          ${renderSenderBadge(profile.name || name, profile.kind)}
          <span class="badge ${isLead ? "lead" : "worker"}">${escapeHtml(label(profile.kind || "member"))}</span>
          <span class="pill">${escapeHtml(label(execution.sessionState || "unknown"))}</span>
          <span class="pill">${escapeHtml(label(profile.roleLabel || ""))}</span>
        </div>
        <div class="subtle">${escapeHtml(profile.name || name)} · ${fmtTime(profile.joinedAt)}</div>
      </div>
      <div class="detail-card">
        <div class="section-title">${isLead ? t("leadCoordination") : t("profile")}</div>
        <div class="detail-grid">
          <div><span class="muted">${t("name")}</span><div>${escapeHtml(profile.name || name)}</div></div>
          <div><span class="muted">${t("kind")}</span><div>${escapeHtml(label(profile.kind || "member"))}</div></div>
          <div><span class="muted">${t("role")}</span><div>${escapeHtml(label(profile.roleLabel || ""))}</div></div>
          <div><span class="muted">${t("status")}</span><div>${escapeHtml(label(profile.status || "unknown"))}</div></div>
        </div>
      </div>
      <div class="detail-card">
        <div class="section-title">${t("executionSnapshot")}</div>
        <div class="detail-grid">
          <div><span class="muted">${t("executionMode")}</span><div>${escapeHtml(label(execution.executionMode || "unknown"))}</div></div>
          <div><span class="muted">${t("sessionState")}</span><div>${escapeHtml(label(execution.sessionState || "unknown"))}</div></div>
          <div><span class="muted">${t("adapter")}</span><div>${escapeHtml(label(execution.adapter || "n/a"))}</div></div>
          <div><span class="muted">${t("model")}</span><div>${escapeHtml(label(execution.model || "n/a"))}</div></div>
          <div><span class="muted">${t("cwd")}</span><div>${escapeHtml(execution.cwd || na())}</div></div>
          <div><span class="muted">${t("systemPrompt")}</span><div>${execution.hasSystemPrompt ? t("folded") : t("none")}</div></div>
        </div>
        <div class="detail-list-block">
          <div class="muted">${t("environment")}</div>
          <div class="detail-pills">${
            envKeys.length
              ? envKeys.map((key) => `<span class="pill">${escapeHtml(key)}=${escapeHtml(redactedEnv[key] ?? na())}</span>`).join("")
              : `<span class="empty-inline">${t("none")}</span>`
          }</div>
        </div>
      </div>
      <div class="detail-card">
        <div class="section-title">${isLead ? t("leadCoordination") : t("recentActivity")}</div>
        <div class="detail-list-block">
          <div class="detail-pills">
            <span class="pill">${summary.sentCount || 0} ${t("sent")}</span>
            <span class="pill">${summary.receivedCount || 0} ${t("received")}</span>
            <span class="pill">${summary.mentionedCount || 0} ${t("mentioned")}</span>
          </div>
          ${
            activityItems.length
              ? activityItems
                  .slice(0, 8)
                  .map(
                    (item) => `
                      <div class="detail-item">
                        <div class="detail-head">
                          <span class="badge worker">${escapeHtml(label(item.itemType))}</span>
                          <span class="pill">${fmtTime(item.createdAt)}</span>
                        </div>
                        <div class="message-body">${escapeHtml(localText(item.summary))}</div>
                      </div>
                    `,
                  )
                  .join("")
              : `<div class="empty">${t("noMessages")}</div>`
          }
        </div>
      </div>
      <details class="detail-card">
        <summary>${t("rawJson")}</summary>
        <pre class="code-block">${escapeHtml(JSON.stringify(data, null, 2))}</pre>
      </details>
    `;
}

function isLeadName(name) {
  return leadMemberName() === name || state.members.find((member) => member.name === name)?.kind === "lead";
}

async function retryDetail() {
  if (state.failedTeamId) {
    const teamId = state.failedTeamId;
    state.failedTeamId = null;
    state.detailError = "";
    state.teamError = "";
    state.refreshError = "";
    await loadTeam(teamId);
    return;
  }
  if (state.selectedMessageId) {
    const message = allMessages().find((item) => item.id === state.selectedMessageId);
    if (message) {
      renderMessageDetail(message);
      return;
    }
  }
  if (state.selectedMemberName) {
    await refreshMemberDetail(state.selectedMemberName, { force: true });
  }
}

function renderFooter() {
  const messages = allMessages();
  const visible = filteredMessages().length;
  const statusBits = [`${visible}/${messages.length} ${t("messagesVisible")}`];
  if (state.teamError) statusBits.push(t("teamLoadFailed"));
  if (state.refreshError) statusBits.push(state.refreshError);
  $("countsSummary").textContent = statusBits.join(" · ");
}

function renderEmptyOrErrorBanner() {
  const banner = $("banner");
  if (!banner) return;
  if (state.teamError) {
    banner.className = "banner error";
    banner.textContent = state.teamError;
    banner.hidden = false;
    return;
  }
  if (!state.teamId && !state.loadingTeams) {
    banner.className = "banner empty";
    banner.textContent = t("noTeams");
    banner.hidden = false;
    return;
  }
  banner.hidden = true;
}

function bindColumnResizer(splitterId, side) {
  const splitter = $(splitterId);
  if (!splitter) {
    return;
  }

  let dragState = null;

  const move = (event) => {
    if (!dragState) {
      return;
    }
    const delta = event.clientX - dragState.startX;
    const nextWidth =
      side === "left" ? dragState.startWidth + delta : dragState.startWidth - delta;
    state.columnWidths[side] = clamp(
      Math.round(nextWidth),
      COLUMN_LIMITS[side].min,
      COLUMN_LIMITS[side].max,
    );
    applyColumnWidths();
  };

  const stop = () => {
    if (!dragState) {
      return;
    }
    dragState = null;
    splitter.classList?.remove("dragging");
    saveColumnWidths();
    window.removeEventListener?.("pointermove", move);
    window.removeEventListener?.("pointerup", stop);
    window.removeEventListener?.("pointercancel", stop);
  };

  splitter.addEventListener("pointerdown", (event) => {
    if (window.matchMedia?.("(max-width: 960px)").matches) {
      return;
    }
    event.preventDefault?.();
    dragState = {
      startX: event.clientX,
      startWidth: resolvedPaneWidth(side),
    };
    splitter.classList?.add("dragging");
    splitter.setPointerCapture?.(event.pointerId);
    window.addEventListener?.("pointermove", move);
    window.addEventListener?.("pointerup", stop);
    window.addEventListener?.("pointercancel", stop);
  });

  splitter.addEventListener("keydown", (event) => {
    if (!["ArrowLeft", "ArrowRight"].includes(event.key)) {
      return;
    }
    event.preventDefault?.();
    const direction = event.key === "ArrowRight" ? 1 : -1;
    const delta = side === "left" ? direction * 16 : direction * -16;
    state.columnWidths[side] = clamp(
      resolvedPaneWidth(side) + delta,
      COLUMN_LIMITS[side].min,
      COLUMN_LIMITS[side].max,
    );
    applyColumnWidths();
    saveColumnWidths();
  });
}

function bindConversationEvents() {
  $("detailBody").addEventListener("click", async (event) => {
    const copyButton = event.target.closest?.(".text-block-copy");
    if (copyButton) {
      const text = copyButton.dataset.copyText || "";
      try {
        await navigator.clipboard?.writeText(text);
        copyButton.textContent = "✓";
        copyButton.classList.add("copied");
        copyButton.setAttribute("title", t("copied"));
        window.setTimeout(() => {
          copyButton.textContent = "⧉";
          copyButton.classList.remove("copied");
          copyButton.setAttribute("title", t("copy"));
        }, 1200);
      } catch {
        copyButton.textContent = "!";
      }
      return;
    }

    const header = event.target.closest?.(".tool-row-header");
    if (!header || header.classList.contains("non-expandable")) {
      return;
    }
    toggleToolRow(header.closest(".tool-row"));
  });

  $("detailBody").addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") {
      return;
    }
    const header = event.target.closest?.(".tool-row-header");
    if (!header || header.classList.contains("non-expandable")) {
      return;
    }
    event.preventDefault();
    toggleToolRow(header.closest(".tool-row"));
  });
}

function toggleToolRow(row) {
  if (!row) return;
  const expanded = row.classList.toggle("expanded");
  row.classList.toggle("collapsed", !expanded);
  const chevron = row.querySelector(".expand-chevron");
  if (chevron) {
    chevron.textContent = expanded ? "▾" : "▸";
  }
}

function bindEvents() {
  bindColumnResizer("leftSplitter", "left");
  bindColumnResizer("rightSplitter", "right");
  bindConversationEvents();

  window.addEventListener?.("resize", () => {
    if (rightPaneUsesBalancedWidth()) {
      applyColumnWidths();
    }
  });

  $("teamSelect").addEventListener("change", (event) => {
    applyTeamDeepLink(event.target.value || null);
    state.selectedMessageId = null;
    state.selectedMemberName = null;
    state.detailTab = "session";
    clearMemberDetailCache();
    if (state.teamId) {
      loadTeam(state.teamId);
    } else {
      state.room = null;
      state.members = [];
      renderShell();
    }
  });

  $("searchInput").addEventListener("input", (event) => {
    state.search = event.target.value;
    renderShell();
  });

  $("languageToggleButton").addEventListener("click", () => {
    state.language = state.language === "zh" ? "en" : "zh";
    renderShell();
  });

  $("detailTabButton").addEventListener("click", () => {
    state.detailTab = "detail";
    renderShell();
  });

  $("sessionTabButton").addEventListener("click", async () => {
    state.detailTab = "session";
    renderShell();
    if (state.selectedMemberName) {
      await refreshMemberConversation(state.selectedMemberName);
    }
  });

  $("diagnosticsTabButton").addEventListener("click", async () => {
    state.detailTab = "diagnostics";
    renderShell();
    if (state.teamId && !state.diagnostics && !state.diagnosticsLoading && !state.diagnosticsError) {
      state.diagnosticsLoading = true;
      renderShell();
      await loadDiagnostics(state.teamId);
    }
  });

  $("reloadButton").addEventListener("click", () => {
    if (state.teamId) {
      loadTeam(state.teamId);
    } else {
      loadTeams();
    }
  });

  $("clearFiltersButton").addEventListener("click", () => {
    state.search = "";
    state.senderFilter = "";
    state.mentionFilter = "";
    $("searchInput").value = "";
    renderShell();
  });

  $("focusLeadButton").addEventListener("click", async () => {
    const lead = leadMemberName();
    if (lead) {
      state.selectedMessageId = null;
      state.selectedMemberName = lead;
      state.detailTab = "session";
      clearMemberDetailCache();
      state.detailError = "";
      setDeepLink("member", lead);
      renderShell();
    }
  });

  window.addEventListener("hashchange", () => {
    state.deepLink = parseDeepLink();
    if (state.deepLink.team && state.deepLink.team !== state.teamId) {
      loadTeam(state.deepLink.team);
      return;
    }
    if (state.teamId) {
      applyDeepLink();
      renderShell();
    }
  });

  const composerForm = document.getElementById("composerForm");
  if (composerForm) {
    composerForm.addEventListener("submit", (event) => {
      event.preventDefault();
      submitComposer();
    });
  }
  const composerInput = document.getElementById("composerInput");
  if (composerInput) {
    // Enter sends; Shift+Enter inserts a newline.
    composerInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        submitComposer();
      }
    });
  }
  const composerSelect = document.getElementById("composerMention");
  if (composerSelect) {
    composerSelect.addEventListener("change", (event) => {
      state.composerMention = event.target.value;
    });
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  bindEvents();
  const deepLink = parseDeepLink();
  state.deepLink = deepLink.team || deepLink.message || deepLink.member ? deepLink : null;
  renderShell();
  await loadTeams();
  startAutoRefresh();
});
