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
  composerMentions: [],
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
    coordinator: "主协调",
    running: "运行中",
    starting: "启动中",
    failed: "失败",
    dead: "已离线",
    stopped: "已停止",
    paused: "已暂停",
    not_spawned: "未启动",
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
  en: {
    coordinator: "Coordinator",
    running: "Running",
    starting: "Starting",
    failed: "Failed",
    dead: "Offline",
    stopped: "Stopped",
    paused: "Paused",
    not_spawned: "Not spawned",
    unknown: "Unknown",
  },
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

Object.assign(globalThis, {
  REFRESH_INTERVAL_MS,
  COLUMN_LIMITS,
  TIMELINE_BOTTOM_THRESHOLD_PX,
  state,
  $,
  refreshTimer,
  STRINGS,
  t,
  localizedError,
  messageCountLabel,
  na,
  clamp,
  loadColumnWidths,
  saveColumnWidths,
  applyColumnWidths,
  resolvedRightPaneWidth,
  resolvedPaneWidth,
  rightPaneUsesBalancedWidth,
  label,
  sourceLabel,
  localText,
});
