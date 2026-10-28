"use strict";

const $ = (id) => document.getElementById(id);

const FACES = {
  idle: "(・ω・)",
  searching: "(・ω・)?",
  running: "(•̀ω•́)",
  paused: "(-ω-)",
  banned: "(×ω×)",
  offline: "(;ω;)",
};

const PHASES = {
  idle: "Stopped",
  searching: "Looking for Discord…",
  running: "Running",
  paused: "Paused",
  banned: "Ban message seen",
};

/* [label, text before the first reading, command the row's right-click menu sends] */
const REMINDERS = {
  huntbot: ["Huntbot", "Press refresh to check", "owohb"],
  daily: ["owo daily", "Shows after owodaily is sent", "owodaily"],
  cookie: ["Cookie to give", "Press refresh to check", "owo cookie"],
  vote: ["Vote for OwO", "Press refresh to check", "owovote"],
};

/* [key, label, text before the first reading, command the row's right-click menu sends] */
const STATISTICS = [
  ["cash", "Cowoncy", "Not read yet", "owocash"],
  ["cookies", "Cookies", "Not read yet", "owo cookie"],
  ["luck", "Luck points", "Not read yet", "owopray"],
  ["essence", "Animal essence", "Not read yet", "owohb"],
  ["streak", "Battle streak", "Not read yet", "owob"],
  ["daily_streak", "Daily streak", "Shows after owodaily", "owodaily"],
  ["unread_mail", "Unread mail", "Shows after owodaily", null],
  ["boss_tickets", "Boss tickets", "Not read yet", "owoboss t"],
];

const FIELDS = {
  mode: ["Timed commands", "owoh, owob or both", ["hunt", "battle", "both"]],
  gem1: ["Gem 1 to use", "If you have none, the next lower ID", range(51, 57)],
  gem3: ["Gem 3 to use", "If you have none, the next lower ID", range(65, 71)],
  gem4: ["Gem 4 to use", "If you have none, the next lower ID", range(72, 78)],
  owo: ["Plain owo", "Every 11 seconds"],
  pray: ["owopray", "Every 5 minutes"],
  daily: ["owodaily", "Once a day, after OwO's daily reset"],
  run: ["oworun", "Until today's limit is used up"],
  pup: ["owopup", "Until today's limit is used up"],
  piku: ["owopiku", "Until today's limit is used up"],
  army: ["owoarmy", "Until today's limit is used up"],
  stats: ["Read stats on their own", "Every 30 to 60 minutes; the refresh buttons read them any time"],
  reminders: ["Windows reminders", "Huntbot, daily, cookie and vote"],
  huntbot: ["Automatic huntbot", "Sends owohb with your full duration when huntbot is back; you type OwO's password"],
  update_check: ["Check for updates", "At startup, from GitHub"],
};

