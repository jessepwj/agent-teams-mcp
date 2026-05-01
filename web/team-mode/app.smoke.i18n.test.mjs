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
  assert.equal(harness.document.getElementById("dashboardViewButton").textContent, "Dashboard");
  assert.equal(harness.document.getElementById("detailTitle").textContent, "Process Session · lead");
  assert.match(harness.document.getElementById("detailBody").innerHTML, /团队状态正常/);
});


test("dashboard zh mode localizes dashboard chrome and transport labels", async () => {
  const payloads = basePayloads();
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });

  await harness.start();
  harness.context.setDashboardMockState("ready", {
    data: {
      workers: [{ name: "前端", status: "alive", adapter: "本地", sessionId: "会话1", role: "界面" }],
      agents: [{ name: "前端", tasks: [{ id: "任务1", label: "处理回复", state: "active" }] }],
    },
    transport: { mode: "polling", source: "fallback", polling: "connected" },
  });
  harness.context.renderDashboardShell();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  const dashboardHtml = harness.document.getElementById("dashboardRoot").innerHTML;
  const liveStatus = harness.document.getElementById("liveStatus").textContent;
  assert.equal(harness.document.getElementById("dashboardViewButton").textContent, "仪表盘");
  assert.match(dashboardHtml, /团队仪表盘/);
  assert.match(dashboardHtml, /成员状态/);
  assert.match(dashboardHtml, /适配器/);
  assert.match(dashboardHtml, /会话/);
  assert.match(dashboardHtml, /在线/);
  assert.match(dashboardHtml, /轮询备用/);
  assert.match(liveStatus, /轮询备用/);
  assert.doesNotMatch(`${dashboardHtml} ${liveStatus}`, /Dashboard|polling|sse connected|sse reconnecting|mock fixture|Worker Status|Adapter|Session/);
});
