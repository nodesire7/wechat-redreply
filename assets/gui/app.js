(() => {
  "use strict";
  const token = new URLSearchParams(location.search).get("token") || "";
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const state = { config: null, status: {}, windows: [], contacts: [], page: "account", filter: 311, paused: false, dirty: false };
  let toastTimer = 0;

  async function api(path, body) {
    const url = "/api/" + path + (path.includes("?") ? "&" : "?") + "token=" + encodeURIComponent(token);
    const options = { cache: "no-store", headers: { "X-Token": token } };
    if (body !== undefined) {
      options.method = "POST";
      options.headers["Content-Type"] = "application/json";
      options.body = JSON.stringify(body);
    }
    const response = await fetch(url, options);
    const text = await response.text();
    let data;
    try { data = JSON.parse(text); } catch { data = { error: text }; }
    if (!response.ok || data.error) throw new Error(data.error || ("HTTP " + response.status));
    return data;
  }

  function toast(message) {
    const el = $("#toast");
    el.textContent = message;
    el.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.classList.add("hidden"), 4200);
  }
  function notice(message) { $("#notice").textContent = message; }
  function setDirty(value = true) { state.dirty = value; }

  function configPayload() {
    return {
      listen_host: $("#listen-host").value.trim(),
      connect_host: $("#connect-host").value.trim(),
      port: Number($("#port").value),
      self_id: Number($("#self-id").value),
      access_token: $("#token").value,
      management_url: $("#management-url").value.trim(),
      tls_certfile: $("#tls-cert").value.trim(),
      tls_keyfile: $("#tls-key").value.trim(),
      whitelist: $("#whitelist").value.split(/\r?\n/).map(s => s.trim()).filter(Boolean),
      group_whitelist: $("#group-whitelist").value.split(/\r?\n/).map(s => s.trim()).filter(Boolean),
      interval: Number($("#interval").value),
      recall_limit: Number($("#recall-limit").value),
      forward_staging_chat: $("#staging-chat").value.trim(),
      friend_accept: $("#friend-accept").checked
    };
  }

  function fill(config) {
    state.config = config;
    $("#listen-host").value = config.listen_host ?? "";
    $("#connect-host").value = config.connect_host ?? "";
    $("#port").value = config.port ?? "";
    $("#self-id").value = config.self_id ?? "";
    $("#token").value = config.access_token ?? "";
    $("#management-url").value = config.management_url ?? "";
    $("#tls-cert").value = config.tls_certfile ?? "";
    $("#tls-key").value = config.tls_keyfile ?? "";
    $("#whitelist").value = (config.whitelist || []).join("\n");
    $("#group-whitelist").value = (config.group_whitelist || []).join("\n");
    $("#interval").value = config.interval ?? 0.2;
    $("#recall-limit").value = config.recall_limit ?? 120;
    $("#staging-chat").value = config.forward_staging_chat ?? "";
    $("#friend-accept").checked = !!config.friend_accept;
    $("#scope-ids").textContent = (config.scope_ids || []).join("   ");
    setDirty(false);
  }

  function renderBinding(binding) {
    const valid = !!(binding && binding.valid);
    $("#binding-pill").textContent = valid ? "已绑定" : "未绑定";
    $("#binding-note").textContent = valid ? (binding.label || "已绑定当前微信账号") : "只绑定你明确选择的微信账号";
  }

  function renderStatus(status) {
    const running = !!status.running;
    const ready = !!status.ready;
    const connected = !!status.connected;
    $("#wechat-state").textContent = running && ready ? "微信 已绑定" : (status.binding && status.binding.valid ? "微信 已绑定" : "微信 未绑定");
    $("#red-state").textContent = running && connected ? "RedReply 已连接" : "RedReply 未连接";
    $("#wechat-dot").className = "dot " + (ready || (status.binding && status.binding.valid) ? "on" : "");
    $("#red-dot").className = "dot " + (connected ? "on" : "");
    $("#sidebar-status-text").textContent = running ? (connected ? "运行中 · 已连接" : "运行中 · 等待 RedReply") : "监听已停止";
    $("#sidebar-status .dot").className = "dot " + (running && ready ? "on" : "");
    const start = $("#start-btn"), stop = $("#stop-btn");
    start.classList.toggle("hidden", running);
    stop.classList.toggle("hidden", !running);
    start.disabled = running;
    if (running) notice(connected ? "监听已运行，配置已应用。关闭本窗口不会停止监听。" : "微信数据库已就绪，等待 RedReply 连接。");
    if (status.error) notice(status.error);
  }

  async function refreshWindows(selectCurrent = true) {
    try {
      const data = await api("windows");
      state.windows = data.windows || [];
      const select = $("#window-select");
      select.innerHTML = "";
      const empty = document.createElement("option");
      empty.value = ""; empty.textContent = "请选择微信窗口";
      select.appendChild(empty);
      state.windows.forEach((w, i) => {
        const option = document.createElement("option");
        option.value = String(i);
        option.textContent = `${w.title}　PID ${w.pid}　${w.offscreen ? "已离屏" : "当前可见"}`;
        select.appendChild(option);
      });
      const bound = data.bound;
      if (selectCurrent && bound) {
        const index = state.windows.findIndex(w => w.hwnd === bound.hwnd && w.pid === bound.pid);
        if (index >= 0) select.value = String(index);
      }
      if (!select.value && state.windows.length === 1) select.value = "0";
    } catch (error) { toast(error.message); }
  }

  async function refreshState() {
    try {
      const data = await api("state");
      fill(data.config);
      renderBinding(data.binding);
      state.status = data.status || {};
      renderStatus(state.status);
      await refreshWindows(true);
    } catch (error) { toast(error.message); }
  }

  async function pollStatus() {
    try {
      const data = await api("status");
      state.status = data.status || {};
      renderStatus(data.status);
      if (data.binding) renderBinding(data.binding);
      const outbox = data.outbox || {};
      $("#queue-stats").textContent = `发送队列：待发 ${outbox.queued || 0} · 准备 ${outbox.preparing || 0} · 发送 ${outbox.sending || 0} · 成功 ${outbox.sent || 0} · 失败 ${outbox.failed || 0} · 待核对 ${outbox.uncertain || 0}`;
      if (state.page === "logs" && !state.paused) await loadLogs();
      if (state.page === "queue") await loadQueue();
    } catch { /* the status line is informative; transient failures are not toasted */ }
  }

  async function save(silent = false) {
    try {
      const data = await api("save", configPayload());
      fill(data.config);
      renderBinding(data.binding);
      toast(data.notice || "设置已保存");
      if (!silent) notice(data.notice || "设置已保存，白名单和查询间隔自动应用");
    } catch (error) { toast(error.message); notice(error.message); }
  }

  async function bind() {
    const value = $("#window-select").value;
    if (value === "") { toast("请先选择一个微信窗口"); return; }
    const selected = state.windows[Number(value)];
    if (!selected) { toast("所选窗口已变化，请刷新"); return; }
    try {
      const data = await api("bind", { hwnd: selected.hwnd, pid: selected.pid, created_at: selected.created_at });
      fill(data.config);
      renderBinding(data.binding);
      await refreshWindows(false);
      toast(data.notice || "账号已绑定并保存");
    } catch (error) { toast(error.message); }
  }

  async function start() {
    try {
      await save(true);
      const data = await api("start", {});
      toast(data.notice || "监听正在启动");
      state.dirty = false;
      setTimeout(pollStatus, 600);
    } catch (error) { toast(error.message); }
  }
  async function stop() {
    try {
      const data = await api("stop", {});
      toast(data.notice || "正在停止监听");
      setTimeout(pollStatus, 600);
    } catch (error) { toast(error.message); }
  }

  async function loadContacts() {
    const button = $("#load-contacts");
    button.disabled = true; button.textContent = "正在读取…";
    try {
      const data = await api("contacts", {});
      state.contacts = data.contacts || [];
      const select = $("#contact-select");
      select.innerHTML = "";
      const empty = document.createElement("option");
      empty.value = ""; empty.textContent = state.contacts.length ? "请选择联系人" : "没有读到联系人";
      select.appendChild(empty);
      state.contacts.forEach((c, i) => {
        const option = document.createElement("option");
        option.value = String(i);
        option.textContent = `${c.group ? "群聊" : "私聊"} · ${c.name}`;
        select.appendChild(option);
      });
      if (state.contacts.length) select.value = "0";
      toast(`已读取 ${state.contacts.length} 个联系人`);
    } catch (error) { toast(error.message); }
    finally { button.disabled = false; button.textContent = "读取联系人"; }
  }

  async function addWhitelist() {
    const item = state.contacts[Number($("#contact-select").value)];
    if (!item) { toast("请先读取并选择联系人"); return; }
    const area = item.group ? $("#group-whitelist") : $("#whitelist");
    const lines = area.value.split(/\r?\n/).map(s => s.trim()).filter(Boolean);
    if (!lines.includes(item.name)) {
      lines.push(item.name);
      area.value = lines.join("\n");
      setDirty(true);
    }
    await save(false);
    toast("已加入白名单并保存");
  }

  async function copyText(text, message) {
    if (!text) { toast("没有可复制的内容"); return; }
    try { await api("copy", { text }); toast(message || "已复制"); }
    catch (error) { toast(error.message); }
  }

  async function loadLogs() {
    const view = $("#log-text");
    try {
      const data = await api("logs?filter=" + state.filter);
      const atBottom = view.scrollTop + view.clientHeight >= view.scrollHeight - 12;
      const keep = view.scrollTop;
      const next = data.text || "尚无此类记录。";
      if (view.textContent !== next) {
        view.textContent = next;
        if (atBottom) view.scrollTop = view.scrollHeight;
        else view.scrollTop = keep;
      }
      $("#log-count").textContent = `${view.textContent.split("\n").filter(Boolean).length} 行`;
    } catch (error) { view.textContent = error.message; }
  }
  async function loadQueue() {
    try {
      const data = await api("logs?filter=318");
      $("#queue-text").textContent = data.text || "暂无发送任务。";
    } catch (error) { $("#queue-text").textContent = error.message; }
  }

  function go(page) {
    state.page = page;
    $$(".nav-item").forEach(b => b.classList.toggle("active", b.dataset.page === page));
    $$(".page").forEach(p => p.classList.toggle("active", p.id === "page-" + page));
    if (page === "logs") loadLogs();
    if (page === "queue") loadQueue();
  }

  function wire() {
    $$(".nav-item").forEach(button => button.addEventListener("click", () => go(button.dataset.page)));
    $("#refresh-windows").addEventListener("click", () => refreshWindows(true));
    $("#bind-account").addEventListener("click", bind);
    $("#start-btn").addEventListener("click", start);
    $("#stop-btn").addEventListener("click", stop);
    $("#scope-start").addEventListener("click", start);
    $("#save-btn").addEventListener("click", () => save(false));
    $$("[data-save]").forEach(b => b.addEventListener("click", () => save(false)));
    $("#reload-btn").addEventListener("click", async () => {
      if (state.dirty && !confirm("放弃未保存修改并重新载入？")) return;
      await refreshState(); toast("已重新载入配置");
    });
    $("#load-contacts").addEventListener("click", loadContacts);
    $("#add-whitelist").addEventListener("click", addWhitelist);
    $("#copy-contact-id").addEventListener("click", async () => {
      const item = state.contacts[Number($("#contact-select").value)];
      if (!item) { toast("请先读取并选择联系人"); return; }
      try { const data = await api("contact-id", { name: item.name, group: item.group }); await copyText(data.id, "选中联系人的 ID 已复制"); }
      catch (error) { toast(error.message); }
    });
    $("#copy-url").addEventListener("click", async () => {
      try { const data = await api("state"); await copyText(data.config.url, "完整接入地址已复制，包含令牌，请勿公开"); }
      catch (error) { toast(error.message); }
    });
    $("#reveal-token").addEventListener("click", () => {
      const input = $("#token"); input.type = input.type === "password" ? "text" : "password";
    });
    $("#queue-refresh").addEventListener("click", loadQueue);
    $$(".filter[data-filter]").forEach(button => button.addEventListener("click", () => {
      state.filter = Number(button.dataset.filter);
      $$(".filter[data-filter]").forEach(b => b.classList.toggle("active", b === button));
      loadLogs();
    }));
    $("#pause-logs").addEventListener("click", () => {
      state.paused = !state.paused;
      $("#pause-logs").textContent = state.paused ? "继续刷新" : "暂停刷新";
    });
    $("#copy-logs").addEventListener("click", () => copyText($("#log-text").textContent, "当前日志已复制"));
    $("#retry-friends").addEventListener("click", async () => {
      try { const data = await api("retry-friends", {}); toast(data.notice || "已重新检查好友申请"); }
      catch (error) { toast(error.message); }
    });
    $$("#page-account input, #page-account select, #page-scope textarea, #page-advanced input").forEach(el => {
      el.addEventListener("input", () => setDirty(true));
      el.addEventListener("change", () => setDirty(true));
    });
    $$("[data-window]").forEach(button => button.addEventListener("click", () => {
      api("window", { action: button.dataset.window }).catch(e => toast(e.message));
    }));
    const drag = $("#dragRegion");
    drag.addEventListener("mousedown", event => {
      if (event.button !== 0 || event.target.closest(".wc")) return;
      if (event.detail === 2) { api("window", { action: "maximize" }).catch(() => {}); return; }
      api("window", { action: "drag" }).catch(() => {});
    });
    const zones = [["n","north"],["s","south"],["e","east"],["w","west"],["ne","north-east"],["nw","north-west"],["se","south-east"],["sw","south-west"]];
    zones.forEach(([cls, dir]) => {
      const el = document.createElement("div");
      el.className = "resize-zone " + cls;
      el.addEventListener("mousedown", event => {
        if (event.button !== 0) return;
        event.preventDefault();
        api("window", { action: "resize", direction: dir }).catch(() => {});
      });
      document.body.appendChild(el);
    });
  }

  async function boot() {
    wire();
    await refreshState();
    await pollStatus();
    const initialPage = new URLSearchParams(location.search).get("page");
    if (["account", "scope", "queue", "logs", "advanced"].includes(initialPage)) go(initialPage);
    setInterval(pollStatus, 1000);
  }
  boot();
})();