function range(from, to) {
  const out = [];
  for (let i = from; i <= to; i++) out.push(String(i));
  return out;
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function span(ms) {
  const minutes = Math.max(1, Math.ceil(ms / 60000));
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  return minutes % 60 ? `${hours} h ${minutes % 60} min` : `${hours} h`;
}

function ago(at, now) {
  return now - at < 60000 ? "read just now" : `read ${span(now - at)} ago`;
}

function clock(ms) {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function number(value) {
  return value.toLocaleString();
}

let lastState = null;
let lastLog = -1;

function renderStatus(state) {
  const phase = state.phase;
  document.body.dataset.phase = phase;
  $("face").textContent = FACES[phase];
  $("phase").textContent = PHASES[phase];
  let reason = state.reason || "";
  if (phase === "banned") reason = "OwO posted a ban message. Check who sent it before you resume.";
  if (phase === "idle") reason = "Press Start to look for your Discord channel and begin.";
  $("reason").textContent = reason;
  $("target").textContent = state.target && phase !== "idle" ? `${state.target.app} · ${state.target.title}` : "";
  const stoppedLike = phase === "paused" || phase === "banned";
  $("start").textContent = stoppedLike ? "Resume" : "Start";
  $("start").disabled = !(phase === "idle" || stoppedLike);
  $("pause").disabled = phase !== "running";
  $("stop").disabled = phase === "idle";
  for (const id of ["refresh", "refresh-reminders"]) $(id).disabled = !(phase === "running" || stoppedLike);
  $("developer").hidden = !state.debug;
  $("simulate-captcha").disabled = phase !== "running";
  $("dump-chat").disabled = !(phase === "running" || stoppedLike);
}

/* The page polls every second; swapping identical rows would repaint the panel each time for nothing.
   A focused row keeps the focus when its panel is redrawn, so keyboard users are not thrown out of it. */
function replaceIfChanged(list, rows) {
  const before = [...list.children].map((n) => n.outerHTML).join("");
  const after = rows.map((n) => n.outerHTML).join("");
  if (before === after) return;
  const focused = [...list.children].indexOf(document.activeElement);
  list.replaceChildren(...rows);
  if (focused >= 0 && rows[focused]) rows[focused].focus();
}

/* A row whose command can be sent from the right-click menu; it takes keyboard focus for Shift+F10. */
function sendable(row, command) {
  if (!command) return row;
  row.dataset.command = command;
  row.tabIndex = 0;
  row.setAttribute("aria-describedby", "menu-hint");
  return row;
}

function renderReminders(state) {
  const rows = [];
  for (const reminder of state.reminders) {
    const [label, unknown, command] = REMINDERS[reminder.key];
    const row = sendable(el("li"), command);
    row.append(el("span", "", label));
    const at = reminder.ready_at;
    if (at === null) {
      row.append(el("span", "unknown", unknown));
    } else if (at <= state.now) {
      row.append(el("span", "ready", "Ready now"));
    } else {
      const value = el("span", "value", `in ${span(at - state.now)}`);
      value.append(el("span", "when", `at ${clock(at)}`));
      row.append(value);
    }
    rows.push(row);
  }
  replaceIfChanged($("reminders"), rows);
}

/* States where a fresh owohb makes sense; while OwO is mid-reply or waiting for the password it would only confuse it. */
const HUNTBOT_RESTARTABLE = ["waiting", "no_cowoncy", "expired", "no_reply", "no_duration"];

function huntbotStatus(state) {
  const huntbot = state.huntbot;
  const now = state.now;
  const back = state.reminders.find((r) => r.key === "huntbot")?.ready_at ?? null;
  const retry = huntbot.until ? clock(huntbot.until) : "later";
  switch (huntbot.state) {
    case "off":
      return "Automatic huntbot is off. Turn it on in Settings to have Zippy send owohb when huntbot is back.";
    case "idle":
      return "After you press Start, Zippy sends owohb with your full duration each time huntbot is back.";
    case "waiting":
      return back !== null && back > now
        ? `Hunting. Zippy starts it again when it's back, at ${clock(back)}.`
        : "Huntbot looks free. Zippy checks it with owohb next.";
    case "working":
      return "Waiting for OwO's reply to the huntbot command…";
    case "password":
      return `OwO posted a password image for ${huntbot.command}. ${span(huntbot.until - now)} left before it resets.`;
    case "no_cowoncy":
      return huntbot.have !== null && huntbot.need !== null
        ? `Not enough cowoncy: a full hunt costs ${number(huntbot.need)} and you have ${number(huntbot.have)}. Zippy checks again at ${retry}.`
        : `OwO says you don't have enough cowoncy. Zippy checks again at ${retry}.`;
    case "expired":
      return `The password reset before it was typed. Zippy asks OwO for a new one at ${retry}.`;
    case "no_reply":
      return `OwO didn't answer the huntbot commands. Zippy tries again at ${retry}.`;
    case "no_duration":
      return `Zippy couldn't read your duration from owohb. It tries again at ${retry}.`;
    default:
      return "";
  }
}

let huntbotState = null;

function renderHuntbot(state) {
  const huntbot = state.huntbot;
  const panel = $("huntbot");
  const asking = huntbot.state === "password";
  const status = huntbotStatus(state);
  if ($("huntbot-status").textContent !== status) $("huntbot-status").textContent = status;
  panel.dataset.state = huntbot.state;
  const active = ["running", "paused"].includes(state.phase);
  $("huntbot-now").disabled = !(active && HUNTBOT_RESTARTABLE.includes(huntbot.state));
  $("huntbot-send").disabled = !active;

  const form = $("huntbot-form");
  form.hidden = !asking;
  const hint = huntbot.wrong_password
    ? "OwO said that password was wrong. Check the image in Discord and type it again."
    : "Find OwO's \"Here is your password!\" message in Discord and type the letters in its image.";
  if ($("huntbot-hint").textContent !== hint) $("huntbot-hint").textContent = hint;
  $("huntbot-hint").classList.toggle("error", huntbot.wrong_password);
  /* Focus moves to the box once, when OwO starts asking, so the password can be typed straight away. */
  if (asking && huntbotState !== "password") {
    $("huntbot-feedback").textContent = "";
    $("huntbot-password").focus();
  }
  huntbotState = huntbot.state;
  document.title = asking ? "Password needed · Zippy" : "Zippy";

  const rows = [];
  const stats = state.stats;
  for (const [key, label, format] of [
    ["huntbot_hours", "Full duration", (v) => `${(v / 10).toFixed(1)} h`],
    ["huntbot_cost", "A full hunt costs", (v) => `${number(v)} cowoncy`],
  ]) {
    const row = sendable(el("div"), "owohb");
    row.append(el("dt", "", label));
    const cell = el("dd");
    const reading = stats[key];
    if (reading) {
      cell.append(el("span", "value", format(reading.value)), el("span", "when", ago(reading.at, state.now)));
    } else {
      cell.append(el("span", "unknown", "Read from owohb"));
    }
    row.append(cell);
    rows.push(row);
  }
  replaceIfChanged($("huntbot-rows"), rows);
}

async function sendPassword(event) {
  event.preventDefault();
  const input = $("huntbot-password");
  const feedback = $("huntbot-feedback");
  feedback.classList.remove("error");
  try {
    await post("/api/control", { action: "huntbot_password", password: input.value });
    feedback.textContent = "Sent to Zippy. It types the command into Discord next.";
    input.value = "";
  } catch (e) {
    feedback.textContent = `Not sent: ${e.message}`;
    feedback.classList.add("error");
    input.focus();
  }
}

function renderStatistics(state) {
  const rows = [];
  const stats = state.stats;
  for (const [key, label, unread, command] of STATISTICS) {
    const row = sendable(el("div"), command);
    row.append(el("dt", "", label));
    const cell = el("dd");
    const reading = stats[key];
    if (!reading) {
      cell.append(el("span", "unknown", unread));
    } else {
      let text = number(reading.value);
      if (key === "boss_tickets" && stats.boss_tickets_max) text += ` / ${stats.boss_tickets_max.value}`;
      if (key === "unread_mail" && reading.value === 0) text = "None";
      cell.append(el("span", "value", text));
      let when = ago(reading.at, state.now);
      if (key === "luck") when += ` · ${stats.prays_sent} prays sent this run`;
      if (key === "streak") when += ` · this run: ${stats.wins} won, ${stats.losses} lost, ${stats.draws} drawn`;
      if (key === "unread_mail" && reading.value > 0) when += " · open them with owomail";
      cell.append(el("span", "when", when));
    }
    row.append(cell);
    rows.push(row);
  }
  replaceIfChanged($("statistics"), rows);
}

/* Rows are kept between polls so a bar can slide from its old width to the new one. */
const dailyRows = new Map();

function dailyRow(command) {
  let row = dailyRows.get(command);
  if (row) return row;
  const li = sendable(el("li", "daily"), command);
  const status = el("span");
  const meter = el("div", "meter");
  meter.setAttribute("role", "progressbar");
  meter.setAttribute("aria-label", `${command} uses left today`);
  meter.setAttribute("aria-valuemin", "0");
  const fill = el("div", "fill");
  fill.style.transform = "scaleX(0)";
  meter.append(fill);
  li.append(el("span", "value", command), status, meter);
  row = { li, status, meter, fill, left: null };
  dailyRows.set(command, row);
  return row;
}

function dailyStatus(daily, now) {
  if (daily.used_up) {
    if (!daily.retry_at) return ["", "Used up for today"];
    if (daily.retry_at <= now) return ["unknown", "Checking again soon"];
    const next = span(daily.retry_at - now);
    return ["", daily.command === "owodaily" ? `Claimed today · next in ${next}` : `Used up · tries again in ${next}`];
  }
  if (daily.left === null) return ["unknown", "Not checked yet"];
  return ["count", daily.limit ? `${daily.left} of ${daily.limit} left today` : `${daily.left} left today`];
}

function renderDailies(state) {
  const list = $("dailies");
  if (state.dailies.length === 0) {
    dailyRows.clear();
    list.replaceChildren(el("li", "unknown", "None turned on. Turn them on in Settings."));
    return;
  }
  const rows = state.dailies.map((daily) => {
    const row = dailyRow(daily.command);
    const [className, text] = dailyStatus(daily, state.now);
    row.status.className = className;
    row.status.textContent = text;
    const left = daily.used_up ? 0 : daily.left ?? 0;
    const limit = Math.max(daily.limit ?? left, left, 1);
    row.fill.style.transform = `scaleX(${left / limit})`;
    row.meter.setAttribute("aria-valuemax", String(limit));
    row.meter.setAttribute("aria-valuenow", String(left));
    if (row.left !== null && daily.left !== null && daily.left < row.left) {
      row.li.classList.add("used");
      setTimeout(() => row.li.classList.remove("used"), 900);
    }
    row.left = daily.left;
    return row.li;
  });
  for (const command of dailyRows.keys()) {
    if (!state.dailies.some((daily) => daily.command === command)) dailyRows.delete(command);
  }
  const unchanged = rows.length === list.children.length && rows.every((li, i) => list.children[i] === li);
  if (!unchanged) list.replaceChildren(...rows);
}

/* The right-click menu: one item, which asks Zippy to send the row's command ahead of the routine ones.
   Zippy itself decides when it may go (cooldowns, today's limits, huntbot's password); the menu only asks. */
let menuFor = null;

function menuItemFor(command) {
  const phase = lastState?.phase;
  const running = phase === "running" || phase === "paused" || phase === "banned";
  return running ? [`Send ${command} next`, false] : [`Press Start first to send ${command}`, true];
}

function openMenu(row, event) {
  const command = row.dataset.command;
  const list = row.parentElement;
  menuFor = { command, list, index: [...list.children].indexOf(row) };
  const [text, unavailable] = menuItemFor(command);
  const item = $("command-send");
  item.textContent = text;
  item.setAttribute("aria-disabled", String(unavailable));
  const menu = $("command-menu");
  menu.hidden = false;
  /* A keyboard menu key reports no pointer position, so the menu opens under the row instead. */
  const box = row.getBoundingClientRect();
  const fromPointer = event.clientX !== 0 || event.clientY !== 0;
  const x = fromPointer ? event.clientX : box.left + 8;
  const y = fromPointer ? event.clientY : box.bottom;
  const size = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, innerWidth - size.width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, innerHeight - size.height - 8))}px`;
  item.focus();
}

function closeMenu(returnFocus) {
  if ($("command-menu").hidden) return;
  $("command-menu").hidden = true;
  const opened = menuFor;
  menuFor = null;
  if (returnFocus && opened) opened.list.children[opened.index]?.focus();
}

function queueNote(text, isError) {
  const note = $("queue-note");
  note.textContent = text;
  note.classList.toggle("error", Boolean(isError));
}

async function sendFromMenu() {
  if (!menuFor || $("command-send").getAttribute("aria-disabled") === "true") return;
  const { command } = menuFor;
  closeMenu(true);
  try {
    await post("/api/control", { action: "send", command });
    queueNote(`Asked Zippy to send ${command} next. Activity shows when it goes out, or why it has to wait.`);
  } catch (e) {
    queueNote(`Could not ask for ${command}: ${e.message}`, true);
  }
}

function logItem(line) {
  const item = el("li");
  item.append(el("time", "", clock(line.t)));
  item.append(document.createTextNode(line.text));
  return item;
}

/* Only new lines are added and the oldest dropped, so a busy log never rebuilds (and repaints) the whole list. */
function renderLog(state) {
  if (state.log_seq === lastLog) return;
  const added = state.log_seq - lastLog;
  lastLog = state.log_seq;
  const box = $("log");
  const atBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 24;
  const lines = state.log;
  if (added > 0 && added < lines.length && box.children.length > 0) {
    box.append(...lines.slice(-added).map(logItem));
    while (box.children.length > lines.length) box.firstElementChild.remove();
  } else {
    box.replaceChildren(...lines.map(logItem));
  }
  if (atBottom) box.scrollTop = box.scrollHeight;
}

/* The key arrives in the link's #fragment, moves to sessionStorage (this port only), and leaves the address bar. */
function takeKey() {
  const match = location.hash.match(/^#key=([0-9a-f]{64})$/);
  if (match) {
    sessionStorage.setItem("zippy-key", match[1]);
    history.replaceState(null, "", "/");
  }
  return sessionStorage.getItem("zippy-key");
}

const KEY = takeKey();

function api(path, options = {}) {
  const headers = { ...(options.headers || {}), Authorization: `Bearer ${KEY}` };
  return fetch(path, { ...options, headers, cache: "no-store" });
}

function showLocked() {
  sessionStorage.removeItem("zippy-key");
  $("locked").hidden = false;
  for (const node of document.querySelectorAll(".layout, .top-actions, #offline")) node.hidden = true;
}

/* Set while the theme reveal runs: the new theme is a live picture of the page, so any change to the page then
   would be repainted under the moving circle on every frame. The newest state waits here and is drawn after. */
let revealing = false;
let heldState = null;

function render(state) {
  $("offline").hidden = true;
  $("version").textContent = `v${state.version}`;
  renderStatus(state);
  renderHuntbot(state);
  renderReminders(state);
  renderStatistics(state);
  renderDailies(state);
  renderLog(state);
}

async function poll() {
  try {
    const response = await api("/api/state");
    if (response.status === 401) {
      showLocked();
      return;
    }
    if (!response.ok) throw new Error(String(response.status));
    const state = await response.json();
    lastState = state;
    if (revealing) heldState = state;
    else render(state);
  } catch {
    $("offline").hidden = false;
    document.body.dataset.phase = "offline";
    $("face").textContent = FACES.offline;
    $("phase").textContent = lastState ? "Lost connection" : "Can't reach Zippy";
    closeMenu(false);
    for (const id of ["start", "pause", "stop", "refresh", "refresh-reminders", "huntbot-now", "huntbot-send", "simulate-captcha", "dump-chat"]) {
      $(id).disabled = true;
    }
  } finally {
    setTimeout(poll, 1000);
  }
}

async function post(path, body) {
  const response = await api(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    const error = await response.json().catch(() => ({ error: `HTTP ${response.status}` }));
    throw new Error(error.error || `HTTP ${response.status}`);
  }
  return response.json();
}

function control(action) {
  post("/api/control", { action }).catch((e) => {
    $("reason").textContent = `Could not ${action}: ${e.message}`;
  });
}

/* Developer actions answer inside the Settings dialog, where they were pressed. */
function developerAction(action, done) {
  post("/api/control", { action })
    .then(() => saveState(done))
    .catch((e) => saveState(`Not done: ${e.message}`, true));
}

/* The hidden switch: tap "Start" in the Settings note five times in a row. */
const EGG_TAPS = 5;
const EGG_WINDOW_MS = 1500;
let eggTaps = 0;
let eggLastTap = 0;

async function knock() {
  if (!$("developer").hidden) {
    saveState("Developer mode is already on.");
    return;
  }
  const now = Date.now();
  eggTaps = now - eggLastTap < EGG_WINDOW_MS ? eggTaps + 1 : 1;
  eggLastTap = now;
  const left = EGG_TAPS - eggTaps;
  if (left > 0) {
    if (eggTaps >= 3) saveState(left === 1 ? "One more tap…" : `${left} more taps…`);
    return;
  }
  eggTaps = 0;
  try {
    await post("/api/control", { action: "developer_mode" });
    const section = $("developer");
    section.hidden = false;
    section.classList.add("unlocked");
    section.scrollIntoView({ block: "nearest" });
    saveState("You found developer mode. It stays on until Zippy closes.");
  } catch (e) {
    saveState(`Could not unlock developer mode: ${e.message}`, true);
  }
}

function saveState(text, isError) {
  const node = $("save-state");
  node.textContent = text;
  node.classList.toggle("error", Boolean(isError));
}

function renderFields(values) {
  const box = $("fields");
  box.replaceChildren();
  for (const { key, value } of values) {
    const spec = FIELDS[key];
    if (!spec) continue;
    const [label, hint, options] = spec;
    const row = el("div", "field");
    const id = `field-${key}`;
    const text = el("label", "", label);
    text.htmlFor = id;
    text.append(el("small", "", hint));
    row.append(text);
    let input;
    if (options) {
      input = el("select");
      for (const option of options) {
        const choice = el("option", "", option);
        choice.value = option;
        input.append(choice);
      }
      input.value = value;
    } else {
      input = el("input");
      input.type = "checkbox";
      input.checked = value === "on";
    }
    input.id = id;
    input.addEventListener("change", async () => {
      const next = options ? input.value : input.checked ? "on" : "off";
      saveState("Saving…");
      try {
        const saved = await post("/api/settings", { key, value: next });
        renderFields(saved.values);
        saveState(`Saved: ${label} is now ${next}.`);
        $(id).focus();
      } catch (e) {
        saveState(`Not saved: ${e.message}`, true);
        if (options) input.value = value;
        else input.checked = value === "on";
      }
    });
    row.append(input);
    box.append(row);
  }
}

async function openSettings() {
  saveState("");
  try {
    const response = await api("/api/settings");
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    renderFields((await response.json()).values);
  } catch (e) {
    $("fields").replaceChildren();
    saveState(`Could not load settings: ${e.message}`, true);
  }
  $("settings").showModal();
}

function applyTheme(theme) {
  const dark = theme === "dark" || (theme === null && matchMedia("(prefers-color-scheme: dark)").matches);
  if (theme) document.documentElement.dataset.theme = theme;
  $("theme").setAttribute("aria-pressed", String(dark));
  $("theme-tip").textContent = dark ? "Switch to light" : "Switch to dark";
}

let revealCount = 0;

/* The new theme spreads out from the toggle; browsers without view transitions just switch. */
function switchTheme() {
  const button = $("theme");
  const next = button.getAttribute("aria-pressed") === "true" ? "light" : "dark";
  const commit = () => {
    localStorage.setItem("zippy-theme", next);
    applyTheme(next);
  };
  if (!document.startViewTransition) {
    commit();
    return;
  }
  const box = button.getBoundingClientRect();
  const x = box.left + box.width / 2;
  const y = box.top + box.height / 2;
  const reach = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y));
  const root = document.documentElement;
  root.style.setProperty("--reveal-x", `${x}px`);
  root.style.setProperty("--reveal-y", `${y}px`);
  root.style.setProperty("--reveal-r", `${reach}px`);
  root.classList.add("revealing");
  revealing = true;
  /* A second click mid-reveal starts a new transition and ends the first; only the newest one cleans up. */
  const reveal = ++revealCount;
  document.startViewTransition(commit).finished.finally(() => {
    if (reveal !== revealCount) return;
    root.classList.remove("revealing");
    revealing = false;
    if (heldState) render(heldState);
    heldState = null;
  });
}

document.addEventListener("DOMContentLoaded", () => {
  applyTheme(localStorage.getItem("zippy-theme"));
  $("theme").addEventListener("click", switchTheme);
  $("start").addEventListener("click", () => control("start"));
  $("pause").addEventListener("click", () => control("pause"));
  $("stop").addEventListener("click", () => control("stop"));
  /* One turn of the icon confirms the reads were queued. */
  const refreshing = (id, action) => () => {
    const button = $(id);
    button.classList.remove("spun");
    void button.offsetWidth;
    button.classList.add("spun");
    control(action);
  };
  $("refresh").addEventListener("click", refreshing("refresh", "refresh"));
  $("refresh-reminders").addEventListener("click", refreshing("refresh-reminders", "refresh_reminders"));
  $("simulate-captcha").addEventListener("click", () =>
    developerAction("simulate_captcha", "Captcha simulated. Zippy pauses and rings at its next check."),
  );
  $("dump-chat").addEventListener("click", () =>
    developerAction("dump_chat", "Chat messages written to Activity."),
  );
  $("huntbot-now").addEventListener("click", () => control("huntbot"));
  $("huntbot-form").addEventListener("submit", sendPassword);
  document.addEventListener("contextmenu", (event) => {
    const row = event.target.closest?.("[data-command]");
    if (!row) return;
    event.preventDefault();
    openMenu(row, event);
  });
  $("command-send").addEventListener("click", sendFromMenu);
  $("command-menu").addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeMenu(true);
    } else if (event.key === "Tab") {
      closeMenu(false);
    }
  });
  document.addEventListener("pointerdown", (event) => {
    if (!$("command-menu").contains(event.target)) closeMenu(false);
  });
  /* Only the page scrolling moves the row away; the Activity log scrolling itself as lines arrive does not. */
  addEventListener("scroll", (event) => event.target === document && closeMenu(false), true);
  addEventListener("resize", () => closeMenu(false));
  addEventListener("blur", () => closeMenu(false));
  $("egg").addEventListener("click", knock);
  $("open-settings").addEventListener("click", openSettings);
  if (KEY) poll();
  else showLocked();
});
