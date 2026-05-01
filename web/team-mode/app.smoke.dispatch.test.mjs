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

test("dashboard attributes worker replies to the sender", async () => {
  const payloads = basePayloads();
  const fakeSse = createFakeEventSourceHarness();
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });
  harness.context.window.EventSource = fakeSse.FakeEventSource;

  await harness.start();
  fakeSse.instances[0].dispatchOpen();
  fakeSse.instances[0].dispatchEventFrame(
    messageEvent("demo", "m-worker-reply", "Worker reply complete", ["lead"], "c-worker-reply", {
      sender: "alice",
      senderKind: "member",
      kind: "reply",
      replyTo: "m-lead-dispatch",
      threadId: "thread-1",
    }),
  );
  await flushPromises();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  const agents = harness.context.state.dashboard.data.agents;
  const alice = agents.find((agent) => agent.name === "alice");
  const lead = agents.find((agent) => agent.name === "lead");
  assert.ok(alice?.tasks.some((task) => task.id === "demo:message:m-worker-reply"));
  assert.equal(lead?.tasks.some((task) => task.id === "demo:message:m-worker-reply"), undefined);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /alice/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /Worker reply complete/);
});

test("dashboard keeps lead dispatch attribution on mentioned workers", async () => {
  const payloads = basePayloads();
  const fakeSse = createFakeEventSourceHarness();
  const harness = createHarness({
    fetchImpl: async (url) => payloads[url] ?? failedJson(404, "Not Found"),
  });
  harness.context.window.EventSource = fakeSse.FakeEventSource;

  await harness.start();
  fakeSse.instances[0].dispatchOpen();
  fakeSse.instances[0].dispatchEventFrame(
    messageEvent("demo", "m-lead-dispatch", "Please handle dispatch", ["frontend-dev"], "c-lead-dispatch"),
  );
  await flushPromises();
  await harness.document.getElementById("dashboardViewButton").dispatch("click");

  const agents = harness.context.state.dashboard.data.agents;
  const frontend = agents.find((agent) => agent.name === "frontend-dev");
  const lead = agents.find((agent) => agent.name === "lead");
  assert.ok(frontend?.tasks.some((task) => task.id === "demo:message:m-lead-dispatch"));
  assert.equal(lead?.tasks.some((task) => task.id === "demo:message:m-lead-dispatch"), undefined);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /frontend-dev/);
  assert.match(harness.document.getElementById("dashboardRoot").innerHTML, /Please handle dispatch/);
});


test("sending a message forces the timeline to the bottom", async () => {
  const payloads = basePayloads();
  const roomPayload = await payloads["/api/teams/demo/rooms/main?limit=200"].json();
  payloads["/api/teams/demo/rooms/main?limit=200"] = okJson(roomPayload);
  const harness = createHarness({
    fetchImpl: async (url, options = {}) => {
      if (url === "/api/teams/demo/rooms/main/messages" && options.method === "POST") {
        roomPayload.messages.push({
          id: "m3",
          sender: "user",
          senderKind: "member",
          kind: "dispatch",
          body: "@alice manual send",
          bodyPreview: "@alice manual send",
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
        return okJson({ id: "m3" });
      }
      return payloads[url] ?? failedJson(404, "Not Found");
    },
  });

  await harness.start();
  await flushTimers();

  const messageList = harness.document.getElementById("messageList");
  messageList.clientHeight = 100;
  messageList.scrollHeight = 1000;
  messageList.scrollTop = 240;
  harness.document.getElementById("composerMention").value = "alice";
  harness.document.getElementById("composerInput").value = "manual send";

  await harness.context.submitComposer();
  messageList.scrollHeight = 1300;
  await flushTimers();

  assert.equal(messageList.scrollTop, 1300);
});
