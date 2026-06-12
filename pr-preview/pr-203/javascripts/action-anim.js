/* Action lifecycle animation — driven by requestAnimationFrame */

function initActionAnims() {
  document.querySelectorAll(".action-demo").forEach(function(demo) {
    if (demo.dataset.animInit) return;
    demo.dataset.animInit = "1";

    var svg = demo.querySelector("svg.action-anim-svg");
    if (!svg) return;

    // Steps: [delay_ms, from, to, label, color, type]
    // from/to: "client" or "server"
    var steps = [
      [400,  "client", "server", "Goal",            "#3f51b5", "goal"],
      [1400, "server", "client", "Accepted",         "#2e7d32", "accept"],
      [2400, "server", "client", "Feedback  30%",    "#f57c00", "feedback"],
      [3400, "server", "client", "Feedback  65%",    "#f57c00", "feedback"],
      [4400, "server", "client", "Feedback 100%",    "#f57c00", "feedback"],
      [5400, "server", "client", "Result: Succeeded","#1565c0", "result"],
    ];

    var W = svg.viewBox.baseVal.width || 500;
    var H = svg.viewBox.baseVal.height || 320;
    var LX = 90,  RX = W - 90;  // lifeline x positions
    var TOP = 56; // top of lifelines
    var STEP_H = 44; // vertical step per message
    var ARROW_H = 18; // y-center of arrow for each step

    var ns = "http://www.w3.org/2000/svg";

    // Draw static elements
    function drawStatic() {
      // Actor boxes
      var actors = [
        {x: LX-60, label: "Action\nClient"},
        {x: RX-60, label: "Action\nServer"},
      ];
      actors.forEach(function(a) {
        var rect = document.createElementNS(ns, "rect");
        rect.setAttribute("x", a.x); rect.setAttribute("y", 4);
        rect.setAttribute("width", 120); rect.setAttribute("height", 44);
        rect.setAttribute("rx", 6);
        rect.setAttribute("fill", "#3f51b5");
        svg.appendChild(rect);

        var lines = a.label.split("\n");
        lines.forEach(function(line, i) {
          var t = document.createElementNS(ns, "text");
          t.setAttribute("x", a.x + 60);
          t.setAttribute("y", 22 + i * 16);
          t.setAttribute("text-anchor", "middle");
          t.setAttribute("dominant-baseline", "middle");
          t.setAttribute("fill", "#fff");
          t.setAttribute("font-size", "13");
          t.setAttribute("font-weight", "bold");
          t.textContent = line;
          svg.appendChild(t);
        });
      });

      // Lifelines
      [LX, RX].forEach(function(x) {
        var line = document.createElementNS(ns, "line");
        line.setAttribute("x1", x); line.setAttribute("y1", TOP);
        line.setAttribute("x2", x); line.setAttribute("y2", H - 16);
        line.setAttribute("stroke", "#3f51b5");
        line.setAttribute("stroke-opacity", "0.3");
        line.setAttribute("stroke-width", "2");
        line.setAttribute("stroke-dasharray", "4 4");
        svg.appendChild(line);
      });

      // Step row labels + horizontal dashed guides
      steps.forEach(function(step, i) {
        var y = TOP + 30 + i * STEP_H;
        var guide = document.createElementNS(ns, "line");
        guide.setAttribute("x1", LX); guide.setAttribute("y1", y);
        guide.setAttribute("x2", RX); guide.setAttribute("y2", y);
        guide.setAttribute("stroke", "#999");
        guide.setAttribute("stroke-opacity", "0.12");
        guide.setAttribute("stroke-width", "1");
        svg.appendChild(guide);
      });
    }

    drawStatic();

    // Animated arrows
    var arrowGroups = [];
    steps.forEach(function(step, i) {
      var y = TOP + 30 + i * STEP_H;
      var fromX = step[1] === "client" ? LX : RX;
      var toX   = step[2] === "client" ? LX : RX;
      var color = step[4];

      var g = document.createElementNS(ns, "g");
      g.setAttribute("opacity", "0");

      // Arrow shaft
      var line = document.createElementNS(ns, "line");
      line.setAttribute("y1", y); line.setAttribute("y2", y);
      line.setAttribute("x1", fromX); line.setAttribute("x2", fromX);
      line.setAttribute("stroke", color);
      line.setAttribute("stroke-width", "2.5");
      g.appendChild(line);

      // Arrowhead
      var ah = document.createElementNS(ns, "polygon");
      var dir = toX > fromX ? 1 : -1;
      // tip at toX
      var tip = toX, base = toX - dir * 12;
      ah.setAttribute("points",
        tip + "," + y + " " +
        base + "," + (y - 7) + " " +
        base + "," + (y + 7)
      );
      ah.setAttribute("fill", color);
      g.appendChild(ah);

      // Label pill
      var midX = (fromX + toX) / 2;
      var pill = document.createElementNS(ns, "rect");
      var textEl = document.createElementNS(ns, "text");
      textEl.setAttribute("x", midX);
      textEl.setAttribute("y", y - 10);
      textEl.setAttribute("text-anchor", "middle");
      textEl.setAttribute("dominant-baseline", "middle");
      textEl.setAttribute("fill", "#fff");
      textEl.setAttribute("font-size", "11");
      textEl.setAttribute("font-weight", "600");
      textEl.textContent = step[3];
      g.appendChild(pill); // add pill before text for z-order
      g.appendChild(textEl);

      svg.appendChild(g);

      // Store refs for animation
      arrowGroups.push({
        g: g, line: line, ah: ah, pill: pill, textEl: textEl,
        fromX: fromX, toX: toX, color: color, midX: midX, y: y,
        delay: step[0]
      });
    });

    // After text is in DOM, measure pill widths
    arrowGroups.forEach(function(ag) {
      try {
        var bb = ag.textEl.getBBox();
        ag.pill.setAttribute("x", bb.x - 6);
        ag.pill.setAttribute("y", bb.y - 3);
        ag.pill.setAttribute("width", bb.width + 12);
        ag.pill.setAttribute("height", bb.height + 6);
        ag.pill.setAttribute("rx", 8);
        ag.pill.setAttribute("fill", ag.color);
      } catch(e) {}
    });

    // "Executing" label on server side
    var execLabel = document.createElementNS(ns, "text");
    execLabel.setAttribute("x", RX);
    execLabel.setAttribute("y", TOP + 70);
    execLabel.setAttribute("text-anchor", "middle");
    execLabel.setAttribute("fill", "#f57c00");
    execLabel.setAttribute("font-size", "11");
    execLabel.setAttribute("font-weight", "600");
    execLabel.setAttribute("opacity", "0");
    execLabel.textContent = "⚙ Executing…";
    svg.appendChild(execLabel);

    // Animation loop
    var CYCLE = 7800; // ms per full cycle

    function animateArrow(ag, cycleStart) {
      var t = performance.now();
      var elapsed = (t - cycleStart - ag.delay);
      var TRAVEL = 700; // ms for arrow to travel

      if (elapsed < 0) {
        ag.g.setAttribute("opacity", "0");
        ag.line.setAttribute("x2", ag.fromX);
        return;
      }
      if (elapsed > TRAVEL + 600) {
        // fade out
        var fade = Math.max(0, 1 - (elapsed - TRAVEL - 600) / 300);
        ag.g.setAttribute("opacity", fade);
        ag.line.setAttribute("x2", ag.toX);
        return;
      }

      ag.g.setAttribute("opacity", "1");
      if (elapsed <= TRAVEL) {
        var progress = elapsed / TRAVEL;
        var curX = ag.fromX + (ag.toX - ag.fromX) * progress;
        ag.line.setAttribute("x2", curX);
        // move arrowhead with tip
        var dir = ag.toX > ag.fromX ? 1 : -1;
        var base = curX - dir * 12;
        ag.ah.setAttribute("points",
          curX + "," + ag.y + " " +
          base + "," + (ag.y - 7) + " " +
          base + "," + (ag.y + 7)
        );
      } else {
        ag.line.setAttribute("x2", ag.toX);
      }
    }

    var cycleStart = performance.now();

    function tick() {
      var t = performance.now();
      var elapsed = (t - cycleStart) % CYCLE;
      var thisCycleStart = t - elapsed;

      // Executing label: visible during feedback phase
      var execVisible = elapsed > 1800 && elapsed < 5200;
      execLabel.setAttribute("opacity", execVisible ? "1" : "0");

      arrowGroups.forEach(function(ag) {
        animateArrow(ag, thisCycleStart);
      });

      requestAnimationFrame(tick);
    }

    requestAnimationFrame(tick);
  });
}

document$.subscribe(function() {
  // Small delay to let MkDocs finish rendering
  setTimeout(initActionAnims, 100);
});
