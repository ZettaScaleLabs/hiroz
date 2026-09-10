document$.subscribe(() => {
  const elements = document.querySelectorAll("pre.mermaid > code");
  if (!elements.length) return;

  // Convert <pre class="mermaid"><code>...</code></pre>
  // into <div class="mermaid">...</div> which mermaid.js expects
  elements.forEach(el => {
    const pre = el.parentElement;
    const div = document.createElement("div");
    div.className = "mermaid";
    div.textContent = el.textContent;
    pre.replaceWith(div);
  });

  import("https://unpkg.com/mermaid@11/dist/mermaid.esm.min.mjs").then(m => {
    m.default.initialize({
      startOnLoad: false,
      theme: document.body.getAttribute("data-md-color-scheme") === "slate"
        ? "dark"
        : "default",
    });
    m.default.run();
  });
});
