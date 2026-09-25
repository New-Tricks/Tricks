// Discover webview script. Renders results with textContent only (no HTML injection).
(function () {
  const vscode = acquireVsCodeApi();
  const $ = (id) => document.getElementById(id);
  const state = vscode.getState() || {};
  let timer;

  function params() {
    return {
      query: $("q").value,
      agent: $("agent").value || undefined,
      trust: $("trust").value || undefined,
      license: $("license").value || undefined,
      noScripts: $("noScripts").checked,
      limit: 40,
    };
  }

  function run() {
    const p = params();
    vscode.setState({ ...state, params: p });
    vscode.postMessage({ type: "search", params: p });
  }

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined) e.textContent = text;
    return e;
  }

  function fmt(n) {
    if (n >= 1e6) return (n / 1e6).toFixed(1) + "M";
    if (n >= 1e3) return (n / 1e3).toFixed(1) + "k";
    return String(n);
  }

  function render(results) {
    const root = $("results");
    root.textContent = "";
    $("status").textContent = results.length ? `${results.length} result(s)` : "No results";
    for (const r of results) {
      const card = el("div", "card");
      const head = el("div", "head");
      head.appendChild(el("span", "name", r.name));
      head.appendChild(el("span", "id", r.id.replace(/^github\.com\//, "")));
      card.appendChild(head);
      if (r.description) card.appendChild(el("div", "desc", r.description));
      const tags = el("div", "tags");
      const add = (t, cls) => tags.appendChild(el("span", "tag " + (cls || ""), t));
      add(r.trust, "trust-" + r.trust);
      add(r.license_class === "allow" ? "open licence" : "licence: " + r.license_class, "lic-" + r.license_class);
      if (r.installs) add(fmt(r.installs) + " installs");
      if (r.stars) add("★ " + fmt(r.stars));
      if (r.listed_in && r.listed_in.length) add("in " + r.listed_in.length + " catalog(s)");
      if (r.duplicates && r.duplicates.length) add(r.duplicates.length + " identical cop" + (r.duplicates.length > 1 ? "ies" : "y"));
      if (r.variants) add(r.variants + " variant(s)");
      const tq = r.signals && r.signals.tessl && r.signals.tessl.quality;
      if (typeof tq === "number") add("Tessl quality " + Math.round(tq * 100) + "%");
      for (const k of r.risk || []) add(k, "risk");
      if (r.linked) add("linked", "state");
      if (r.vendored) add("vendored", "state");
      card.appendChild(tags);
      const actions = el("div", "actions");
      const btn = (label, type) => {
        const b = el("button", "", label);
        b.addEventListener("click", () => vscode.postMessage({ type, id: r.id }));
        actions.appendChild(b);
      };
      btn("Preview", "preview");
      btn("Try…", "try");
      btn("Vendor", "vendor");
      btn("Copy ID", "copy");
      card.appendChild(actions);
      root.appendChild(card);
    }
  }

  $("f").addEventListener("submit", (e) => {
    e.preventDefault();
    run();
  });
  $("q").addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(run, 400);
  });
  for (const id of ["agent", "trust", "license", "noScripts"]) $(id).addEventListener("change", run);

  window.addEventListener("message", (ev) => {
    const m = ev.data;
    if (m.type === "results") render(m.results || []);
    if (m.type === "status") $("status").textContent = m.text;
    if (m.type === "setQuery") {
      $("q").value = m.query;
      run();
    }
  });

  if (state.params) {
    $("q").value = state.params.query || "";
    $("agent").value = state.params.agent || "";
    $("trust").value = state.params.trust || "";
    $("license").value = state.params.license || "";
    $("noScripts").checked = !!state.params.noScripts;
  }
  run();
})();
